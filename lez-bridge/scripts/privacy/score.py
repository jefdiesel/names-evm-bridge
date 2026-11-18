#!/usr/bin/python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Score the adversary's pairing against the ground truth (which the adversary never reads).

  score.py <run dir> <adversary dir>     writes <adversary dir>/score.json and prints tables

Chance: within one (token, denomination) set of n items, a uniformly random matching gets on average
exactly 1 right (n x 1/n), whatever n; P(at least k right) is computed exactly from the fixed-point
distribution of random permutations: P(exactly k) = C(n,k) D(n-k) / n!.
"""
import json, math, os, sys
from collections import defaultdict


def derange(m):
    d = [1, 0]
    for i in range(2, m + 1):
        d.append((i - 1) * (d[-1] + d[-2]))
    return d[m]


def p_at_least(n, k):
    if k <= 0:
        return 1.0
    return sum(math.comb(n, j) * derange(n - j) for j in range(k, n + 1)) / math.factorial(n)


def jl(path):
    return [json.loads(l) for l in open(path)] if os.path.exists(path) else []


def main(run, adv):
    T = os.path.join(run, "truth")
    dep = {x["tx"]: x for x in jl(os.path.join(T, "deposits.jsonl"))}
    cl = {x["block"]: x for x in jl(os.path.join(T, "claims.jsonl"))}
    bu = {x["E"]: x for x in jl(os.path.join(T, "burns.jsonl"))}
    rel = {x["tx"]: x for x in jl(os.path.join(T, "releases.jsonl"))}
    A = json.load(open(os.path.join(adv, "adversary.json")))

    def who(desc):
        k = desc.get("kind")
        if k == "deposit":
            x = dep.get(desc["tx"])
        elif k == "claim":
            x = cl.get(desc["block"])
        elif k == "burn":
            x = bu.get(desc["E"])
        elif k == "release":
            x = rel.get(desc["tx"])
        else:
            x = None
        return (x["user"], x["cohort"]) if x else (None, None)

    out = {}
    for prob in ("inbound", "outbound", "lez", "roundtrip"):
        P = A[prob]
        tw = [who(t) for t in P["targets"]]
        cw = [who(c) for c in P["candidates"]]
        res = {}
        base = defaultdict(lambda: [0.0, 0.0])  # expected right for a uniform guess among the feasible candidates
        for i, (u, coh) in enumerate(tw):
            base[coh][0] += 1.0 / max(1, P["hard_set_sizes"][i])
            base[coh][1] += 1.0 / max(1, P.get("feasible_sizes", P["hard_set_sizes"])[i])
        res["guess_baseline"] = {c: {"allowed": round(v[0], 2), "everything": round(v[1], 2)} for c, v in base.items()}
        for mode in ("allowed", "everything"):
            by = defaultdict(lambda: [0, 0])
            for i, (u, coh) in enumerate(tw):
                j = P["pairing"][mode].get(str(i))
                ok = j is not None and cw[j][0] == u and u is not None
                by[coh][0] += ok
                by[coh][1] += 1
            res[mode] = {c: {"correct": v[0], "n": v[1], "expected_random": 1.0 if v[1] else 0.0,
                             "expected_feasible_guess": res["guess_baseline"][c][mode],
                             "p_at_least_observed": p_at_least(v[1], v[0])} for c, v in by.items()}
        feats = {}
        for f, per in P["features"].items():
            agg = defaultdict(lambda: {"n": 0, "mean_set": 0.0, "mean_of": 0.0, "truth_kept": 0, "unique_correct": 0, "unique_wrong": 0})
            for i, (u, coh) in enumerate(tw):
                a = agg[coh]
                x = per[i]
                a["n"] += 1
                a["mean_set"] += x["set"]
                a["mean_of"] += x["of"]
                a["truth_kept"] += any(cw[j][0] == u for j in x["keep"])
                if x["guess"] is not None:
                    a["unique_correct" if cw[x["guess"]][0] == u else "unique_wrong"] += 1
            for a in agg.values():
                a["mean_set"] = round(a["mean_set"] / max(1, a["n"]), 2)
                a["mean_of"] = round(a["mean_of"] / max(1, a["n"]), 2)
            feats[f] = dict(agg)
        res["features"] = feats
        res["unmatched_targets"] = sum(1 for u in tw if u[0] is None)
        out[prob] = res
    json.dump(out, open(os.path.join(adv, "score.json"), "w"), indent=1)
    for prob, res in out.items():
        print("\n## %s (unmatched targets: %d)" % (prob, res["unmatched_targets"]))
        print("| cohort | mode | correct / n | expected, random matching | P(>= observed), random matching | expected, guess among feasible |")
        print("|---|---|---|---|---|---|")
        for mode in ("allowed", "everything"):
            for c, v in sorted(res[mode].items(), key=lambda kv: str(kv[0])):
                print("| %s | %s | %d / %d | %.1f | %.3g | %.2f |" % (c, mode, v["correct"], v["n"], v["expected_random"], v["p_at_least_observed"], v["expected_feasible_guess"]))
        print("\n| feature | cohort | mean set kept / of | truth kept | unique right | unique wrong |")
        print("|---|---|---|---|---|---|")
        for f, agg in res["features"].items():
            for c, a in sorted(agg.items(), key=lambda kv: str(kv[0])):
                print("| %s | %s | %.2f / %.2f | %d / %d | %d | %d |" % (f, c, a["mean_set"], a["mean_of"], a["truth_kept"], a["n"], a["unique_correct"], a["unique_wrong"]))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
