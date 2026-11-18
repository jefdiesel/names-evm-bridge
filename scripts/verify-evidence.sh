#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# verify-evidence.sh: re-check every public-network row of docs/EVIDENCE.md against public endpoints,
# read only (no key, no account, no build, no proof). From a clean clone, from anywhere:
#
#   scripts/verify-evidence.sh                       # ask the public endpoints now (about a minute)
#   scripts/verify-evidence.sh --offline docs/evidence-answers   # replay a saved set; no network
#   scripts/verify-evidence.sh --save <dir>          # ask, and save the answers to <dir>/answers.json
#
# Needs bash, curl and python3 (3.8 or later, standard library only). Foundry's cast is not needed;
# every Sepolia check is an ordinary eth_call or receipt read that cast can repeat.
#
# What it reads (expected values and the record each comes from: docs/evidence-checks.json):
# - Sepolia JSON-RPC, SEPOLIA_RPC (a comma-separated list, tried in order) or by default
#   https://sepolia.gateway.tenderly.co, then https://11155111.rpc.thirdweb.com: each transaction's
#   receipt (status 1, block, to or created address, sender where recorded, the vault's event, the
#   block's UTC date against the row's date); the v4 to v7 vaults' and gates' fixed values; RISC
#   Zero's verifier VERSION() and SELECTOR(); the committed v4 Groth16 seals through its verify(), a
#   changed seal refused; the v6 deposit root.
# - The team's public LEZ chain, LEZ_RPC or by default https://lez.nameswallet.app/ (JSON-RPC over
#   POST; under 4 requests a second): each named transaction's block (and size where recorded), and the
#   two RFP-021 light-client stores (instance, chain, head slot and period).
# - The deposit page (HTTP 200 and its title).
#
# Output: one PASS/FAIL line per check, tagged with the EVIDENCE.md row it backs; INFO lines print
# values that change over time; ERROR means no endpoint answered. Exit 0 only when every check passed.
# A value that disagrees with the chain is reported as FAIL, never adjusted here.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
command -v python3 > /dev/null || { echo "verify-evidence.sh needs python3" >&2; exit 2; }
case " $* " in
  *" --offline "*) ;;
  *) command -v curl > /dev/null || { echo "verify-evidence.sh needs curl (or use --offline <dir>)" >&2; exit 2; } ;;
esac
PYTHONDONTWRITEBYTECODE=1 exec python3 -I "$ROOT/scripts/verify-evidence.py" --root "$ROOT" "$@"
