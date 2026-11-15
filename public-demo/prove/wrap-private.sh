#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Encrypt a receipt (receipt.bin + journal.hex) for .github/workflows/wrap-private.yml, so a
# public repository's runner can wrap it without the journal (which can name a person's Ethereum
# address) ever being public. For the conductor; never run it on someone else's receipt without
# their word.
#
#   wrap-private.sh --src DIR [--ssh HOST] --key-file FILE [--repo-root DIR] [--image-id H]
#   wrap-private.sh --decrypt-test BUNDLE --key-file FILE      decrypt into a temp dir, print the digest, delete
#
# --src DIR: the receipt's directory, holding receipt.bin and journal.hex (on HOST with --ssh,
#   read with `ssh HOST cat`, never copied to disk outside a mode-700 temp dir that is deleted).
# --key-file FILE: the 32-byte random key, hex, mode 600. Made here if missing (`openssl rand`),
#   never printed. Reuse it for later bundles or make a new one per bundle.
# Writes <repo-root>/public-demo/prove/receipts-enc/<id>.bin (id: 16 random hex characters), the
#   ciphertext: `tar receipt.bin journal.hex | openssl enc -aes-256-cbc -pbkdf2 -iter 200000 -md
#   sha256 -salt`. Safe to publish. Prints the id, the journal's sha256 digest, the image id and the
#   exact commands to set the repository secret (from the file, not echoed), commit, dispatch and
#   download.
# What an observer of the public repository learns: the ciphertext's size (about the receipt's,
#   ~224 KB), when it was committed and wrapped, the image id, the journal digest, the seal and the
#   on-chain result. Not the journal: the digest is a hash. (The vault's `release` call publishes the
#   journal's fields itself, recipient included, when it is sent.)
set -uo pipefail
OPENSSL=${OPENSSL:-openssl}
ENC=(-aes-256-cbc -pbkdf2 -iter 200000 -md sha256)
PUBLIC_REPO=${PUBLIC_REPO:-jefdiesel/names-evm-bridge}
SRC="" HOST="" KEY="" ROOT="" IMAGE_ID=0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4 TEST=""
while [ $# -gt 0 ]; do
  case "$1" in
    --src) SRC=${2:-}; shift 2 ;;
    --ssh) HOST=${2:-}; shift 2 ;;
    --key-file) KEY=${2:-}; shift 2 ;;
    --repo-root) ROOT=${2:-}; shift 2 ;;
    --image-id) IMAGE_ID=${2:-}; shift 2 ;;
    --decrypt-test) TEST=${2:-}; shift 2 ;;
    *) echo "usage: $0 --src DIR [--ssh HOST] --key-file FILE [--repo-root DIR] [--image-id H] | --decrypt-test BUNDLE --key-file FILE"; exit 2 ;;
  esac
done
[ -n "$KEY" ] || { echo "--key-file is required"; exit 2; }
digest() { python3 -I -c 'import sys,hashlib; print("0x"+hashlib.sha256(bytes.fromhex(open(sys.argv[1]).read().strip().removeprefix("0x"))).hexdigest())' "$1"; }
TMP=$(mktemp -d "${TMPDIR:-/tmp}/wrap-private.XXXXXX") && chmod 700 "$TMP"
trap 'rm -rf "$TMP"' EXIT

if [ -n "$TEST" ]; then
  [ -f "$KEY" ] || { echo "no key file $KEY"; exit 2; }
  WRAP_KEY=$(cat "$KEY") "$OPENSSL" enc -d "${ENC[@]}" -pass env:WRAP_KEY -in "$TEST" | tar -x -C "$TMP" receipt.bin journal.hex || { echo "decrypt failed"; exit 1; }
  echo "decrypted: receipt.bin $(wc -c < "$TMP/receipt.bin" | tr -d ' ') bytes, journal digest $(digest "$TMP/journal.hex")"
  exit 0
fi

[ -n "$SRC" ] || { echo "--src is required"; exit 2; }
ROOT=${ROOT:-$(git rev-parse --show-toplevel 2>/dev/null)}
[ -d "$ROOT/public-demo/prove" ] || { echo "--repo-root must be a checkout of the bridge repository"; exit 2; }
if [ ! -f "$KEY" ]; then
  ( umask 077; "$OPENSSL" rand -hex 32 > "$KEY" ) || { echo "cannot make $KEY"; exit 1; }
  echo "made a new key in $KEY (mode 600; not shown)"
fi
case "$(stat -f %Lp "$KEY" 2>/dev/null || stat -c %a "$KEY")" in 600|400) ;; *) echo "$KEY must be mode 600"; exit 2 ;; esac
for f in receipt.bin journal.hex; do
  if [ -n "$HOST" ]; then
    ssh "$HOST" cat "$SRC/$f" > "$TMP/$f" || { echo "cannot read $HOST:$SRC/$f"; exit 1; }
  else
    cp "$SRC/$f" "$TMP/$f" || { echo "cannot read $SRC/$f"; exit 1; }
  fi
done
D=$(digest "$TMP/journal.hex") || { echo "journal.hex is not hex"; exit 1; }
if [ -f "$SRC/journal.sha256" ] || { [ -n "$HOST" ] && ssh "$HOST" test -f "$SRC/journal.sha256"; }; then
  want=$( { [ -n "$HOST" ] && ssh "$HOST" cat "$SRC/journal.sha256"; } || cat "$SRC/journal.sha256")
  want=$(echo "$want" | tr -d ' \n' | tr 'A-F' 'a-f'); case "$want" in 0x*) ;; *) want=0x$want ;; esac
  [ "$want" = "$D" ] || { echo "journal.sha256 ($want) is not the journal's digest ($D)"; exit 1; }
  echo "journal.sha256 matches"
fi
ID=$("$OPENSSL" rand -hex 8)
OUTD=$ROOT/public-demo/prove/receipts-enc
mkdir -p "$OUTD"
B=$OUTD/$ID.bin
( cd "$TMP" && COPYFILE_DISABLE=1 tar -c --format ustar receipt.bin journal.hex ) | WRAP_KEY=$(cat "$KEY") "$OPENSSL" enc -e "${ENC[@]}" -salt -pass env:WRAP_KEY -out "$B" || { echo "encrypt failed"; rm -f "$B"; exit 1; }
# round trip before anything is published
WRAP_KEY=$(cat "$KEY") "$OPENSSL" enc -d "${ENC[@]}" -pass env:WRAP_KEY -in "$B" | tar -x -O journal.hex > "$TMP/check.hex" && [ "$(digest "$TMP/check.hex")" = "$D" ] \
  || { echo "the bundle does not decrypt back to the same journal"; rm -f "$B"; exit 1; }
cat <<EOF
bundle   $B ($(wc -c < "$B" | tr -d ' ') bytes; ciphertext, safe to publish)
id       $ID
image    $IMAGE_ID
digest   $D

Next (the conductor):
  1. gh secret set WRAP_KEY --repo $PUBLIC_REPO < $KEY          # reads the key from the file; nothing echoed
  2. git -C $ROOT add public-demo/prove/receipts-enc/$ID.bin && git -C $ROOT commit -m "receipts-enc: $ID (encrypted receipt for the wrap)"
     then export and push to the public repository as usual
  3. gh workflow run wrap-private.yml --repo $PUBLIC_REPO --ref main -f bundle=$ID -f journal_digest=$D -f image_id=$IMAGE_ID
  4. gh run list --repo $PUBLIC_REPO --workflow wrap-private.yml --limit 1
     gh run download <run id> --repo $PUBLIC_REPO --name wrap-private-$ID --dir <dir>   # seal.bin, seal.hex, summary.txt
EOF
