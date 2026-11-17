#!/usr/bin/python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""The adversary: pairs deposits with claims and burns with releases from public data only.

  adversary.py <capture dir> <out dir>

Reads only <capture dir> (every Ethereum block, transaction, receipt and vault storage diff; every
LEZ block's bytes; public.json, the deployment's public constants). Never reads the ground truth.
Writes <out dir>/pairing.json (its best pairing per problem and mode) and features.json (per
feature: how far it narrows each item's anonymity set, and its own best guess per item).

Problems: inbound (claim -> deposit), outbound (release -> burn), roundtrip (release -> deposit),
lez (burn -> claim, inside LEZ). Modes: "allowed" (token, denomination, time and order only) and
"everything" (every feature below).

Features (each scored independently, then summed in "everything"):
  set        same token and denomination                                     (allowed, hard)
  order      the earlier event happened before the later one                 (allowed, hard)
  timing     closeness in time rank                                          (allowed, soft)
  gap        a short time from the earlier event to the later (-log(1 + seconds)) (allowed, soft)
  bound      the claim's root (or the release's exit root) bounds the set   (published anchor)
  exact32    a 32-byte value of one event appears in the other's bytes
  exact20    a 20-byte value (an Ethereum address) appears in the other's bytes
  sub8       an 8-byte high-entropy substring shared between the two
  derivedC   recompute C = SHA256("RFP022/dep"|k|dest|token|denom|cfgId) from published 32-byte
             values (as k: every window, and the padding / SHA-256 / Keccak of every Ethereum
             address seen) and account ids (as dest), and match a deposit's C
  derivedNf  recompute nf_r = SHA256("RFP022/nfr"|k) from every published 32-byte window and
             match a release's nullifier
  fee        a fee-like integer (>= 100000) of one event equals one of the other's
  sender     the same Ethereum account sends/receives both (round trip)
  size       closeness of transaction sizes (bytes) / gas used, rank-matched
"""
import hashlib, json, math, os, sys
from collections import defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from keccak import keccak256  # noqa: E402

B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58d(s):
    n = 0
    for c in s:
        n = n * 58 + B58.index(c)
    return n.to_bytes(32, "big")


def h(x):
    return bytes.fromhex(x[2:] if x.startswith("0x") else x)


def sha(*parts):
    return hashlib.sha256(b"".join(parts)).digest()


DEP_TOPIC = keccak256(b"Deposit(address,uint32,bytes32,uint64,bytes32)")
SU_SIG = keccak256(b"StateUpdated(bytes32,bytes32,bytes32,uint64,bytes32[])")
REL_SEL = keccak256(b"release(bytes,bytes32,bytes32,address,uint32,address,address,uint256)")[:4]
DEP_LEAF_TAG = keccak256(b"RFP022/deposit-leaf/v1")
EXIT_LEAF_TAG = keccak256(b"RFP022/exit-leaf/v1")


def u256(n):
    return int(n).to_bytes(32, "big")


def entropy_ok(w):
    return len(set(w)) >= 5 and w.count(0) <= 2


def windows(b, n, step=1):
    return {b[i:i + n] for i in range(0, len(b) - n + 1, step)}


def ints_le(b):
    """Every u64 and u128 little-endian value at any offset, in a fee-like range."""
    out = set()
    for i in range(len(b) - 7):
        v = int.from_bytes(b[i:i + 8], "little")
        if 100_000 <= v < 10 ** 22:
            out.add(v)
    return out


# ------------------------------------------------------------------------------- load
def load(cap):
    pub = json.load(open(os.path.join(cap, "public.json")))
    eth = json.load(open(os.path.join(cap, "ethereum.json")))
    lez = json.load(open(os.path.join(cap, "lez-blocks.json")))
    return pub, eth, lez


def parse_eth(pub, eth):
    vault = pub["vault"].lower()
    deposits, releases, updates, txs = [], [], [], {}
    for rec in eth:
        blk = rec["block"]
        t = int(blk["timestamp"], 16)
        rc = {r["transactionHash"]: r for r in rec["receipts"]}
        for tx in blk["transactions"]:
            r = rc[tx["hash"]]
            ints = {int(tx[k], 16) for k in ("gasPrice", "maxFeePerGas", "maxPriorityFeePerGas", "value", "gas") if tx.get(k)}
            ints.add(int(r["gasUsed"], 16))
            if r.get("effectiveGasPrice"):
                ints.add(int(r["effectiveGasPrice"], 16))
            ints = {v for v in ints if v >= 100_000}
            inp = h(tx["input"])
            vals = {h(tx["hash"]), h(blk["hash"]), h(tx["from"]), h(tx["to"] or "0x" + "00" * 20)}
            vals |= {inp[4 + 32 * i:36 + 32 * i] for i in range((len(inp) - 4) // 32)}
            # the transaction's storage writes (prestate diff): every slot and value it changed
            dd = rec.get("diffs", {}).get(tx["hash"], {})
            for side in ("pre", "post"):
                for acc in (dd.get(side) or {}).values() if isinstance(dd, dict) else []:
                    for k2, v2 in (acc.get("storage") or {}).items():
                        vals |= {h(k2), h(v2)}
            for lg in r["logs"]:
                vals |= {h(x) for x in lg["topics"]}
                d = h(lg["data"])
                vals |= {d[32 * i:32 * i + 32] for i in range(len(d) // 32)}
            base = {"tx": tx["hash"], "from": tx["from"].lower(), "time": t, "block": int(blk["number"], 16), "ints": ints,
                    "vals": {v for v in vals if len(v) in (20, 32) and entropy_ok(v)}, "bytes": inp, "gas": int(r["gasUsed"], 16)}
            txs[tx["hash"]] = base
            for lg in r["logs"]:
                if lg["address"].lower() != vault:
                    continue
                if h(lg["topics"][0]) == DEP_TOPIC:
                    d = h(lg["data"])
                    dep = dict(base, kind="deposit", token="0x" + lg["topics"][1][-40:], di=int.from_bytes(d[0:32], "big"),
                               C=d[32:64], leaf_index=int.from_bytes(d[64:96], "big"), root=d[96:128])
                    deposits.append(dep)
                elif h(lg["topics"][0]) == SU_SIG:
                    d = h(lg["data"])
                    first = int.from_bytes(d[96:128], "big")
                    off = int.from_bytes(d[128:160], "big")
                    n = int.from_bytes(d[off:off + 32], "big")
                    leaves = [d[off + 32 + 32 * k: off + 64 + 32 * k] for k in range(n)]
                    updates.append(dict(base, exit_root=d[64:96], first=first, leaves=leaves))
            if (tx["to"] or "").lower() == vault and inp[:4] == REL_SEL and int(r["status"], 16) == 1:
                a = inp[4:]
                w = lambda i: a[32 * i:32 * i + 32]
                releases.append(dict(base, kind="release", root=w(1), nf=w(2), token="0x" + w(3)[-20:].hex(), di=int.from_bytes(w(4), "big"),
                                     recipient="0x" + w(5)[-20:].hex(), relayer="0x" + w(6)[-20:].hex(), fee=int.from_bytes(w(7), "big")))
    return deposits, releases, updates, txs


def parse_lez(lez):
    import base64
    blocks = []
    for k in sorted(lez, key=int):
        b = base64.b64decode(lez[k])
        bid = int.from_bytes(b[0:8], "little")
        ts = int.from_bytes(b[72:80], "little")
        blocks.append({"id": bid, "time_ms": ts, "bytes": b, "body": b[80:]})
    return blocks


# ------------------------------------------------------------------------------- matching
def hungarian(score):
    """Max-weight assignment for a rectangular matrix (rows <= cols); -inf = forbidden. Returns row -> col."""
    n, m = len(score), len(score[0]) if score else 0
    if n == 0:
        return {}
    big = 1e9
    cost = [[(-score[i][j] if score[i][j] > -1e17 else big) for j in range(m)] for i in range(n)]
    INF = float("inf")
    u, v, p, way = [0.0] * (n + 1), [0.0] * (m + 1), [0] * (m + 1), [0] * (m + 1)
    for i in range(1, n + 1):
        p[0] = i
        j0 = 0
        minv = [INF] * (m + 1)
        used = [False] * (m + 1)
        while True:
            used[j0] = True
            i0, delta, j1 = p[j0], INF, -1
            for j in range(1, m + 1):
                if not used[j]:
                    cur = cost[i0 - 1][j - 1] - u[i0] - v[j]
                    if cur < minv[j]:
                        minv[j], way[j] = cur, j0
                    if minv[j] < delta:
                        delta, j1 = minv[j], j
            for j in range(m + 1):
                if used[j]:
                    u[p[j]] += delta
                    v[j] -= delta
                else:
                    minv[j] -= delta
            j0 = j1
            if p[j0] == 0:
                break
        while True:
            j1 = way[j0]
            p[j0] = p[j1]
            j0 = j1
            if j0 == 0:
                break
    return {p[j] - 1: j - 1 for j in range(1, m + 1) if p[j]}


# ------------------------------------------------------------------------------- LEZ events
def lez_events(pub, blocks, deposits, updates):
    """Find claims and burns in the LEZ blocks from public data alone.

    A burn names E; the vault later lists its exit leaf SHA256(tag|cfg|token|denom|E), so every 32-byte
    window of a block is tried as E. A claim mints a denomination of a wrapped definition: the block
    names the definition (a public id from `register`), carries the denomination as a u128 and names
    the bridge config; deploy/setup blocks (before the first deposit's root was attested) are skipped."""
    cfg = h(pub["cfg_id"])
    tokens = pub["tokens"]  # [{eth, def, denoms}]
    leafmap = {}
    for up in updates:
        for k, lf in enumerate(up["leaves"]):
            leafmap[lf] = (up["first"] + k, up)
    roots = {d["root"] for d in deposits}
    # attest blocks publish a deposit root in clear; a claim names the root's account (a PDA), which the
    # attest block also names: map each root to the rare 32-byte windows of its attest block
    attest = {}
    first_seen = set()
    for blk in blocks:
        w = windows(blk["body"], 32)
        new = w - first_seen  # windows this block is the first to name (a root's account appears first where it is attested)
        first_seen |= w
        for x in roots & w:
            attest.setdefault(x, (blk["id"], new))  # the first block naming a root records it (public claims repeat it)
    first_attest = min([v[0] for v in attest.values()] or [0])
    defs = {b58d(t["def"]): t for t in tokens}
    cfg_acct = b58d(pub["config"])
    bridge = b58d(pub["bridge"])
    # windows common to many blocks (program ids, config, clock, the attestor) never identify a root
    seen = defaultdict(int)
    for blk in blocks:
        for w in windows(blk["body"], 32):
            seen[w] += 1
    common = {w for w, c in seen.items() if c > 3}
    claims, burns = [], []
    for blk in blocks:
        body = blk["body"]
        if bridge not in body and cfg_acct not in body:
            continue
        w32 = windows(body, 32)
        found_burn = False
        for t in tokens:
            for di, dn in enumerate(t["denoms"]):
                pre = EXIT_LEAF_TAG + cfg + h(t["eth"]).rjust(20, b"\0") + u256(dn)
                for w in w32:
                    lf = sha(pre, w)
                    if lf in leafmap:
                        burns.append({"kind": "burn", "block": blk["id"], "time": blk["time_ms"] / 1000, "E": w, "leaf": lf,
                                      "leaf_index": leafmap[lf][0], "token": t["eth"].lower(), "di": di, "size": len(body), "blk": blk})
                        found_burn = True
        if found_burn:
            continue
        if blk["id"] <= first_attest:
            continue
        hit_roots = [x for x, (bid, aw) in attest.items() if bid < blk["id"] and {y for y in aw & w32 if entropy_ok(y)}]
        for d, t in defs.items():
            if d not in w32:
                continue
            for di, dn in enumerate(t["denoms"]):
                if dn.to_bytes(16, "little") in body:
                    claims.append({"kind": "claim", "block": blk["id"], "time": blk["time_ms"] / 1000, "token": t["eth"].lower(), "di": di,
                                   "roots": hit_roots, "size": len(body), "blk": blk})
    return claims, burns


# ------------------------------------------------------------------------------- features
class Feat:
    """Per-event byte views, computed once."""

    def __init__(self, consts):
        self.consts = consts
        self.cache = {}

    def straddles(self, w):
        if not hasattr(self, "c24"):
            self.c24 = set()
            for c in self.consts:
                self.c24 |= windows(c, 24)
        return any(w[i:i + 24] in self.c24 for i in range(0, 9))

    def view(self, ev):
        k = id(ev)
        if k not in self.cache:
            b = ev["blk"]["body"] if "blk" in ev else ev["bytes"]
            extra = ev.get("vals", set())
            # a window that is mostly a constant (a repeated program id plus a byte or two of its neighbour) is noise
            w32 = {w for w in windows(b, 32) if entropy_ok(w) and w not in self.consts and not self.straddles(w)} | {v for v in extra if len(v) == 32}
            w20 = {w for w in windows(b, 20) if entropy_ok(w) and w not in self.consts} | {v for v in extra if len(v) == 20}
            w8 = {w for w in windows(b, 8) if entropy_ok(w)}
            for v in extra:
                w8 |= {w for w in windows(v, 8) if entropy_ok(w)}
            ints = ints_le(b) | ev.get("ints", set())
            self.cache[k] = {"w32": w32, "w20": w20, "w8": w8, "ints": ints, "body": b}
        return self.cache[k]


def same_set(x, y):
    return x["token"].lower() == y["token"].lower() and x["di"] == y["di"]


def rank_map(evs):
    out = {}
    groups = defaultdict(list)
    for e in evs:
        groups[(e["token"].lower(), e["di"])].append(e)
    for g in groups.values():
        g.sort(key=lambda e: e["time"])
        for i, e in enumerate(g):
            out[id(e)] = i / max(1, len(g) - 1)
    return out


def shared(fv, a, b, key, consts8=frozenset()):
    va, vb = fv.view(a)[key], fv.view(b)[key]
    s = va & vb
    if consts8:
        s -= consts8
    return len(s)


def derived_c(fv, claim, deposits, pub, programs, addr_ks):
    """Deposits whose C is recomputed from values the claim publishes (k = a window or an address-derived
    value, dest = an account id: a window followed by a known program id or constant)."""
    body = fv.view(claim)["body"]
    cfg = h(pub["cfg_id"])
    tok = h(claim["token"]).rjust(20, b"\0")
    dn = u256([t for t in pub["tokens"] if t["eth"].lower() == claim["token"]][0]["denoms"][claim["di"]])
    dests = {body[i:i + 32] for i in range(len(body) - 63) if body[i + 32:i + 64] in programs}
    ks = {w for w in fv.view(claim)["w32"]} | addr_ks
    want = {d["C"]: d for d in deposits if same_set(d, claim)}
    hits = set()
    tag = b"RFP022/dep"
    for dest in dests:
        for k in ks:
            c = sha(tag, k, dest, tok, dn, cfg)
            if c in want:
                hits.add(id(want[c]))
    return hits


def derived_nf(fv, burn, releases):
    want = {r["nf"]: r for r in releases}
    hits = set()
    for w in fv.view(burn)["w32"]:
        n = sha(b"RFP022/nfr", w)
        if n in want:
            hits.add(id(want[n]))
    return hits


def solve(targets, cands, score):
    if not targets or not cands:
        return {}
    m = [[score(t, c) for c in cands] for t in targets]
    a = hungarian(m)
    return {i: (j if m[i][j] > -1e17 else None) for i, j in a.items()}


def analyse(cap, out):
    pub, eth, lez = load(cap)
    deposits, releases, updates, _ = parse_eth(pub, eth)
    blocks = parse_lez(lez)
    claims, burns = lez_events(pub, blocks, deposits, updates)
    # constants: 32-byte windows present in more than half of the bridge blocks (program ids, config, definitions)
    cnt = defaultdict(int)
    bridge_blocks = [e["blk"] for e in claims + burns]
    for blk in bridge_blocks:
        for w in windows(blk["body"], 32):
            cnt[w] += 1
    consts = {w for w, c in cnt.items() if c > max(2, len(bridge_blocks) // 2)}
    programs = consts | {b58d(pub["bridge"]), b58d(pub["token_program"])}
    fv = Feat(consts)
    addrs = {h(d["from"]) for d in deposits} | {h(r["from"]) for r in releases}
    addr_ks = set()
    for a in addrs:
        addr_ks |= {a.rjust(32, b"\0"), sha(a), keccak256(a), a.ljust(32, b"\0")}
    rk = rank_map(deposits + claims + burns + releases)
    # size ranks within each set: a LEZ transaction's block bytes, an Ethereum transaction's gas used
    szr = {}
    for evs, key in ((claims, "size"), (burns, "size"), (deposits, "gas"), (releases, "gas")):
        groups = defaultdict(list)
        for e in evs:
            groups[(e["token"].lower(), e["di"])].append(e)
        for g in groups.values():
            vals = sorted(e[key] for e in g)
            for e in g:  # average rank of ties: equal sizes carry no order
                lo, hi = vals.index(e[key]), len(vals) - 1 - vals[::-1].index(e[key])
                szr[id(e)] = (lo + hi) / 2 / max(1, len(g) - 1)
    # exit-tree size at each exit root, and deposit count at each deposit root
    exit_count = {}
    for up in updates:
        exit_count[up["exit_root"]] = up["first"] + len(up["leaves"])
    dep_count = {d["root"]: d["leaf_index"] + 1 for d in deposits}

    feats = {}

    def per_pair(name, targets, cands, fn):
        feats[name] = {i: {j: fn(t, c) for j, c in enumerate(cands)} for i, t in enumerate(targets)}

    # ---------------------------------------------------------------- inbound: claim -> deposit
    hard_in = lambda c, d: same_set(c, d) and d["time"] <= c["time"] + 1
    bound_in = lambda c, d: (not c["roots"]) or any(d["leaf_index"] < dep_count.get(r, 10 ** 9) for r in c["roots"])
    dc = {id(c): derived_c(fv, c, deposits, pub, programs, addr_ks) for c in claims}
    F_in = {
        "timing": lambda c, d: -abs(rk[id(c)] - rk[id(d)]),
        "gap": lambda c, d: -math.log1p(max(0.0, c["time"] - d["time"])),
        "bound": lambda c, d: 0 if bound_in(c, d) else -1e18,
        "exact32": lambda c, d: shared(fv, c, d, "w32"),
        "exact20": lambda c, d: shared(fv, c, d, "w20"),
        "sub8": lambda c, d: shared(fv, c, d, "w8"),
        "derivedC": lambda c, d: 1 if id(d) in dc[id(c)] else 0,
        "fee": lambda c, d: len(fv.view(c)["ints"] & d["ints"]),
        "size": lambda c, d: -abs(szr[id(c)] - szr[id(d)]),
    }
    # ---------------------------------------------------------------- outbound: release -> burn
    hard_out = lambda r, b: same_set(r, b) and b["time"] <= r["time"] + 1
    bound_out = lambda r, b: b["leaf_index"] < exit_count.get(r["root"], 10 ** 9)
    dn = {id(b): derived_nf(fv, b, releases) for b in burns}
    F_out = {
        "timing": lambda r, b: -abs(rk[id(r)] - rk[id(b)]),
        "gap": lambda r, b: -math.log1p(max(0.0, r["time"] - b["time"])),
        "bound": lambda r, b: 0 if bound_out(r, b) else -1e18,
        "exact32": lambda r, b: shared(fv, r, b, "w32"),
        "exact20": lambda r, b: shared(fv, r, b, "w20"),
        "sub8": lambda r, b: shared(fv, r, b, "w8"),
        "derivedNf": lambda r, b: 1 if id(r) in dn[id(b)] else 0,
        "fee": lambda r, b: len(fv.view(b)["ints"] & r["ints"]),
        "size": lambda r, b: -abs(szr[id(r)] - szr[id(b)]),
    }
    # ---------------------------------------------------------------- inside LEZ: burn -> claim
    hard_lez = lambda b, c: same_set(b, c) and c["time"] <= b["time"]
    F_lez = {
        "timing": lambda b, c: -abs(rk[id(b)] - rk[id(c)]),
        "gap": lambda b, c: -math.log1p(max(0.0, b["time"] - c["time"])),
        "exact32": lambda b, c: shared(fv, b, c, "w32"),
        "sub8": lambda b, c: shared(fv, b, c, "w8"),
        "fee": lambda b, c: len(fv.view(b)["ints"] & fv.view(c)["ints"]),
    }
    # ---------------------------------------------------------------- round trip: release -> deposit
    hard_rt = lambda r, d: same_set(r, d) and d["time"] <= r["time"]
    F_rt = {
        "timing": lambda r, d: -abs(rk[id(r)] - rk[id(d)]),
        "gap": lambda r, d: -math.log1p(max(0.0, r["time"] - d["time"])),
        "sender": lambda r, d: int(d["from"] in (r["from"], r["relayer"].lower(), r["recipient"].lower())),
        "exact32": lambda r, d: len(r["vals"] & d["vals"]),
        "fee": lambda r, d: len(r["ints"] & d["ints"]),
    }
    W = {"gap": 1, "timing": 1, "bound": 1, "exact32": 50, "exact20": 50, "sub8": 5, "derivedC": 10000, "derivedNf": 10000, "fee": 200, "size": 0.5,
         "sender": 10000, "composed": 20}
    ALLOWED = {"timing", "gap"}  # the root / exit-root bound is a published anchor: counted under "everything"

    def run(problem, targets, cands, hard, F, extra=None):
        res = {"targets": [describe(t) for t in targets], "candidates": [describe(c) for c in cands], "features": {}, "pairing": {}}
        base = [[j for j, c in enumerate(cands) if hard(t, c)] for t in targets]
        res["hard_set_sizes"] = [len(b) for b in base]
        # chance baseline per target: a uniform guess among the candidates left by the set, time order and (for
        # "everything") the published bound
        bf = F.get("bound")
        res["feasible_sizes"] = [len([j for j in base[i] if bf is None or bf(t, cands[j]) > -1e17]) for i, t in enumerate(targets)]
        for fname, fn in F.items():
            per = []
            for i, t in enumerate(targets):
                vals = {j: fn(t, cands[j]) for j in base[i]}
                if fname == "bound":
                    keep = [j for j, v in vals.items() if v > -1e17]
                else:
                    best = max(vals.values()) if vals else 0
                    keep = [j for j, v in vals.items() if v == best] if vals and any(v != best for v in vals.values()) else list(vals)
                per.append({"set": len(keep), "of": len(vals), "guess": keep[0] if len(keep) == 1 else None, "keep": keep})
            res["features"][fname] = per
        for mode in ("allowed", "everything"):
            use = [f for f in F if mode == "everything" or f in ALLOWED]

            def score(t, c, use=use):
                if not hard(t, c):
                    return -1e18
                s = 0.0
                for f in use:
                    v = F[f](t, c)
                    if v <= -1e17:
                        return -1e18
                    s += W[f] * v
                if extra and mode == "everything":
                    s += W["composed"] * extra(t, c)
                return s
            a = solve(targets, cands, score)
            res["pairing"][mode] = {str(i): a.get(i) for i in range(len(targets))}
        return res

    R = {"mode": json.load(open(os.path.join(cap, "meta.json"))).get("mode"), "counts": {"deposits": len(deposits), "claims": len(claims),
         "burns": len(burns), "releases": len(releases), "updates": len(updates), "lez_blocks": len(blocks)}, "constants": len(consts)}
    R["inbound"] = run("inbound", claims, deposits, hard_in, F_in)
    R["outbound"] = run("outbound", releases, burns, hard_out, F_out)
    R["lez"] = run("lez", burns, claims, hard_lez, F_lez)
    pin, pout, plez = (R[k]["pairing"]["everything"] for k in ("inbound", "outbound", "lez"))

    def composed(r, d):
        i = releases.index(r)
        b = pout.get(str(i))
        c = plez.get(str(b)) if b is not None else None
        dd = pin.get(str(c)) if c is not None else None
        return int(dd is not None and deposits[dd] is d)
    R["roundtrip"] = run("roundtrip", releases, deposits, hard_rt, F_rt, extra=composed)
    os.makedirs(out, exist_ok=True)
    json.dump(R, open(os.path.join(out, "adversary.json"), "w"), indent=1)
    print(json.dumps(R["counts"]))
    return R


def describe(e):
    d = {"kind": e.get("kind"), "token": e["token"], "di": e["di"], "time": e["time"]}
    for k in ("tx", "block", "leaf_index", "from", "recipient", "relayer"):
        if k in e:
            d[k] = e[k]
    if "C" in e:
        d["C"] = "0x" + e["C"].hex()
    if "E" in e:
        d["E"] = "0x" + e["E"].hex()
    if "roots" in e:
        d["roots_inferred"] = ["0x" + x.hex() for x in e["roots"]]
    if "size" in e:
        d["bytes"] = e["size"]
    return d


if __name__ == "__main__":
    analyse(sys.argv[1], sys.argv[2])
