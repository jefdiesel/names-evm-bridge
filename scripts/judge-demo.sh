#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# judge-demo.sh: re-run the whole bridge on your own machine, from a clean clone, in one command.
#
#   scripts/judge-demo.sh [--keep] [--purge] [--work DIR] [--help]
#
# What it does, each as a numbered step with its command and PASS/FAIL:
#   prerequisites; fetch the LEZ 0.4 testnet kit at a pinned commit; build the kit's sequencer and
#   wallet, the lez-bridge CLI (token and bridge guests embedded), the outbound host (G2-lite and
#   G3 guests, executor only) and the vault; bring up a throwaway LEZ v0.4.0-rc3 chain with the kit;
#   start a local Ethereum node (anvil); deploy the token and bridge programs and the vault;
#   deposits; attest the deposit root; a relayed PUBLIC claim into a never-used account that holds
#   no LEZ (the relay's payer co-signs as fee payer); that account's balances; burns; the outbound
#   updates; releases on Ethereum; final balances reconciled on both sides. A summary table at the
#   end; exit status 1 if any step failed.
#
# Built on the existing scripts, not new bridge logic: lez-bridge/scripts/e2e-roundtrip.sh (run in
# phases with STOP_AFTER) and public-demo/relayer/relayer.py (started the way
# public-demo/relayer/e2e-relay.sh starts it). The never-used account's deposit, relayed claim,
# burn and release use the same CLI and outbound-host commands as those scripts.
#
# Labels: every step that is weaker than its production form says so in its output. The wording
# follows the maintainer's statement of what the bridge demo shows (BRIDGE-DEMO-SHOWS.md, a
# maintainer note, not in this repository) and proposal/RISKS-AND-GAPS; change a label only when
# that statement changes.
#
# Options:
#   --keep      leave the chain, anvil and the relay running and keep the run directory
#   --rapidsnark-from-source   (Linux x86-64 CPUs without ADX/BMI2 only) build rapidsnark v0.0.8 here from
#               source WITHOUT assembly and link the kit's sequencer and wallet against it, instead of
#               refusing in step 1; needs cmake and m4; same results, slower field arithmetic
#   --purge     also delete the build directory (WORK/build: the kit checkout and three target
#               directories, about 12 GB) at the end
#   --work DIR  where everything goes (default: run/judge-demo in this checkout; run/ is ignored by git)
#
# Environment (all optional):
#   LEZ_PORT (6440; the kit also takes +1, +5, +6), ANVIL_PORT (6450), RELAY_PORT (6452),
#   BLOCK_TIME (5 s), CARGO_BUILD_JOBS (4),
#   BUILD_WRAP   a command prefix for every cargo build (default "nice -n 10")
#   RUN_WRAP     a command prefix for the outbound host's executor runs (default "env")
#   KIT_URL, KIT_COMMIT   the testnet kit (default: the public branch testnet-0.4 at the pinned commit)
#   SEQ_BIN, WALLET_BIN, LEZ_BRIDGE_BIN, OUTBOUND_HOST_BIN   prebuilt binaries instead of building
#                (the transcript then says "prebuilt, not built by this run")
#
# Nothing here proves: every zkVM guest is executed, not proved (RISC Zero's executor), and the
# vault checks seals with its TEST MockVerifier. No key, host or account of anyone else is used:
# the chain's genesis uses LEZ's published test keys and anvil's default accounts, every other key
# is made fresh in the run directory and deleted with it.
set -uo pipefail
export LC_ALL=C

REPO=$(cd "$(dirname "$0")/.." && pwd)
KEEP=0; PURGE=0; WORK=${WORK:-}; RS_FROM_SOURCE=${RAPIDSNARK_FROM_SOURCE:-0}
usage() { sed -n '3,45p' "$0" | sed 's/^# \{0,1\}//'; }
while [ $# -gt 0 ]; do
  case $1 in
    --keep) KEEP=1; shift ;;
    --purge) PURGE=1; shift ;;
    --rapidsnark-from-source) RS_FROM_SOURCE=1; shift ;;
    --work) WORK=${2:?--work needs a directory}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done
WORK=${WORK:-$REPO/run/judge-demo}
mkdir -p "$WORK"; WORK=$(cd "$WORK" && pwd)
KIT_URL=${KIT_URL:-https://github.com/jefdiesel/logos-execution-zone.git}
KIT_COMMIT=${KIT_COMMIT:-96a0a50232203ceae0cb8e69ef564d2d4a5d34a1}   # branch testnet-0.4: LEZ v0.4.0-rc3 + the kit's patches
# --rapidsnark-from-source only: the release the kit's rust-rapidsnark downloads (its RAPIDSNARK_VERSION, 0.0.8)
RAPIDSNARK_TAG=v0.0.8; RAPIDSNARK_COMMIT=2921b0eb33369f4d329a6902e97959246b802e6c
LEZ_PORT=${LEZ_PORT:-6440}; ANVIL_PORT=${ANVIL_PORT:-6450}; RELAY_PORT=${RELAY_PORT:-6452}
BLOCK_TIME=${BLOCK_TIME:-5}
JOBS=${CARGO_BUILD_JOBS:-4}
BUILD_WRAP=${BUILD_WRAP:-nice -n 10}
RUN_WRAP=${RUN_WRAP:-env}
STAMP=$(date +%Y%m%d-%H%M%S)
B=$WORK/build                 # kept between runs (builds are cached there)
RUN=$WORK/run-$STAMP          # this run: chain, wallets, keys, logs; removed at the end unless --keep
R=$RUN/roundtrip              # e2e-roundtrip.sh's run directory
TRANSCRIPT=$WORK/judge-demo-$STAMP.log
export PATH="$HOME/.foundry/bin:$HOME/.risc0/bin:$HOME/.cargo/bin:$PATH"
export LEE_WALLET_PASSWORD=   # the throwaway wallets have no password
SEQ_URL=http://127.0.0.1:$LEZ_PORT
ETH_RPC=http://127.0.0.1:$ANVIL_PORT
ETH=0x0000000000000000000000000000000000000000
D_ETH=10000000000000000         # 0.01 ETH, the vault's ETH denomination 0 (RoundTripDeploy.s.sol)
DF_ETH=100000000000000          # its deposit fee (1%)
WF_ETH=50000000000000           # its withdraw fee (0.5%)
XMAX_ETH=100000000000000        # max relay fee written into the exit commitment
# anvil's default account 0 (a well-known public test key; never holds real funds): the never-used
# account's depositor on Ethereum and the submitter of its release. The round trip uses account 3.
ANVIL0=$("$REPO/lez-bridge/scripts/anvil-dev-key.sh" 0) || exit 1
T_START=$(date +%s)

mkdir -p "$RUN"
chmod 700 "$RUN"
# Everything goes to the transcript in full and to this terminal (or CI log) line by line. The
# terminal copy shortens lines over 2000 characters (the wallet prints whole transactions; a
# program deploy segment is a line of several hundred kilobytes, after which GitHub Actions showed
# no more of the job's output). If the terminal or log reader goes away, the transcript still gets
# every line and no command of the demo is killed by a closed pipe.
exec > >(python3 -I -c '
import os, signal, sys
for s in (signal.SIGINT, signal.SIGTERM):     # the script handles these and still prints its summary;
    signal.signal(s, signal.SIG_IGN)          # this filter ends when the script closes its output
log, out, lim = open(sys.argv[1], "ab", buffering=0), sys.stdout.buffer, 2000
for line in iter(sys.stdin.buffer.readline, b""):
    log.write(line)
    if out is None:
        continue
    if len(line) > lim:
        line = line[:lim] + b" ... (%d more characters in the transcript)\n" % (len(line) - lim)
    try:
        out.write(line); out.flush()
    except OSError:                              # the reader went away: keep the transcript going
        out = None
        os.dup2(os.open(os.devnull, os.O_WRONLY), 1)
' "$TRANSCRIPT") 2>&1

# ------------------------------------------------------------------------------------- labels
L_CHAIN="throwaway LEZ 0.4 (v0.4.0-rc3) chain you run yourself: one standalone sequencer, no Logos Blockchain anchoring, LEZ's published test genesis keys; not a Logos network, not the LEZ 0.4 bridge test network"
L_ETH="local Ethereum node (anvil), not Sepolia"
L_VAULT="vault with the TEST MockVerifier: mock-verified (it accepts a seal that is imageId | sha256(journal)); no real verifier, no Sepolia"
L_ROOT="operator-attested: the demo operator posts the vault root (here from a local anvil vault). Not the trustless path: the RFP-021 light client was proved on local chains only (rc3 and rc1; about 2 h per update), and does not run in this demo"
L_RELAY="the claimer signs, the bridge's relay co-signs as fee payer; the relay is a demo-network convenience, not part of the trust-minimised design"
L_BURN="public burns on the throwaway chain; the never-used account pays its own burn fee from a top-up by the demo operator (the relay co-signs claims only)"
L_OUT="Simulated: proofs executed, not proved, and LEZ finality trusted (G2-lite TrustedTip and G3 guests run in RISC Zero's executor; the vault's MockVerifier accepts their mock seals)"
L_PRIV="private claims take about 10 minutes of proving each (done separately, see public-demo/prove/), so this demo makes public claims only"

# ------------------------------------------------------------------------------------- steps
N=0; CUR=""; T0=0; FAILED=0
S_NAME=(); S_RES=(); S_LABEL=(); S_SECS=()
hr() { echo "------------------------------------------------------------------------------------------"; }
step() { N=$((N + 1)); CUR=$1; CURLABEL=${2:-}; T0=$(date +%s); LAST_OUT=""; echo; hr; echo "STEP $N. $CUR"; [ -z "$CURLABEL" ] || echo "  label: $CURLABEL"; hr; }
record() { S_NAME+=("$N. $CUR"); S_RES+=("$1"); S_LABEL+=("${CURLABEL:--}"); S_SECS+=("$(( $(date +%s) - T0 ))"); }
pass() { echo "PASS: step $N. $CUR"; record PASS; }
# LAST_OUT: the file holding the output of the step's last command that wrote to one (never a file
# with a key or recovery phrase); a FAIL prints its last lines next to the FAIL line.
LAST_OUT=""
fail() {
  echo "FAIL: step $N. $CUR: $*"
  if [ -n "$LAST_OUT" ] && [ -s "$LAST_OUT" ]; then
    echo "  last lines of $LAST_OUT:"; tail -n 20 "$LAST_OUT" | cut -c1-300 | sed 's/^/    /'
  fi
  record FAIL; FAILED=1; finish
}
show() { echo "\$ $*"; }                  # print a command as it runs (secrets are never on these lines)
run() { show "$@"; "$@"; }
note() { echo "  $*"; }

summary() {
  echo; hr; echo "SUMMARY ($(date '+%F %T'), wall clock $(( ($(date +%s) - T_START) / 60 )) min)"; hr
  local i
  for i in "${!S_NAME[@]}"; do
    printf '%-4s  %5ss  %s\n' "${S_RES[$i]}" "${S_SECS[$i]}" "${S_NAME[$i]}"
    printf '               label: %s\n' "${S_LABEL[$i]}"
  done
  [ "$FAILED" = 0 ] && echo "ALL STEPS PASSED" || echo "A STEP FAILED (see FAIL above); later steps did not run"
  echo "Not shown by this demo: private claims ($L_PRIV)."
  echo "Not shown by this demo: the trustless deposit root (RFP-021 light client) and any real proof; Sepolia; the deposit desk (the round-trip vault has none)."
  echo "Transcript: $TRANSCRIPT"
}

ANVIL_PID=""; RELAY_PID=""
cleanup() {
  if [ "$KEEP" = 1 ]; then
    echo; echo "--keep: still running: chain (kit home $RUN/kit), anvil ${ANVIL_PID:+pid $ANVIL_PID}, relay ${RELAY_PID:+pid $RELAY_PID}"
    echo "stop later: kill $ANVIL_PID $RELAY_PID; $B/kit/testnet/down.sh --home $RUN/kit; rm -rf $RUN"
    return
  fi
  echo; echo "cleanup: stopping the relay, anvil and the chain, deleting $RUN"
  [ -n "$RELAY_PID" ] && kill "$RELAY_PID" 2>/dev/null
  [ -n "$ANVIL_PID" ] && kill "$ANVIL_PID" 2>/dev/null
  [ -d "$RUN/kit" ] && "$B/kit/testnet/down.sh" --home "$RUN/kit" >/dev/null 2>&1
  sleep 1
  rm -rf "$RUN"
  [ "$PURGE" = 1 ] && { echo "cleanup: --purge, deleting $B"; rm -rf "$B"; }
}
DONE_FINISH=0
finish() {
  [ "$DONE_FINISH" = 1 ] && return; DONE_FINISH=1
  summary; cleanup; scrub_transcript
  exit "$FAILED"
}
on_signal() {
  echo "interrupted"
  if [ "$N" -gt "${#S_NAME[@]}" ]; then fail "interrupted (INT or TERM)"; else FAILED=1; finish; fi
}
trap on_signal INT TERM
# Any other way out (an unset variable under set -u, a shell error) still prints a FAIL line, the
# tail of the step's output and the summary.
on_exit() {
  local rc=$?
  [ "$DONE_FINISH" = 1 ] && return
  [ "$N" -gt "${#S_NAME[@]}" ] || { N=$((N + 1)); CUR="(after the last step)"; }
  fail "the script stopped unexpectedly (exit status $rc)"
}
trap on_exit EXIT

# A copy of the transcript safe to publish: this machine's paths, user and host name replaced, and
# public keys printed by the wallet shortened.
scrub_transcript() {
  sleep 1
  local pre=""; local b
  for b in "${SEQ_BIN:-}" "${WALLET_BIN:-}" "${LEZ_BRIDGE_BIN:-}" "${OUTBOUND_HOST_BIN:-}"; do
    case "$b" in ""|"$B"/*) ;; *) pre="$pre:$(dirname "$b")" ;; esac
  done
  JD_BUILD_WRAP=$BUILD_WRAP JD_RUN_WRAP=$RUN_WRAP python3 - "$TRANSCRIPT" "${TRANSCRIPT%.log}.public.log" "$REPO" "$WORK" "$HOME" "$(id -un)" "$(hostname -s 2>/dev/null || hostname)" "$pre" <<'PY'
import os, re, sys
src, dst, repo, work, home, user, host, pre = sys.argv[1:]
t = open(src, errors="replace").read()
for p in sorted({p for p in pre.split(":") if len(p) > 1}, key=len, reverse=True):
    t = t.replace(p, "<prebuilt>")
for w in (os.environ.get("JD_BUILD_WRAP", ""), os.environ.get("JD_RUN_WRAP", "")):
    if "/" in w:                       # a wrapper given by path (e.g. a local build lock)
        t = t.replace(w, "<build wrapper>")
for a, b in ((work, "<work>"), (repo, "<repo>"), (home, "~")):
    if a and len(a) > 1:
        t = t.replace(a, b)
for w, b in ((host, "<host>"), (user, "<user>")):
    if w and len(w) > 2:
        t = re.sub(r"\b%s\b" % re.escape(w), b, t)
t = re.sub(r"\b(pk|With pk) [0-9a-fA-F]{64}\b", r"\1 <public key>", t)
keyword = re.compile(r"priv(?:ate)?.?key|secret|seed|mnemonic|signing.?key|\bsk\b|\bpk\b", re.I)
t = "\n".join(re.sub(r"(?:0x)?[0-9a-fA-F]{64}(?![0-9a-fA-F])", "<hex>", l) if keyword.search(l) else l for l in t.split("\n"))
t = re.sub(r"\x1b\[[0-9;]*m", "", t)
# the wallet prints whole transactions; keep the first 160 characters of those lines
t = re.sub(r"(?m)^(Transaction data is .{0,160}).*$", r"\1 …", t)
open(dst, "w").write(t)
PY
  echo "public copy of the transcript (paths, user and host replaced): ${TRANSCRIPT%.log}.public.log"
}

port_free() { python3 -c 'import socket,sys; s=socket.socket(); s.settimeout(0.5); sys.exit(0 if s.connect_ex(("127.0.0.1", int(sys.argv[1]))) else 1)' "$1"; }
idof() { grep -oE 'Public/[1-9A-HJ-NP-Za-km-z]{32,44}' | sed -n 1p | cut -d/ -f2; }
pkof() { sed -n 's/^With pk \([0-9a-fA-F]\{64\}\).*/\1/p' | sed -n 1p; }
rpc() { curl -s -m 10 -X POST "$SEQ_URL/" -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}"; }
acct() { rpc getAccount "[\"$1\"]" | python3 -c '
import json,sys
r=json.load(sys.stdin).get("result")
if r is None: print("ABSENT (never used)")
else:
    raw=((r.get("data") or {}).get("actor_states") or {}).get("11111111111111111111111111111111") or []
    print("PRESENT nonce=%s native=%d" % (r.get("nonce"), int.from_bytes(bytes(raw),"little")))'; }
native() { acct "$1" | sed -n 's/.*native=\([0-9]*\).*/\1/p'; }
newsecret() { ( umask 077; python3 -c 'import secrets; print(secrets.token_hex(32))' > "$1" ); }
ebal() { cast balance "$1" --rpc-url "$ETH_RPC"; }

echo "judge-demo $(date '+%F %T')  repo $(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo '?')  $(uname -sm)"
echo "work $WORK  run $RUN"

# ===================================================================================== 0
step "Prerequisites"
missing=0
need() {
  command -v "$1" >/dev/null 2>&1 && return 0
  local d
  for d in /opt/homebrew/bin /usr/local/bin "$HOME/.local/bin"; do
    if [ -x "$d/$1" ]; then
      echo "  not on PATH: $1 is installed in $d    fix: export PATH=\"$d:\$PATH\" (a non-interactive ssh shell often lacks it), then run this again"
      missing=1; return 1
    fi
  done
  echo "  missing: $1    install: $2"; missing=1
}
OS=$(uname -s); ARCH=$(uname -m)
case "$OS/$ARCH" in Linux/x86_64|Linux/aarch64|Darwin/arm64) note "platform $OS/$ARCH" ;; *) echo "  unsupported platform $OS/$ARCH (Linux x86_64/aarch64 or macOS arm64)"; missing=1 ;; esac
# The kit's sequencer and wallet link iden3's prebuilt rapidsnark (linux-x86_64, fetched by the
# rust-rapidsnark crate) whose field arithmetic uses ADX and BMI2 instructions: on a CPU without
# them both binaries stop at once with "Illegal instruction" (seen on a Xeon E5-2680 v3). Whether built
# here or given as SEQ_BIN/WALLET_BIN, they need these CPU flags, so check before hours of building.
NOADX=0
if [ "$OS/$ARCH" = Linux/x86_64 ]; then
  for f in adx bmi2; do
    grep -qw "$f" /proc/cpuinfo 2>/dev/null || { echo "  this CPU lacks the $f instructions ($(sed -n 's/^model name[[:space:]]*: //p' /proc/cpuinfo | sed -n 1p)): the LEZ kit's sequencer and wallet link upstream's prebuilt rapidsnark library (a Logos node dependency, not this repository's code), which needs ADX and BMI2, and stop with \"Illegal instruction\" here. Run on an x86-64 CPU that lists adx and bmi2 in /proc/cpuinfo, or on an arm64 machine; or rerun with --rapidsnark-from-source to build rapidsnark here without assembly (needs cmake and m4)"; [ "$RS_FROM_SOURCE" = 1 ] || missing=1; NOADX=1; }
  done
fi
PORTABLE_RS=0
if [ "$NOADX" = 1 ] && [ "$RS_FROM_SOURCE" = 1 ]; then
  PORTABLE_RS=1
  note "--rapidsnark-from-source: step 3 builds rapidsnark $RAPIDSNARK_TAG from source WITHOUT assembly on this machine and links the kit's sequencer and wallet against it"
  need cmake "sudo apt install cmake"
  need m4 "sudo apt install m4   (GMP's configure needs it)"
  need g++ "sudo apt install build-essential"
elif [ "$RS_FROM_SOURCE" = 1 ]; then
  note "--rapidsnark-from-source ignored: this machine runs the prebuilt rapidsnark"
fi
need git "sudo apt install git / xcode-select --install"
need curl "sudo apt install curl / brew install curl"
need python3 "sudo apt install python3 / xcode-select --install"
need shasum "sudo apt install perl (shasum ships with perl) / preinstalled on macOS"
BUILDING=0
[ -n "${SEQ_BIN:-}" ] && [ -n "${WALLET_BIN:-}" ] && [ -n "${LEZ_BRIDGE_BIN:-}" ] && [ -n "${OUTBOUND_HOST_BIN:-}" ] || BUILDING=1
if [ "$BUILDING" = 1 ]; then
  need cargo "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   (each rust-toolchain.toml then installs Rust 1.98.1)"
  need rustup "as for cargo"
  if command -v cargo >/dev/null 2>&1 && ! cargo --version >/dev/null 2>&1; then
    echo "  cargo is on PATH but does not run: $(cargo --version 2>&1 | sed -n 1p)    fix: rustup default stable (or install rustup as above)"; missing=1
  fi
  need clang "sudo apt install build-essential clang libclang-dev / xcode-select --install"
  need pkg-config "sudo apt install pkg-config / brew install pkg-config"
  if [ "$OS" = Linux ]; then
    pkg-config --exists libpcsclite 2>/dev/null || { echo "  missing: libpcsclite    install: sudo apt install libpcsclite-dev"; missing=1; }
    pkg-config --exists openssl 2>/dev/null || { echo "  missing: OpenSSL headers    install: sudo apt install libssl-dev"; missing=1; }
  fi
  RZ=$(command -v rzup || echo "")
  if [ -z "$RZ" ]; then
    echo "  missing: rzup (RISC Zero's toolchain manager; builds the zkVM guests)    install: cargo install rzup --version 0.5.0 --locked && rzup install rust 1.97.0 && rzup install cargo-risczero 3.0.5"; missing=1
  else
    ls -d "$HOME"/.risc0/toolchains/*rust* >/dev/null 2>&1 || { echo "  missing: the RISC Zero Rust toolchain    install: rzup install rust 1.97.0"; missing=1; }
    ls -d "$HOME"/.risc0/extensions/v3.0.5-cargo-risczero* >/dev/null 2>&1 || { echo "  missing: cargo-risczero 3.0.5    install: rzup install cargo-risczero 3.0.5"; missing=1; }
  fi
fi
FOUNDRY_HINT="curl -L https://foundry.paradigm.xyz | bash && foundryup --install v1.5.1   (or a release archive from github.com/foundry-rs/foundry/releases)"
need forge "$FOUNDRY_HINT"; need cast "$FOUNDRY_HINT"; need anvil "$FOUNDRY_HINT"
for p in $LEZ_PORT $((LEZ_PORT + 1)) $((LEZ_PORT + 5)) $((LEZ_PORT + 6)) $ANVIL_PORT $RELAY_PORT; do
  port_free "$p" || { echo "  port $p is in use: set LEZ_PORT / ANVIL_PORT / RELAY_PORT to free ones"; missing=1; }
done
free_gb=$(df -Pk "$WORK" | awk 'NR==2 {print int($4 / 1048576)}')
need_gb=3; [ "$BUILDING" = 1 ] && [ ! -d "$B/target-kit" ] && need_gb=25
[ "$free_gb" -ge "$need_gb" ] || { echo "  only $free_gb GB free in $WORK; a first build needs about $need_gb GB (set --work to a bigger disk)"; missing=1; }
note "disk: $free_gb GB free in $WORK (the builds take about 12 GB; keep 25 GB free for the first run)"
if [ "$OS" = Darwin ]; then mem_gb=$(( $(sysctl -n hw.memsize) / 1073741824 )); else mem_gb=$(( $(awk '/MemTotal/ {print $2}' /proc/meminfo) / 1048576 )); fi
note "memory: $mem_gb GB (8 GB or more recommended for the builds)"
for t in git cargo forge anvil python3 rzup; do command -v $t >/dev/null 2>&1 && note "$t: $($t --version 2>&1 | sed -n 1p)"; done
[ "$missing" = 0 ] || fail "install what is listed above, then run this again"
pass

# ===================================================================================== 1
step "Fetch the LEZ 0.4 testnet kit (public branch testnet-0.4, pinned commit ${KIT_COMMIT:0:8})"
mkdir -p "$B"
if [ ! -d "$B/kit/.git" ]; then
  run git init -q "$B/kit" && run git -C "$B/kit" remote add origin "$KIT_URL" || fail "git init"
fi
if [ "$(git -C "$B/kit" rev-parse HEAD 2>/dev/null)" != "$KIT_COMMIT" ]; then
  run git -C "$B/kit" fetch -q --depth 1 origin "$KIT_COMMIT" || fail "fetch $KIT_URL $KIT_COMMIT"
  run git -C "$B/kit" -c advice.detachedHead=false checkout -q FETCH_HEAD || fail "checkout"
fi
got=$(git -C "$B/kit" rev-parse HEAD)
[ "$got" = "$KIT_COMMIT" ] || fail "kit is at $got, want $KIT_COMMIT"
note "kit at $got"
pass

# ===================================================================================== 2
step "Build the kit's sequencer and wallet (LEZ v0.4.0-rc3 + kit patches; cached after the first run)"
if [ -n "${SEQ_BIN:-}" ] && [ -n "${WALLET_BIN:-}" ]; then
  CURLABEL="prebuilt, not built by this run: $SEQ_BIN, $WALLET_BIN"
else
  KITKEY=$KIT_COMMIT
  if [ "$PORTABLE_RS" = 1 ]; then
    CURLABEL="rapidsnark built from source WITHOUT assembly on this machine (--rapidsnark-from-source: this CPU lacks ADX/BMI2, which the prebuilt library needs); same results, slower field arithmetic"
    RS=$B/rapidsnark-noasm
    if [ ! -f "$RS/librapidsnark.a" ]; then
      if [ ! -d "$B/rapidsnark-src/.git" ]; then
        run git clone -q --depth 1 --branch "$RAPIDSNARK_TAG" --recurse-submodules --shallow-submodules https://github.com/iden3/rapidsnark "$B/rapidsnark-src" \
          || fail "clone rapidsnark"
      fi
      [ "$(git -C "$B/rapidsnark-src" rev-parse HEAD)" = "$RAPIDSNARK_COMMIT" ] || fail "rapidsnark is not at $RAPIDSNARK_TAG ($RAPIDSNARK_COMMIT)"
      show "(cd $B/rapidsnark-src && CC=\"cc -std=gnu17\" ./build_gmp.sh host_noasm && CXXFLAGS=\"-include cstdint\" make host_noasm)    (GMP with --disable-assembly, rapidsnark with USE_ASM=NO)"
      # GMP 6.3.0's configure rejects GCC 15's default C23 ("could not find a working compiler"): build it as gnu17
      # shellcheck disable=SC2086  # BUILD_WRAP is a command prefix
      # rapidsnark 0.0.8's binfile_utils.hpp uses uint32_t without <cstdint> (GCC 15 no longer pulls it in)
      (cd "$B/rapidsnark-src" && { [ -d depends/gmp/package ] || CC="${CC:-cc} -std=gnu17" $BUILD_WRAP ./build_gmp.sh host_noasm; } \
        && CXXFLAGS="${CXXFLAGS:-} -include cstdint" $BUILD_WRAP make host_noasm) > "$B/build-rapidsnark.log" 2>&1 \
        || { tail -20 "$B/build-rapidsnark.log"; fail "rapidsnark build (log $B/build-rapidsnark.log)"; }
      mkdir -p "$RS"
      for l in librapidsnark.a libfr.a libfq.a libgmp.a; do cp "$B/rapidsnark-src/package_noasm/lib/$l" "$RS/" || fail "rapidsnark: no $l"; done
    fi
    export RAPIDSNARK_LIB_DIR=$RS
    KITKEY=$KIT_COMMIT-rapidsnark-noasm
    note "rapidsnark: built from source without assembly here; RAPIDSNARK_LIB_DIR=$RS"
  fi
  show "P=$B/kit-bin CARGO_TARGET_DIR=$B/target-kit LEZ_BUILD_WRAP=\"$BUILD_WRAP\" $B/kit/testnet/sequencer/build.sh"
  P=$B/kit-bin CARGO_TARGET_DIR=$B/target-kit LEZ_BUILD_KEY=$KITKEY LEZ_BUILD_WRAP="$BUILD_WRAP" CARGO_BUILD_JOBS=$JOBS \
    "$B/kit/testnet/sequencer/build.sh" || fail "kit build (logs in $B/kit-bin/logs)"
  SEQ_BIN=$B/kit-bin/bin/sequencer_service; WALLET_BIN=$B/kit-bin/bin/wallet
fi
[ -x "$SEQ_BIN" ] && [ -x "$WALLET_BIN" ] || fail "no sequencer/wallet binary"
for b in "$SEQ_BIN" "$WALLET_BIN"; do
  "$b" --help >/dev/null 2>&1
  [ $? = 132 ] && fail "$b stops with \"Illegal instruction\" on this CPU"
done
pass

# ===================================================================================== 3
step "Build the bridge: lez-bridge CLI, outbound host (executor only), vault"
if [ -n "${LEZ_BRIDGE_BIN:-}" ]; then CURLABEL="prebuilt, not built by this run: $LEZ_BRIDGE_BIN"
else
  show "(cd lez-bridge && CARGO_TARGET_DIR=$B/target-bridge $BUILD_WRAP cargo build --release -j $JOBS -p lez_bridge_cli)"
  # shellcheck disable=SC2086  # BUILD_WRAP is a command prefix, split on purpose
  (cd "$REPO/lez-bridge" && CARGO_TARGET_DIR=$B/target-bridge $BUILD_WRAP cargo build --release -j "$JOBS" -p lez_bridge_cli) > "$B/build-cli.log" 2>&1 \
    || { tail -30 "$B/build-cli.log"; fail "CLI build (log $B/build-cli.log)"; }
  LEZ_BRIDGE_BIN=$B/target-bridge/release/lez-bridge
fi
if [ -n "${OUTBOUND_HOST_BIN:-}" ]; then CURLABEL="${CURLABEL:+$CURLABEL; }prebuilt, not built by this run: $OUTBOUND_HOST_BIN"
else
  show "(cd lez-bridge/outbound && CARGO_TARGET_DIR=$B/target-outbound $BUILD_WRAP cargo build --release -j $JOBS -p outbound_host)"
  # shellcheck disable=SC2086
  (cd "$REPO/lez-bridge/outbound" && CARGO_TARGET_DIR=$B/target-outbound $BUILD_WRAP cargo build --release -j "$JOBS" -p outbound_host) > "$B/build-outbound.log" 2>&1 \
    || { tail -30 "$B/build-outbound.log"; fail "outbound host build (log $B/build-outbound.log)"; }
  OUTBOUND_HOST_BIN=$B/target-outbound/release/outbound_host
fi
if [ ! -f "$REPO/vault/lib/forge-std/src/Test.sol" ] || [ ! -d "$REPO/vault/lib/openzeppelin-contracts/contracts" ]; then
  run git -C "$REPO" submodule update --init --recursive vault/lib/forge-std vault/lib/openzeppelin-contracts || fail "vault submodules"
fi
show "(cd vault && forge build)"
(cd "$REPO/vault" && forge build) > "$B/build-vault.log" 2>&1 || { tail -30 "$B/build-vault.log"; fail "forge build"; }
CLI=$LEZ_BRIDGE_BIN; HOST=$OUTBOUND_HOST_BIN
note "CLI $(shasum -a 256 "$CLI" | cut -c1-16)…  outbound host $(shasum -a 256 "$HOST" | cut -c1-16)…"
run "$HOST" ids || fail "outbound host ids"
pass

# ===================================================================================== 4
step "Bring up a throwaway LEZ rc3 chain with the kit" "$L_CHAIN"
show "SEQ_BIN=… WALLET_BIN=… $B/kit/testnet/up.sh --genesis-key default --port $LEZ_PORT --block-time $BLOCK_TIME --home $RUN/kit"
SEQ_BIN=$SEQ_BIN WALLET_BIN=$WALLET_BIN "$B/kit/testnet/up.sh" --genesis-key default --port "$LEZ_PORT" \
  --block-time "$BLOCK_TIME" --home "$RUN/kit" || fail "up.sh"
pass

# ===================================================================================== 5
step "Start a local Ethereum node (anvil)" "$L_ETH"
show "anvil --host 127.0.0.1 --port $ANVIL_PORT --silent &"
anvil --host 127.0.0.1 --port "$ANVIL_PORT" --silent > "$RUN/anvil.log" 2>&1 < /dev/null &
ANVIL_PID=$!
for _ in $(seq 1 30); do cast chain-id --rpc-url "$ETH_RPC" >/dev/null 2>&1 && break; sleep 1; done
cid=$(cast chain-id --rpc-url "$ETH_RPC" 2>/dev/null) || fail "anvil did not answer"
note "anvil pid $ANVIL_PID, chain id $cid"
pass

# The round trip, in phases. Its own output carries its label on every OK/FAILED line.
roundtrip() { # $1: the step to stop after ('' = to the end)
  show "R=$R RESUME=1 STOP_AFTER=$1 SEQ=$SEQ_URL ETH_RPC=$ETH_RPC lez-bridge/scripts/e2e-roundtrip.sh"
  LAST_OUT=$R/roundtrip.log
  R=$R RESUME=1 STOP_AFTER=$1 VAULT_DIR=$REPO/vault TGT=$B/target-outbound HOST=$HOST CLI_SRC=$CLI \
    SEQ=$SEQ_URL ETH_RPC=$ETH_RPC LOCK=$RUN_WRAP TMPDIR_OUTBOUND=$RUN/tmp bash "$REPO/lez-bridge/scripts/e2e-roundtrip.sh"
}
rt_done() { grep -qx "$1" "$R/steps.done" 2>/dev/null; }
ids() { . "$R/ids.env"; }

# ===================================================================================== 6
step "LEZ: fund the demo account P from the genesis account, deploy the token and bridge programs" "P is the demo operator: bridge admin, freezer and root attestor"
mkdir -p "$R/wallet" "$RUN/tmp"; chmod 700 "$R/wallet"
# Create the round trip's wallet first, so its recovery phrase goes to a file, not the transcript.
LEE_WALLET_HOME_DIR=$R/wallet LEZ_BRIDGE_SEQUENCER=$SEQ_URL "$CLI" wallet check-health > "$R/wallet/first-run.txt" 2>&1
chmod 600 "$R/wallet/first-run.txt"
roundtrip programs && rt_done programs || fail "round trip: wallet, programs"
pass

# ===================================================================================== 7
step "Ethereum: G2 genesis state, then deploy the vault" "$L_VAULT. The round-trip deploy has no deposit desk, so none is deployed"
roundtrip vault && rt_done vault || fail "round trip: genesis, vault"
pass

# ===================================================================================== 8
step "LEZ: wrapped USDC and ETH (minted only by the bridge), bridge init bound to the vault, token registry"
roundtrip setup && rt_done setup || fail "round trip: setup"
pass

# ===================================================================================== 9
step "Ethereum: deposits (six for P, one for a never-used LEZ account X)"
roundtrip deposits && rt_done deposits || fail "round trip: deposits"
ids
XW=$R/wallet-x; mkdir -p "$XW" "$RUN/x"; chmod 700 "$XW" "$RUN/x"
xcli() { LEE_WALLET_HOME_DIR=$XW LEZ_BRIDGE_SEQUENCER=$SEQ_URL "$CLI" "$@"; }
xcli wallet check-health > "$XW/first-run.txt" 2>&1; chmod 600 "$XW/first-run.txt"
show "lez-bridge wallet account new public    (X's own wallet; the key never leaves it)"
LAST_OUT=$RUN/x/new.out; xcli wallet account new public > "$LAST_OUT" 2>&1 || fail "new account"
X=$(idof < "$RUN/x/new.out"); XPK=$(pkof < "$RUN/x/new.out")
[ -n "$X" ] && [ ${#XPK} = 64 ] || fail "could not create X"
note "X = $X: $(acct "$X")"
newsecret "$RUN/x/deposit.secret"
show "lez-bridge commitment --secret-file <X's deposit secret, mode 600> --dest $X --eth-token $ETH --denom $D_ETH --cfg-id $CFG_ID"
XC=$("$CLI" commitment --secret-file "$RUN/x/deposit.secret" --dest "$X" --eth-token $ETH --denom $D_ETH --cfg-id "$CFG_ID" | sed -n 1p)
grep -qE '^0x[0-9a-f]{64}$' <<<"$XC" || fail "commitment"
show "cast send $VAULT 'depositETH(uint32,bytes32)' 0 $XC --value $((D_ETH + DF_ETH))   (anvil account 0)"
LAST_OUT=$RUN/x/deposit.json
cast send "$VAULT" "depositETH(uint32,bytes32)" 0 "$XC" --value $((D_ETH + DF_ETH)) --private-key $ANVIL0 --rpc-url "$ETH_RPC" --json > "$RUN/x/deposit.json" \
  || fail "X's deposit"
note "X's deposit tx $(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["transactionHash"])' "$RUN/x/deposit.json"); vault depositCount $(cast call "$VAULT" 'depositCount()(uint64)' --rpc-url "$ETH_RPC" 2>/dev/null || echo '?')"
pass

# ===================================================================================== 10
step "LEZ: attest the deposit root, then P's six public claims and a refused repeat" "$L_ROOT"
roundtrip claims && rt_done claims || fail "round trip: attest, claims"
XLEAF=$(cast call "$VAULT" "depositLeaf(address,uint256,bytes32)(bytes32)" $ETH $D_ETH "$XC" --rpc-url "$ETH_RPC")
XIDX=$(( $(grep -nix "$XLEAF" "$R/leaves.txt" | cut -d: -f1) - 1 ))
[ "$XIDX" -ge 0 ] || fail "X's leaf is not under the attested root"
note "X's deposit is leaf $XIDX of $(wc -l < "$R/leaves.txt" | tr -d ' ') under the attested root"
pass

# ===================================================================================== 11
step "LEZ: relayed PUBLIC claim into X, which was never used and holds no LEZ" "$L_RELAY"
RW=$R/wallet-relay; mkdir -p "$RW" "$RUN/relay"; chmod 700 "$RW"
rcli() { LEE_WALLET_HOME_DIR=$RW LEZ_BRIDGE_SEQUENCER=$SEQ_URL "$CLI" "$@"; }
pcli() { LEE_WALLET_HOME_DIR=$R/wallet LEZ_BRIDGE_SEQUENCER=$SEQ_URL "$CLI" "$@"; }
rcli wallet check-health > "$RW/first-run.txt" 2>&1; chmod 600 "$RW/first-run.txt"
LAST_OUT=$RUN/relay/new.out; rcli wallet account new public > "$LAST_OUT" 2>&1 || fail "relay payer account"
RP=$(idof < "$RUN/relay/new.out"); RPK=$(pkof < "$RUN/relay/new.out")
note "relay payer $RP (a dedicated account: no bridge role)"
show "lez-bridge wallet auth-transfer send --from Public/$F --to-pk <relay payer key> --amount 100000000    (genesis account F, in P's wallet)"
pcli wallet auth-transfer send --from "Public/$F" --to-pk "$RPK" --amount 100000000 > "$RUN/relay/fund.out" 2>&1 || { cat "$RUN/relay/fund.out"; fail "fund the relay payer"; }
show "relayer.py --bind-payer    (a new payer, bound to this chain before it starts: it refuses an unbound payer)"
P=$RUN/relay CLI=$CLI RELAY_WALLET=$RW SEQUENCER=$SEQ_URL BRIDGE=$BP TOKEN_PROGRAM=$TP PAYER=$RP \
  RELAY_STATE=$RUN/relay/state.json RELAY_LOG=$RUN/relay/relayer.log python3 -I "$REPO/public-demo/relayer/relayer.py" --bind-payer \
  > "$RUN/relay/bind.out" 2>&1 || { cat "$RUN/relay/bind.out"; fail "bind the relay payer"; }
show "relayer.py on 127.0.0.1:$RELAY_PORT (public-demo/relayer/relayer.py; BRIDGE=$BP PAYER=$RP)"
P=$RUN/relay CLI=$CLI RELAY_WALLET=$RW SEQUENCER=$SEQ_URL RELAY_LISTEN=127.0.0.1:$RELAY_PORT BRIDGE=$BP \
  TOKEN_PROGRAM=$TP PAYER=$RP RELAY_STATE=$RUN/relay/state.json RELAY_LOG=$RUN/relay/relayer.log \
  nohup python3 -I "$REPO/public-demo/relayer/relayer.py" > "$RUN/relay/relayer.out" 2>&1 < /dev/null &
RELAY_PID=$!
RELAY=http://127.0.0.1:$RELAY_PORT/relay
for _ in $(seq 1 20); do curl -s -m 2 "$RELAY/info" >/dev/null 2>&1 && break; sleep 1; done
info=$(curl -s -m 5 "$RELAY/info") || { cat "$RUN/relay/relayer.out"; fail "the relay does not answer"; }
note "relay info: $info"
note "X before: $(acct "$X")"
XCLAIM=(claim --bridge "$BP" --token-program "$TP" --from "$X" --eth-token $ETH --denom-index 0 --denom $D_ETH
        --secret-file "$RUN/x/deposit.secret" --cfg-id "$CFG_ID" --leaves "$R/leaves.txt" --index "$XIDX")
p0=$(native "$RP")
show "lez-bridge claim --from $X … --secret-file <X's deposit secret> --index $XIDX --relay $RELAY    (signed in X's wallet)"
xcli "${XCLAIM[@]}" --relay "$RELAY" > "$RUN/x/claim.out" 2>&1; rc=$?
grep -vE '^Transaction data' "$RUN/x/claim.out" | cut -c1-200 | tail -8
[ "$rc" = 0 ] || fail "relayed claim exit $rc"
sleep 2
p1=$(native "$RP")
note "relay payer native LEZ $p0 -> $p1: the relay paid the claim's fee, $((p0 - p1))"
show "lez-bridge claim … --relay $RELAY    (the same deposit again)"
if xcli "${XCLAIM[@]}" --relay "$RELAY" > "$RUN/x/repeat.out" 2>&1; then cat "$RUN/x/repeat.out"; fail "the repeat claim went through"; fi
tail -1 "$RUN/x/repeat.out"; note "repeat refused (CLI pre-check), nothing sent"
pass

# ===================================================================================== 12
step "LEZ: X's balances after the relayed claim"
xa=$(acct "$X"); note "X on chain: $xa"
show "lez-bridge balances --token-program $TP    (X's wallet)"
LAST_OUT=$RUN/x/balances.out; xcli balances --token-program "$TP" > "$LAST_OUT" 2>&1
grep -E "^Public/$X" "$RUN/x/balances.out" | tee "$RUN/x/holding.txt"
xn=$(native "$X")
[ "$xn" = 0 ] || fail "X holds $xn native LEZ, expected 0"
[ "$(awk -v d="$DEF_ETH" '$2 == d {print $3}' "$RUN/x/holding.txt")" = "$D_ETH" ] || fail "X does not hold $D_ETH of the wrapped ETH ($DEF_ETH)"
note "X holds 0 native LEZ and $D_ETH units (0.01) of wrapped ETH"
pass

# ===================================================================================== 13
step "LEZ: burns (X burns its 0.01 wrapped ETH toward a fresh Ethereum address; P's first batch)" "$L_BURN"
XR=$(cast keccak "judge-demo-x-recipient-$STAMP" | cut -c1-42)
newsecret "$RUN/x/exit.secret"
XE=$("$CLI" exit-commitment --secret-file "$RUN/x/exit.secret" --eth-dest "$XR" --eth-token $ETH --denom $D_ETH \
  --max-relay-fee $XMAX_ETH --cfg-id "$CFG_ID" | sed -n 1p)
grep -qE '^0x[0-9a-f]{64}$' <<<"$XE" || fail "exit commitment"
note "X's Ethereum recipient $XR; exit commitment E = $XE (the recipient is inside E, not on LEZ)"
show "lez-bridge wallet auth-transfer send --from Public/$F --to-pk <X's public key> --amount 50000000    (the demo operator tops X up for its burn fee)"
pcli wallet auth-transfer send --from "Public/$F" --to-pk "$XPK" --amount 50000000 > "$RUN/x/topup.out" 2>&1 || { cat "$RUN/x/topup.out"; fail "top-up for X's burn fee"; }
x0=$(native "$X"); note "X native after the operator's top-up: $x0"
show "lez-bridge burn --from $X --definition $DEF_ETH --amount $D_ETH --exit $XE    (signed in X's wallet)"
xcli burn --bridge "$BP" --token-program "$TP" --from "$X" --definition "$DEF_ETH" --amount $D_ETH --exit "$XE" > "$RUN/x/burn.out" 2>&1 \
  || { cat "$RUN/x/burn.out"; fail "X's burn"; }
grep -vE '^Transaction data' "$RUN/x/burn.out" | cut -c1-200 | tail -4
sleep 2; x1=$(native "$X"); note "X paid its burn fee: $((x0 - x1))"
roundtrip burns1 && rt_done burns1 || fail "round trip: burns batch 1"
# From here the round trip's own checks (anonymity sets, privacy search) cover X's exit too.
echo "x $XE" >> "$R/exit-commitments.txt"
pass

# ===================================================================================== 14
step "Outbound: G2-lite updates over the LEZ blocks, submitted to the vault" "$L_OUT"
roundtrip submit1 && rt_done submit1 || fail "round trip: updates"
grep -qi "$(echo "${XE#0x}" | tr 'A-F' 'a-f')" "$R/update1/report.json" || fail "X's exit commitment is not in G2 update 1"
note "X's exit commitment is in G2 update 1's exits"
pass

# ===================================================================================== 15
step "Ethereum: releases (P's seven exits, then X's)" "$L_OUT"
roundtrip releases2 && rt_done releases2 || fail "round trip: releases"
show "outbound_host release --secret <X's exit secret> --eth-dest $XR --token $ETH --denom $D_ETH … --relayer $ETH --relay-fee 0"
XIDX_E=$(python3 - "$XE" "$R/exit-leaves.txt" "$CFG_ID" $ETH $D_ETH <<'PY'
import hashlib, sys
E, lf, cfg, tok, d = sys.argv[1:]
# the vault's exit leaf, as e2e-roundtrip.sh computes it: SHA256(tag | cfgId | token | denom | E)
tag = bytes.fromhex("22018742ad6028fda6cb0726afaae90ea20dff3cef6b1ec5b3c14f856d76c59c")
leaf = "0x" + hashlib.sha256(tag + bytes.fromhex(cfg[2:]) + bytes.fromhex(tok[2:]) + int(d).to_bytes(32, "big") + bytes.fromhex(E[2:])).hexdigest()
print([l.strip() for l in open(lf) if l.strip()].index(leaf))
PY
) || fail "X's exit leaf is not in the vault's exit leaves"
# shellcheck disable=SC2086  # RUN_WRAP is a command prefix
$RUN_WRAP "$HOST" release --secret "$(cat "$RUN/x/exit.secret")" --eth-dest "$XR" --token $ETH --denom $D_ETH \
  --max-relay-fee $XMAX_ETH --cfg-id "$CFG_ID" --leaves "$R/exit-leaves.txt" --index "$XIDX_E" --relayer $ETH --relay-fee 0 \
  > "$RUN/x/release.txt" 2>&1 || { tail -5 "$RUN/x/release.txt"; fail "G3 release (executor)"; }
grep -E '^(ROOT|NULLIFIER|USER_CYCLES)=' "$RUN/x/release.txt"
XROOT=$(sed -n 's/^ROOT=//p' "$RUN/x/release.txt"); XNF=$(sed -n 's/^NULLIFIER=//p' "$RUN/x/release.txt"); XSEAL=$(sed -n 's/^SEAL=//p' "$RUN/x/release.txt")
at=$(cast call "$VAULT" "releasableAt(address,bytes32)(uint256)" $ETH "$XROOT" --rpc-url "$ETH_RPC" | cut -d' ' -f1)
while [ "$(date +%s)" -le "$((at + 1))" ]; do sleep 2; done
b0=$(ebal "$XR")
show "cast send $VAULT 'release(bytes,bytes32,bytes32,address,uint32,address,address,uint256)' <seal> $XROOT $XNF $ETH 0 $XR $ETH 0   (anvil account 0)"
LAST_OUT=$RUN/x/release-tx.json
cast send "$VAULT" "release(bytes,bytes32,bytes32,address,uint32,address,address,uint256)" "$XSEAL" "$XROOT" "$XNF" $ETH 0 "$XR" $ETH 0 \
  --private-key $ANVIL0 --rpc-url "$ETH_RPC" --json > "$RUN/x/release-tx.json" || fail "X's release on the vault"
XTX=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["transactionHash"])' "$RUN/x/release-tx.json")
b1=$(ebal "$XR")
[ "$((b1 - b0))" = "$((D_ETH - WF_ETH))" ] || fail "X's recipient got $((b1 - b0)), want $((D_ETH - WF_ETH))"
[ "$(cast call "$VAULT" 'releaseNullifierSpent(bytes32)(bool)' "$XNF" --rpc-url "$ETH_RPC")" = true ] || fail "X's release nullifier not spent"
echo "x $XTX" >> "$R/released.txt"
note "X's release tx $XTX: recipient $XR +$((b1 - b0)) wei (0.01 ETH - 0.5% withdraw fee), nullifier spent"
pass

# ===================================================================================== 16
step "Final balances reconciled on both sides" "$L_OUT"
note "re-running the round trip's closing checks over every release (X's included): vault counters = bridge counters,"
note "vault balance = deposits - releases + fees per token, LEZ supply = claimed - burned, no exit secret in any release"
roundtrip "" || fail "round trip: closing checks"
xn=$(native "$X")
xw=$(xcli balances --token-program "$TP" 2>/dev/null | awk -v x="Public/$X" -v d="$DEF_ETH" '$1 == x && $2 == d {print $3}')
note "X on LEZ: native $xn (top-up $x0 minus its burn fee), wrapped ETH ${xw:-0} (claimed $D_ETH, burned $D_ETH)"
[ "${xw:-0}" = 0 ] || fail "X still holds wrapped ETH: $xw"
note "X on Ethereum: recipient $XR holds $(ebal "$XR") wei"
[ "$(ebal "$XR")" = "$((D_ETH - WF_ETH))" ] || fail "recipient balance"
note "relay payer: paid $((p0 - p1)) for X's claim; X paid nothing for it"
pass

finish
