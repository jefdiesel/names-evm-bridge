#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# repro-build.sh: build every deployable zkVM guest inside RISC Zero's digest-pinned Docker
# builder and print `guest name, image id` lines on stdout.
#
#   scripts/repro-build.sh [--out FILE] [--elf-dir DIR] [--only g1,g2,...]
#                          [--include-research] [--jobs N] [--allow-unpinned-digest] [--list]
#
# How it works:
# 1. Pulls the builder image strictly by content-addressed digest (`BUILDER_IMAGE@BUILDER_DIGEST`),
#    verifies its RepoDigests, and tags it to `BUILDER_IMAGE:BUILDER_TAG`.
# 2. Runs `RISC0_USE_DOCKER=1 RISC0_DOCKER_CONTAINER_TAG=$BUILDER_TAG cargo check --release --locked`
#    on each guest-embedding crate so each build.rs invokes risc0-build's DockerOptions with the
#    proper workspace root_dir.
# 3. Reads `methods.rs` from Cargo's exact `build-script-executed` out_dir and converts each
#    `*_ID: [u32; 8]` constant to the 0x-prefixed 32-byte little-endian hex image id.
# 4. Cross-checks that `release_guest` embedded by outbound_host matches `release_guest` embedded
#    by release_host.
#
# Progress and cargo stderr go to stderr; stdout emits only `<guest_name>, 0x<64-hex>` lines,
# suitable for `node scripts/check-image-ids.mjs --input <file>`.
#
# Builder image pin (kept in sync with VERSIONS.toml [toolchain] by scripts/check-versions.mjs):
set -euo pipefail

BUILDER_IMAGE="${RISC0_BUILDER_IMAGE:-risczero/risc0-guest-builder}"
BUILDER_TAG="${RISC0_BUILDER_TAG:-r0.1.97.0}"
BUILDER_DIGEST="${RISC0_BUILDER_DIGEST:-sha256:7ae0a27fa72b10fb5c1066005bd949ae43dc4ead8a284f2d6572b99c82ab4468}"

ROOT=$(cd "$(dirname "$0")/.." && pwd -P)

OUT_FILE=""
ELF_DIR=""
ONLY=""
INCLUDE_RESEARCH=0
ALLOW_UNPINNED=0
LIST_ONLY=0
JOBS="${CARGO_BUILD_JOBS:-4}"

usage() {
  cat <<'EOF'
usage: scripts/repro-build.sh [options]

Build every deployable zkVM guest reproducibly in RISC Zero's Docker builder and print
`guest name, image id` lines on stdout.

Options:
  --out FILE                Also write `guest name, image id` lines to FILE
  --elf-dir DIR             Copy compiled guest ELFs to DIR/<guest>.bin and write SHA256SUMS
  --only g1,g2,...          Build only the named guest(s) (comma-separated)
  --include-research        Also build attest-spike research guests
  --jobs N, -j N            Cargo parallel jobs (default: CARGO_BUILD_JOBS or 4)
  --allow-unpinned-digest   Bootstrap only: pull by tag when BUILDER_DIGEST is not yet pinned
  --list                    Print the configured guest table and exit (no Docker/Cargo)
  --help, -h                Show this message
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT_FILE="${2:?--out requires a path}"; shift 2 ;;
    --elf-dir) ELF_DIR="${2:?--elf-dir requires a directory}"; shift 2 ;;
    --only) ONLY="${2:?--only requires a comma-separated list}"; shift 2 ;;
    --include-research) INCLUDE_RESEARCH=1; shift ;;
    --jobs|-j) JOBS="${2:?--jobs requires an integer}"; shift 2 ;;
    --allow-unpinned-digest) ALLOW_UNPINNED=1; shift ;;
    --list) LIST_ONLY=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "repro-build: unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done

# Guest build table: workspace | crate | guest_name | methods.rs constant prefix | group
# Order is deterministic. `release_guest` is built in both outbound_host and release_host and
# cross-checked for equality.
DEPLOYABLE_SPECS=(
  "lez-bridge|bridge_methods|token_guest|TOKEN_GUEST|deployable"
  "lez-bridge|bridge_methods|bridge_guest|BRIDGE_GUEST|deployable"
  "lez-bridge|attest_methods|attest_guest|ATTEST_GUEST|deployable"
  "lez-bridge|attest_methods|attest_probe_guest|ATTEST_PROBE_GUEST|deployable"
  "lez-bridge/outbound|outbound_host|outbound_guest|OUTBOUND_GUEST|deployable"
  "lez-bridge/outbound|outbound_host|outbound_guest_trusted_tip|OUTBOUND_GUEST_TRUSTED_TIP|deployable"
  "lez-bridge/outbound|outbound_host|release_guest|RELEASE_GUEST|deployable"
  "lez-bridge/release|release_host|release_guest|RELEASE_GUEST|crosscheck"
  "lez-bridge/outbound-seqsig|seqsig_host|outbound_guest_seqsig|OUTBOUND_GUEST_SEQSIG|deployable"
  "lez-bridge/outbound-seqsig|seqsig_host|seqsig_evidence_guest|SEQSIG_EVIDENCE_GUEST|deployable"
  "lez-bridge/reclaim|reclaim_host|reclaim_guest|RECLAIM_GUEST|deployable"
)

RESEARCH_SPECS=(
  "attest-spike|methods|consensus-guest|CONSENSUS_GUEST|research"
  "attest-spike|methods|inclusion-guest|INCLUSION_GUEST|research"
  "attest-spike|methods|inclusion-guest-sw|INCLUSION_GUEST_SW|research"
)

SPECS=("${DEPLOYABLE_SPECS[@]}")
if [ "$INCLUDE_RESEARCH" = 1 ]; then
  SPECS+=("${RESEARCH_SPECS[@]}")
fi

want_guest() {
  local name=$1
  if [ -z "$ONLY" ]; then return 0; fi
  case ",$ONLY," in
    *,"$name",*) return 0 ;;
    *) return 1 ;;
  esac
}

if [ "$LIST_ONLY" = 1 ]; then
  echo "builder: ${BUILDER_IMAGE}:${BUILDER_TAG} (${BUILDER_DIGEST})"
  printf '%-28s %-26s %-16s %s\n' "GUEST" "WORKSPACE" "CRATE" "KIND"
  for spec in "${SPECS[@]}"; do
    IFS='|' read -r ws pkg guest const kind <<< "$spec"
    if want_guest "$guest"; then
      printf '%-28s %-26s %-16s %s\n' "$guest" "$ws" "$pkg" "$kind"
    fi
  done
  exit 0
fi

# Validate --only names against the known specs.
if [ -n "$ONLY" ]; then
  IFS=',' read -r -a requested <<< "$ONLY"
  for req in "${requested[@]}"; do
    found=0
    for spec in "${SPECS[@]}"; do
      IFS='|' read -r _ _ guest _ _ <<< "$spec"
      if [ "$guest" = "$req" ]; then found=1; break; fi
    done
    if [ "$found" = 0 ]; then
      echo "repro-build: unknown guest '$req' (use --list to see valid names; pass --include-research for attest-spike)" >&2
      exit 2
    fi
  done
fi

for bin in docker cargo python3; do
  if ! command -v "$bin" >/dev/null 2>&1; then
    echo "repro-build: required tool not found in PATH: $bin" >&2
    exit 2
  fi
done

# risc0-build (3.0.5 and 3.0.7) asks rzup for the default RISC Zero Rust toolchain even when it
# builds in Docker (only to choose the `lower-atomic` LLVM pass name; any version above 1.81 gives
# the same flags) and panics without one. The guests compile inside the builder image.
rzup_show=$(rzup show 2>/dev/null || true)
if [[ "$rzup_show" != *1.97.0* ]]; then
  echo "repro-build: risc0-build needs rzup with RISC Zero Rust installed on the host (rzup install rust 1.97.0)" >&2
  exit 2
fi

if ! docker info >/dev/null 2>&1; then
  echo "repro-build: Docker daemon is not running or not reachable" >&2
  exit 1
fi

# Pull and verify the builder image by content-addressed digest.
pull_builder() {
  if [[ "$BUILDER_DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    local ref="${BUILDER_IMAGE}@${BUILDER_DIGEST}"
    echo "repro-build: pulling pinned builder ${ref}" >&2
    docker pull "$ref" >&2
    local digests
    digests=$(docker image inspect --format='{{json .RepoDigests}}' "$ref")
    case "$digests" in
      *"$ref"*) ;;
      *)
        echo "repro-build: pulled image RepoDigests ($digests) do not contain ${ref}" >&2
        exit 1
        ;;
    esac
    docker tag "$ref" "${BUILDER_IMAGE}:${BUILDER_TAG}"
    local tagged_id
    tagged_id=$(docker image inspect --format='{{.Id}}' "${BUILDER_IMAGE}:${BUILDER_TAG}")
    echo "repro-build: verified ${BUILDER_IMAGE}:${BUILDER_TAG} -> ${ref} (id ${tagged_id})" >&2
  elif [ "$ALLOW_UNPINNED" = 1 ]; then
    echo "repro-build: WARNING: BUILDER_DIGEST is not pinned (${BUILDER_DIGEST}); pulling mutable tag ${BUILDER_IMAGE}:${BUILDER_TAG}" >&2
    docker pull "${BUILDER_IMAGE}:${BUILDER_TAG}" >&2
    echo "repro-build: resolved RepoDigests (pin one in VERSIONS.toml, scripts/repro-build.sh, docs/REPRODUCIBLE-BUILDS.md):" >&2
    docker image inspect --format='{{json .RepoDigests}}' "${BUILDER_IMAGE}:${BUILDER_TAG}" >&2
  else
    cat >&2 <<EOF
repro-build: BUILDER_DIGEST is not pinned to a sha256 digest: ${BUILDER_DIGEST}

To obtain the digest on a machine with Docker:
  docker buildx imagetools inspect ${BUILDER_IMAGE}:${BUILDER_TAG}
or:
  docker pull ${BUILDER_IMAGE}:${BUILDER_TAG} && docker image inspect --format='{{json .RepoDigests}}' ${BUILDER_IMAGE}:${BUILDER_TAG}

Record the sha256:... digest in VERSIONS.toml ([toolchain].docker_builder_digest),
scripts/repro-build.sh (BUILDER_DIGEST), and docs/REPRODUCIBLE-BUILDS.md, then re-run.
(For initial bootstrap discovery only, pass --allow-unpinned-digest.)
EOF
    exit 2
  fi
}

pull_builder

# risc0-build writes `FROM risczero/risc0-guest-builder:<RISC0_DOCKER_CONTAINER_TAG>`. Passing
# `<tag>@<digest>` makes Docker itself refuse any other image, whatever the local tag points at
# (risc0-build 3.0.5's own default tag is r0.1.88.0 and 3.0.7's r0.1.97.0: both are overridden).
if [[ "$BUILDER_DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  CONTAINER_TAG="${BUILDER_TAG}@${BUILDER_DIGEST}"
else
  CONTAINER_TAG="$BUILDER_TAG"
fi
echo "repro-build: guests build FROM ${BUILDER_IMAGE}:${CONTAINER_TAG}" >&2

TMPDIR_WORK=$(mktemp -d "${TMPDIR:-/tmp}/repro-build.XXXXXX")
trap 'rm -rf "$TMPDIR_WORK"' EXIT

if [ -n "$ELF_DIR" ]; then
  mkdir -p "$ELF_DIR"
fi
if [ -n "$OUT_FILE" ]; then
  mkdir -p "$(dirname "$OUT_FILE")"
  : > "$OUT_FILE"
fi

# Collect unique (workspace|crate) targets needed for the requested guests.
TARGETS=()
for spec in "${SPECS[@]}"; do
  IFS='|' read -r ws pkg guest _ _ <<< "$spec"
  if ! want_guest "$guest"; then continue; fi
  key="${ws}|${pkg}"
  seen=0
  for t in "${TARGETS[@]:-}"; do
    if [ "$t" = "$key" ]; then seen=1; break; fi
  done
  if [ "$seen" = 0 ]; then
    TARGETS+=("$key")
  fi
done

# Build each (workspace, crate) once with RISC0_USE_DOCKER=1 and save Cargo's JSON messages.
idx=0
for target in "${TARGETS[@]}"; do
  IFS='|' read -r ws pkg <<< "$target"
  idx=$((idx + 1))
  json_log="$TMPDIR_WORK/cargo-${idx}.jsonl"
  echo "repro-build: [$idx/${#TARGETS[@]}] building guests via ${ws} (-p ${pkg}) with RISC0_USE_DOCKER=1" >&2
  (
    cd "$ROOT/$ws"
    RISC0_USE_DOCKER=1 \
    RISC0_DOCKER_CONTAINER_TAG="$CONTAINER_TAG" \
    cargo check --release --locked -j "$JOBS" -p "$pkg" --message-format=json \
      > "$json_log"
  )
  printf '%s|%s|%s\n' "$ws" "$pkg" "$json_log" >> "$TMPDIR_WORK/target-logs.txt"
done

# Extract each guest's image id (and optional ELF path) from the exact methods.rs built above.
EXTRACT_OUT="$TMPDIR_WORK/extracted.tsv"
python3 -I - "$TMPDIR_WORK/target-logs.txt" "$EXTRACT_OUT" "${SPECS[@]}" <<'PY'
import json
import os
import re
import struct
import sys

logs_file = sys.argv[1]
out_file = sys.argv[2]
specs = sys.argv[3:]

# Map (ws, pkg) -> json_log path
target_logs = {}
with open(logs_file, "r", encoding="utf-8") as fh:
    for line in fh:
        line = line.strip()
        if not line:
            continue
        ws, pkg, log_path = line.split("|", 2)
        target_logs[(ws, pkg)] = log_path


def find_out_dir(log_path, pkg):
    """Find the build-script-executed out_dir for `pkg` from Cargo JSON output."""
    matches = []
    # Match both Cargo v2 (`pkg 0.1.0 (...)`) and v3 (`path+file:///...#pkg@0.1.0` or `/pkg#0.1.0`)
    pkg_re = re.compile(
        r"(?:^" + re.escape(pkg) + r"\s|[#/]" + re.escape(pkg) + r"(?:@|#|$))"
    )
    with open(log_path, "r", encoding="utf-8") as fh:
        for raw in fh:
            raw = raw.strip()
            if not raw.startswith("{"):
                continue
            try:
                msg = json.loads(raw)
            except json.JSONDecodeError:
                continue
            if msg.get("reason") != "build-script-executed":
                continue
            pid = msg.get("package_id", "")
            out_dir = msg.get("out_dir", "")
            if pkg_re.search(pid) and out_dir and os.path.isfile(os.path.join(out_dir, "methods.rs")):
                matches.append(out_dir)
    if not matches:
        raise RuntimeError(f"no build-script-executed out_dir with methods.rs found for {pkg} in {log_path}")
    return matches[-1]


def parse_methods_rs(methods_rs, const_prefix):
    text = open(methods_rs, "r", encoding="utf-8").read()
    m = re.search(
        r"pub\s+const\s+" + re.escape(const_prefix) + r"_ID\s*:\s*\[u32\s*;\s*8\]\s*=\s*\[([^\]]+)\]",
        text,
        re.DOTALL,
    )
    if not m:
        raise RuntimeError(f"{const_prefix}_ID not found in {methods_rs}")
    raw_items = [tok.strip() for tok in m.group(1).split(",") if tok.strip()]
    if len(raw_items) != 8:
        raise RuntimeError(f"{const_prefix}_ID in {methods_rs} has {len(raw_items)} words, expected 8")
    words = []
    for tok in raw_items:
        tok_clean = re.sub(r"_?u32$", "", tok).replace("_", "")
        words.append(int(tok_clean, 0))
    image_id = "0x" + b"".join(struct.pack("<I", w) for w in words).hex()

    elf_path = ""
    pm = re.search(
        r"pub\s+const\s+" + re.escape(const_prefix) + r'_PATH\s*:\s*&str\s*=\s*"([^"]+)"',
        text,
    )
    if pm:
        elf_path = pm.group(1)
    else:
        im = re.search(
            r"pub\s+const\s+" + re.escape(const_prefix) + r'_ELF\s*:\s*&\[u8\]\s*=\s*include_bytes!\(\s*"([^"]+)"\s*\)',
            text,
        )
        if im:
            elf_path = im.group(1)
    return image_id, elf_path


out_dirs = {}
results = []
crosschecks = []

for spec in specs:
    ws, pkg, guest, const_prefix, kind = spec.split("|")
    if (ws, pkg) not in target_logs:
        continue
    if (ws, pkg) not in out_dirs:
        out_dirs[(ws, pkg)] = find_out_dir(target_logs[(ws, pkg)], pkg)
    methods_rs = os.path.join(out_dirs[(ws, pkg)], "methods.rs")
    image_id, elf_path = parse_methods_rs(methods_rs, const_prefix)
    # risc0-build writes Docker-built ELFs under <target>/riscv32im-risc0-zkvm-elf/docker/; a
    # native build (RISC0_USE_DOCKER ignored by a build.rs) would land elsewhere: refuse it.
    if "/riscv32im-risc0-zkvm-elf/docker/" not in elf_path:
        raise RuntimeError(f"{guest}: ELF {elf_path!r} is not a Docker build output")
    if kind == "crosscheck":
        crosschecks.append((guest, ws, pkg, image_id))
    else:
        results.append((guest, image_id, elf_path, ws, pkg))

# Cross-check duplicate embeddings (e.g., release_guest in outbound_host vs release_host).
by_guest = {g: iid for g, iid, _, _, _ in results}
for guest, ws, pkg, iid in crosschecks:
    if guest in by_guest and by_guest[guest] != iid:
        raise RuntimeError(
            f"reproducibility mismatch for {guest}: {by_guest[guest]} vs {ws}/{pkg} {iid}"
        )
    if guest not in by_guest:
        results.append((guest, iid, "", ws, pkg))

with open(out_file, "w", encoding="utf-8") as fh:
    for guest, iid, elf_path, ws, pkg in results:
        fh.write(f"{guest}\t{iid}\t{elf_path}\n")
PY

# Emit `guest name, image id` lines (and optionally copy ELFs + SHA256SUMS).
while IFS=$'\t' read -r guest image_id elf_path; do
  if ! want_guest "$guest"; then continue; fi
  line="${guest}, ${image_id}"
  echo "$line"
  if [ -n "$OUT_FILE" ]; then
    echo "$line" >> "$OUT_FILE"
  fi
  if [ -n "$ELF_DIR" ] && [ -n "$elf_path" ] && [ -f "$elf_path" ]; then
    cp "$elf_path" "$ELF_DIR/${guest}.bin"
  fi
done < "$EXTRACT_OUT"

if [ -n "$ELF_DIR" ]; then
  python3 -I - "$ELF_DIR" <<'PY'
import hashlib
import os
import sys

elf_dir = sys.argv[1]
bins = sorted(f for f in os.listdir(elf_dir) if f.endswith(".bin"))
lines = []
for name in bins:
    h = hashlib.sha256(open(os.path.join(elf_dir, name), "rb").read()).hexdigest()
    lines.append(f"{h}  {name}\n")
with open(os.path.join(elf_dir, "SHA256SUMS"), "w", encoding="utf-8") as fh:
    fh.writelines(lines)
PY
  echo "repro-build: wrote ELFs and SHA256SUMS to ${ELF_DIR}" >&2
fi
