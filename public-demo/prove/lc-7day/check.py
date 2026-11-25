#!/usr/bin/env python3
"""Re-read every light-client update in updates.csv from the public LEZ chain.

For each row it fetches the transaction by its hash (getTransaction) and the block the row names
(getBlock), and checks that the transaction's bytes are inside that block. Standard library only.

  python3 check.py                 # every row (about 4 minutes, paced under the RPC's rate limit)
  python3 check.py --sample 20     # 20 rows spread over the run
  python3 check.py --rpc URL       # another node of the same chain
"""
import argparse, base64, csv, json, os, sys, time, urllib.request

def rpc(url, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(url, data=body, headers={"content-type": "application/json", "user-agent": "lc-7day-check"})
    for attempt in range(4):
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                out = json.load(r)
            if "error" in out:
                raise RuntimeError(out["error"])
            return out["result"]
        except Exception:
            if attempt == 3:
                raise
            time.sleep(3 * (attempt + 1))

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", default="https://lez.nameswallet.app/")
    ap.add_argument("--sample", type=int, default=0)
    a = ap.parse_args()
    rows = list(csv.DictReader(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "updates.csv"))))
    if a.sample and a.sample < len(rows):
        step = len(rows) / a.sample
        rows = [rows[int(i * step)] for i in range(a.sample)]
    ok = bad = 0
    for i, r in enumerate(rows, 1):
        try:
            t = rpc(a.rpc, "getTransaction", [r["tx"]])
            tx = base64.b64decode(t[0] if isinstance(t, list) else t)
            blk = base64.b64decode(rpc(a.rpc, "getBlock", [int(r["lez_block"])]))
            if tx and tx in blk:
                ok += 1
            else:
                bad += 1
                print(f"NOT IN BLOCK: {r['utc']} {r['kind']} tx {r['tx']} block {r['lez_block']}")
        except Exception as e:
            bad += 1
            print(f"ERROR: {r['utc']} {r['kind']} tx {r['tx']}: {e}")
        if i % 25 == 0:
            print(f"  {i}/{len(rows)} checked", file=sys.stderr)
        time.sleep(0.25)
    print(f"{ok} of {len(rows)} updates found in the block the record names; {bad} not.")
    sys.exit(1 if bad else 0)

if __name__ == "__main__":
    main()
