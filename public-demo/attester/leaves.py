#!/usr/bin/python3
"""Print the Sepolia v2 vault's deposit leaves in order (one 0x-hex leaf per line), the input
`lez-bridge claim --secret ... --leaves FILE --index N` needs. Rebuilt from the vault's Deposit
logs up to a block tag (default `finalized`), and checked against the vault's depositCount() and
depositRoot() at that block before anything is printed. Read-only; stdlib only.

  leaves.py [--block finalized|latest|0xNUMBER] > leaves.txt

SEPOLIA_RPC: one RPC, or several separated by commas, tried in order until one gives every
deposit the vault counts. The RPC must serve eth_getLogs back to the vault's creation block
(FROM_BLOCK). PublicNode's Sepolia RPC no longer does (2026-11-14: "pruned history unavailable"
on most requests reaching back that far); the default list starts with Tenderly's public gateway
(the whole range in one call) and falls back to thirdweb's (1,000-block windows, rate-limited).
A set of logs shorter than the vault's count is refused, never printed.

stderr: the first line is the summary ("# N leaves, root 0x..., Sepolia block B (tag) via URL");
notes on RPCs that failed before follow it.

leaf = sha256(DEPOSIT_LEAF_TAG | cfgId | token | uint256(denom) | commitment)
root = the vault's depth-32 sha256 Merkle tree over the leaves (lez-bridge/bridge/src/lib.rs).
"""
import hashlib, json, os, sys, time, urllib.error, urllib.request

DEFAULT_RPCS = "https://sepolia.gateway.tenderly.co,https://11155111.rpc.thirdweb.com"
RPCS = [u.strip() for u in os.environ.get("SEPOLIA_RPC", DEFAULT_RPCS).split(",") if u.strip()]
VAULT = os.environ.get("VAULT", "0x8C8b2664De28e5287158db4DB65D448638871b74")
FROM_BLOCK = int(os.environ.get("FROM_BLOCK", "12122976"))   # v2 vault creation
CHUNK = int(os.environ.get("LOG_CHUNK", "1000"))             # window when a whole-range read is refused
RETRIES = int(os.environ.get("RPC_RETRIES", "6"))            # per request, on rate limits and timeouts
CFG_ID = bytes.fromhex("7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11")
TAG = bytes.fromhex("9e8d098bfe4f2807258ba6b200534afcacae50702d4735cb40a3f31e9bea950c")
DENOMS = {  # the vault's registered denominations (vault/DEPLOYMENTS.md, v2)
    "ac207b9bfebf3ef7c26297ea6abadeeccc4aa71e": [10**8, 10**9, 10**10],      # dUSDC, 6 decimals
    "0000000000000000000000000000000000000000": [10**16, 10**17, 10**18],    # ETH
}
DEPOSIT_TOPIC = "0x7c6270dc673eb588f0d459a71fcc44183d7b70b659eb552028fa91fadbe8ea69"
SEL_DEPOSIT_ROOT = "0x8955fe8a"    # depositRoot()
SEL_DEPOSIT_COUNT = "0x2dfdf0b5"   # depositCount()
DEPTH = 32


class RpcError(Exception):
    """kind: 'pruned' (old history gone: try another RPC), 'range' (split the range),
    'busy' (rate limit, timeout, 5xx: wait and retry) or 'other'."""

    def __init__(self, kind, msg):
        Exception.__init__(self, msg)
        self.kind = kind


def kind_of(message):
    m = message.lower()
    if "pruned" in m or "history unavailable" in m:
        return "pruned"
    if "rate limit" in m or "too many requests" in m:
        return "busy"
    if any(w in m for w in ("range", "too many", "more than", "limit exceeded", "-32005", "response size", "too large", "maximum allowed")):
        return "range"
    return "other"


def rpc_once(url, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(url, body, {"Content-Type": "application/json", "User-Agent": "lez-demo-leaves/2"})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            out = json.load(r)
    except urllib.error.HTTPError as e:
        text = e.read()[:300].decode("utf-8", "replace")
        try:
            out = json.loads(text)
        except ValueError:
            raise RpcError("busy" if e.code == 429 or e.code >= 500 else "other", "%s: HTTP %d %s" % (method, e.code, text.strip()))
        if "error" not in out:
            raise RpcError("busy" if e.code == 429 or e.code >= 500 else "other", "%s: HTTP %d" % (method, e.code))
    except (urllib.error.URLError, OSError, ValueError) as e:
        raise RpcError("busy", "%s: %s" % (method, e))
    if "error" in out:
        msg = "%s: %s" % (method, json.dumps(out["error"]))
        raise RpcError(kind_of(msg), msg)
    if "result" not in out:
        raise RpcError("other", "%s: no result" % method)
    return out["result"]


def rpc(url, method, params):
    """One request, retried with a growing pause on rate limits and transport trouble."""
    for attempt in range(RETRIES + 1):
        try:
            return rpc_once(url, method, params)
        except RpcError as e:
            if e.kind != "busy" or attempt == RETRIES:
                raise
            time.sleep(min(8.0, 0.5 * 2 ** attempt))


def deposit_logs(url, start, end):
    """Every Deposit log of the vault in start..end: the whole range at once, else CHUNK-block
    windows (thirdweb refuses more than 1,000 blocks). Pruned history raises."""
    f = {"address": VAULT, "topics": [DEPOSIT_TOPIC]}
    try:
        return rpc(url, "eth_getLogs", [dict(f, fromBlock=hex(start), toBlock=hex(end))])
    except RpcError as e:
        if e.kind != "range":
            raise
    logs, s = [], start
    while s <= end:
        e = min(end, s + CHUNK - 1)
        logs.extend(rpc(url, "eth_getLogs", [dict(f, fromBlock=hex(s), toBlock=hex(e))]))
        s = e + 1
    return logs


def h(*parts):
    return hashlib.sha256(b"".join(parts)).digest()


def root_of(leaves):
    zero = b"\0" * 32
    layer = list(leaves)
    for _ in range(DEPTH):
        layer = [h(layer[i], layer[i + 1] if i + 1 < len(layer) else zero) for i in range(0, len(layer), 2)] or [h(zero, zero)]
        zero = h(zero, zero)
    return layer[0]


def leaves_via(url, tag):
    """(leaves, root hex, block) through one RPC, or RpcError/ValueError saying what is wrong."""
    block = int(rpc(url, "eth_getBlockByNumber", [tag, False])["number"], 16)
    deps = []
    for lg in deposit_logs(url, FROM_BLOCK, block):
        if lg.get("removed") or lg.get("address", VAULT).lower() != VAULT.lower():
            continue
        data = bytes.fromhex(lg["data"][2:])
        if len(lg["topics"]) != 2 or lg["topics"][0].lower() != DEPOSIT_TOPIC or len(data) != 128:
            continue  # not a Deposit event
        token = lg["topics"][1][-40:].lower()
        denom_index = int.from_bytes(data[0:32], "big")
        commitment = data[32:64]
        leaf_index = int.from_bytes(data[64:96], "big")
        deps.append((leaf_index, token, denom_index, commitment))
    deps.sort()
    count = int(rpc(url, "eth_call", [{"to": VAULT, "data": SEL_DEPOSIT_COUNT}, hex(block)]), 16)
    leaves = []
    for i, (leaf_index, token, denom_index, c) in enumerate(deps):
        if leaf_index != i:
            raise ValueError("the logs skip from leaf %d to leaf %d: the RPC left deposit logs out (pruned history?)" % (i, leaf_index))
        denom = DENOMS[token][denom_index]
        leaves.append(h(TAG, CFG_ID, bytes.fromhex(token), denom.to_bytes(32, "big"), c))
    if len(leaves) != count:
        raise ValueError("the logs hold %d deposit(s) but the vault's depositCount() at block %d is %d: %s"
                         % (len(leaves), block, count,
                            "the RPC left deposit logs out" if len(leaves) < count else "the RPC returned logs the vault does not count"))
    want = rpc(url, "eth_call", [{"to": VAULT, "data": SEL_DEPOSIT_ROOT}, hex(block)]).lower()
    got = "0x" + root_of(leaves).hex()
    if got != want:
        raise ValueError("rebuilt root %s != vault depositRoot %s at block %d" % (got, want, block))
    return leaves, got, block


def main():
    tag = "finalized"
    if "--block" in sys.argv:
        tag = sys.argv[sys.argv.index("--block") + 1]
    if not RPCS:
        raise SystemExit("SEPOLIA_RPC is empty")
    failed = []
    for url in RPCS:
        try:
            leaves, root, block = leaves_via(url, tag)
        except (RpcError, ValueError, KeyError, IndexError, TypeError) as e:
            failed.append("%s: %s" % (url, str(e)[:400]))
            continue
        for leaf in leaves:
            print("0x" + leaf.hex())
        print("# %d leaves, root %s, Sepolia block %d (%s) via %s" % (len(leaves), root, block, tag, url), file=sys.stderr)
        for f in failed:
            print("# skipped %s" % f, file=sys.stderr)
        return
    raise SystemExit("no Sepolia RPC gave the vault's complete deposit history (nothing printed):\n  %s\n"
                     "Set SEPOLIA_RPC to an RPC that serves eth_getLogs back to block %d "
                     "(several, comma-separated, are tried in order)." % ("\n  ".join(failed), FROM_BLOCK))


if __name__ == "__main__":
    main()
