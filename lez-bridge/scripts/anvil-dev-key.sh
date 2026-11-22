#!/usr/bin/env bash
# Usage: anvil-dev-key.sh [0-9]. Prints one of Anvil's public dev keys, asked from Anvil itself,
# so this repo holds no private key. Throwaway local chains only. Never echo the output.
set -euo pipefail
n=${1:-0}
case "$n" in [0-9]) ;; *) echo "anvil-dev-key: account 0 to 9" >&2; exit 2 ;; esac
export PATH="$HOME/.foundry/bin:$PATH"
command -v anvil >/dev/null || { echo "anvil-dev-key: Foundry's anvil is not installed" >&2; exit 2; }
d=$(mktemp -d); chmod 700 "$d"; pid=
trap '[ -z "$pid" ] || kill "$pid" 2>/dev/null || true; rm -rf "$d"' EXIT
anvil --port 0 --silent --config-out "$d/c.json" >/dev/null 2>&1 & pid=$!
for _ in $(seq 1 100); do
  if python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["private_keys"][int(sys.argv[2])])' "$d/c.json" "$n" 2>/dev/null; then exit 0; fi
  sleep 0.1
done
echo "anvil-dev-key: Anvil did not report its keys" >&2
exit 1
