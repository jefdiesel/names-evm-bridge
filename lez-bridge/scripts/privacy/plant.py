#!/usr/bin/python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""SYNTHETIC positive controls for the adversary's byte features (a test of the adversary, not of the bridge).

  plant.py <run dir> <capture dir> <out capture dir>

Copies the capture and appends, to the LEZ claim blocks of 15 main-set users (taken from the ground truth),
one value of the user's own deposit transaction: its sender address (20 bytes; feature exact20) for 5 users,
8 bytes of its commitment (sub8) for 5, its gas used as a u64 (fee) for 5 users whose gas used is unique in
the set. No real client publishes these; the planted capture shows whether the adversary would catch them.
"""
import base64, json, os, shutil, sys


def main(run, cap, out):
    shutil.copytree(cap, out)
    eth = json.load(open(os.path.join(cap, "ethereum.json")))
    gas = {}
    for rec in eth:
        for r in rec["receipts"]:
            gas[r["transactionHash"]] = int(r["gasUsed"], 16)
    dep = {json.loads(l)["user"]: json.loads(l) for l in open(os.path.join(run, "truth", "deposits.jsonl"))}
    cl = {json.loads(l)["user"]: json.loads(l)["block"] for l in open(os.path.join(run, "truth", "claims.jsonl"))}
    main = sorted(u for u in cl if u.startswith("main"))
    g = [gas[dep[u]["tx"]] for u in main]
    unique = [u for u in main if g.count(gas[dep[u]["tx"]]) == 1]
    fee_users = unique[:5]
    rest = [u for u in main if u not in fee_users]
    plan = {"exact20": rest[:5], "sub8": rest[5:10], "fee": fee_users}
    lez = json.load(open(os.path.join(cap, "lez-blocks.json")))
    for kind, users in plan.items():
        for u in users:
            d = dep[u]
            if kind == "exact20":
                add = bytes.fromhex(d["sender"][2:])
            elif kind == "sub8":
                add = bytes.fromhex(d["commitment"][2:])[7:15]
            else:
                add = gas[d["tx"]].to_bytes(8, "little")
            b = cl[u]
            lez[str(b)] = base64.b64encode(base64.b64decode(lez[str(b)]) + add).decode()
    json.dump(lez, open(os.path.join(out, "lez-blocks.json"), "w"))
    json.dump(plan, open(os.path.join(out, "PLANTED.json"), "w"), indent=1)
    print(json.dumps(plan))


if __name__ == "__main__":
    main(*sys.argv[1:4])
