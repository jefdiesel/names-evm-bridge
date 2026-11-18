#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Re-check the public-network rows of docs/EVIDENCE.md against public endpoints, read only.

Run it through scripts/verify-evidence.sh (see its header). Expected values: docs/evidence-checks.json.
Python 3.8+ standard library and curl only; no key, no account, no build. Every answer is reduced to
the fields a check reads ("normalised"), so a saved set (--save DIR) replays exactly (--offline DIR).
"""
import argparse
import base64
import datetime
import hashlib
import json
import os
import subprocess
import sys
import time

SLOTS_PER_PERIOD = 8192  # Sepolia: 32 slots an epoch, 256 epochs a sync-committee period
ANSWERS = "answers.json"


# ------------------------------------------------------------------------------------ keccak-256
# Ethereum's Keccak-256 (not hashlib's SHA3-256), for function selectors and event topics.
_RC = [0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000,
       0x000000000000808B, 0x0000000080000001, 0x8000000080008081, 0x8000000000008009,
       0x000000000000008A, 0x0000000000000088, 0x0000000080008009, 0x000000008000000A,
       0x000000008000808B, 0x800000000000008B, 0x8000000000008089, 0x8000000000008003,
       0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
       0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008]
_ROT = [[0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61], [28, 55, 25, 21, 56], [27, 20, 39, 8, 14]]
_M = (1 << 64) - 1


def _rol(v, n):
    return ((v << n) | (v >> (64 - n))) & _M if n else v


def _keccak_f(a):
    for rc in _RC:
        c = [a[x][0] ^ a[x][1] ^ a[x][2] ^ a[x][3] ^ a[x][4] for x in range(5)]
        d = [c[(x - 1) % 5] ^ _rol(c[(x + 1) % 5], 1) for x in range(5)]
        a = [[a[x][y] ^ d[x] for y in range(5)] for x in range(5)]
        b = [[0] * 5 for _ in range(5)]
        for x in range(5):
            for y in range(5):
                b[y][(2 * x + 3 * y) % 5] = _rol(a[x][y], _ROT[x][y])
        a = [[b[x][y] ^ ((~b[(x + 1) % 5][y]) & b[(x + 2) % 5][y]) for y in range(5)] for x in range(5)]
        a[0][0] ^= rc
    return a


def keccak256(data):
    rate = 136
    p = bytearray(data) + b"\x01"
    while len(p) % rate:
        p.append(0)
    p[-1] |= 0x80
    a = [[0] * 5 for _ in range(5)]
    for off in range(0, len(p), rate):
        for i in range(rate // 8):
            a[i % 5][i // 5] ^= int.from_bytes(p[off + 8 * i:off + 8 * i + 8], "little")
        a = _keccak_f(a)
    return b"".join(a[i % 5][i // 5].to_bytes(8, "little") for i in range(4))


assert keccak256(b"").hex() == "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
assert keccak256(b"transfer(address,uint256)")[:4].hex() == "a9059cbb"


# ------------------------------------------------------------------------------- base58 and PDAs
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58d(s):
    n = 0
    for ch in s:
        n = n * 58 + B58.index(ch)
    raw = n.to_bytes((n.bit_length() + 7) // 8, "big")
    out = b"\0" * (len(s) - len(s.lstrip("1"))) + raw
    if len(out) != 32:
        raise ValueError("not a 32-byte account id: " + s)
    return out


def b58e(b):
    n = int.from_bytes(b, "big")
    s = ""
    while n:
        n, r = divmod(n, 58)
        s = B58[r] + s
    return "1" * (len(b) - len(b.lstrip(b"\0"))) + s


def sha256(*parts):
    return hashlib.sha256(b"".join(parts)).digest()


def store_pda(program, instance_hex):
    # LEZ rc3: PDA = SHA256("/LEE/v0.2/AccountId/PDA/" | 8 zero bytes | program | seed); the RFP-021
    # store seed is SHA256("/RFP021/attest/store/v1" | instance) (lez-bridge/attest/core).
    seed = sha256(b"/RFP021/attest/store/v1", bytes.fromhex(instance_hex[2:]))
    return b58e(sha256(b"/LEE/v0.2/AccountId/PDA/" + b"\0" * 8, b58d(program), seed))


# ----------------------------------------------------------------------------------- transports
class NoAnswer(Exception):
    pass


class Online:
    """Asks public endpoints through curl, rate-limited; remembers every normalised answer."""

    def __init__(self, sepolia, lez):
        self.sepolia, self.lez = sepolia, lez
        self.last = {}
        self.misses = {}
        self.saved = {}

    def _wait(self, host, gap):
        t = self.last.get(host, 0) + gap - time.monotonic()
        if t > 0:
            time.sleep(t)
        self.last[host] = time.monotonic()

    def _curl(self, url, body, gap):
        """POST body (or GET when None) -> (http status, text). Retries 429/5xx/no connection twice."""
        why = ""
        for attempt in range(3):
            self._wait(url, gap)
            cmd = ["curl", "-sS", "-m", "40", "-w", "\n%{http_code}"]
            if body is not None:
                cmd += ["-H", "Content-Type: application/json", "--data-binary", "@-"]
            p = subprocess.run(cmd + [url], input=body or "", capture_output=True, text=True)
            text, _, code = p.stdout.rpartition("\n")
            code = int(code) if code.isdigit() else 0
            if p.returncode == 0 and code and code != 429 and code < 500:
                return code, text
            why = "HTTP %s" % code if code else (p.stderr.strip() or "curl exit %d" % p.returncode)
            time.sleep(2 * (attempt + 1))
        raise NoAnswer(why)

    def _rpc(self, url, method, params, gap):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
        code, text = self._curl(url, body, gap)
        if code != 200:
            raise NoAnswer("HTTP %d" % code)
        try:
            return json.loads(text)
        except ValueError:
            raise NoAnswer("not JSON")

    def ask(self, kind, method, params):
        key = request_key(kind, method, params)
        if key in self.saved:
            return self.saved[key]["answer"]
        if kind == "sepolia":
            tried = []
            for url in self.sepolia:
                if self.misses.get(url, 0) >= 3 and url != self.sepolia[-1]:
                    tried.append("%s: skipped after 3 failures in a row" % host_of(url))
                    continue
                try:
                    ans = normalise(kind, method, self._rpc(url, method, params, 0.15))
                except NoAnswer as e:
                    self.misses[url] = self.misses.get(url, 0) + 1
                    tried.append("%s: %s" % (host_of(url), e))
                    continue
                self.misses[url] = 0
                self.saved[key] = {"answer": ans, "by": url}
                return ans
            raise NoAnswer("; ".join(tried))
        if kind == "lez":
            ans = normalise(kind, method, self._rpc(self.lez, method, params, 0.25))
            self.saved[key] = {"answer": ans, "by": self.lez}
            return ans
        if kind == "http":
            code, text = self._curl(params[0], None, 0.5)
            ans = normalise(kind, method, (code, text))
            self.saved[key] = {"answer": ans, "by": params[0]}
            return ans
        raise ValueError(kind)


class Offline:
    """Replays a saved set. It has no way to reach the network."""

    def __init__(self, path):
        with open(path, encoding="utf-8") as f:
            self.saved = json.load(f)["answers"]

    def ask(self, kind, method, params):
        key = request_key(kind, method, params)
        if key not in self.saved:
            raise NoAnswer("no saved answer for " + key)
        return self.saved[key]["answer"]


def host_of(url):
    return url.split("//", 1)[-1].split("/", 1)[0]


def request_key(kind, method, params):
    return "%s %s %s" % (kind, method, json.dumps(params, separators=(",", ":")))


def lower(v):
    return v.lower() if isinstance(v, str) else v


def normalise(kind, method, resp):
    """Reduce an endpoint's answer to what the checks read. Raises NoAnswer for 'try elsewhere'."""
    if kind == "http":
        code, text = resp
        title = ""
        lo = text.lower()
        i, j = lo.find("<title>"), lo.find("</title>")
        if 0 <= i < j:
            title = text[i + 7:j].strip()
        return {"status": code, "title": title, "sha256": hashlib.sha256(text.encode("utf-8")).hexdigest()}
    if "error" in resp:
        err = resp["error"] or {}
        msg = str(err.get("message", ""))
        if kind == "sepolia" and method == "eth_call" and (err.get("code") == 3 or "revert" in msg.lower()):
            data = err.get("data")
            if isinstance(data, dict):
                data = data.get("data")
            return {"reverted": True, "data": data if isinstance(data, str) else None}
        raise NoAnswer("error %s: %s" % (err.get("code"), msg[:120]))
    r = resp.get("result")
    if kind == "sepolia":
        if method == "eth_chainId":
            return {"chain_id": int(r, 16)}
        if method == "eth_call":
            return {"reverted": False, "result": lower(r)}
        if r is None:  # a pruned or lagging node: ask the next one
            raise NoAnswer("null result")
        if method == "eth_getTransactionReceipt":
            return {"status": int(r["status"], 16), "block": int(r["blockNumber"], 16), "from": lower(r["from"]),
                    "to": lower(r.get("to")), "created": lower(r.get("contractAddress")),
                    "logs": [[lower(lg["address"]), lower((lg.get("topics") or [None])[0])] for lg in r["logs"]]}
        if method == "eth_getBlockByNumber":
            return {"block": int(r["number"], 16), "hash": lower(r["hash"]), "timestamp": int(r["timestamp"], 16)}
        if method == "eth_getTransactionByHash":
            return {"from": lower(r["from"]), "to": lower(r.get("to")), "input": lower(r["input"])}
    if kind == "lez":
        if method == "getTransaction":
            if not r:
                return {"missing": True}
            raw = base64.b64decode(r[0])
            return {"block": int(r[1]), "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}
        if method == "getAccount":
            states = ((r or {}).get("data") or {}).get("actor_states") or {}
            return {"actor_states": {k: bytes(v).hex() for k, v in states.items()}}
    raise ValueError("no normaliser for %s %s" % (kind, method))


# ------------------------------------------------------------------------------------- the checks
class Run:
    def __init__(self, t, root, data):
        self.t, self.root, self.data = t, root, data
        self.n = {"PASS": 0, "FAIL": 0, "ERROR": 0}

    def out(self, status, row, text):
        if status in self.n:
            self.n[status] += 1
        print("%-5s [%s] %s" % (status, row, text))

    def check(self, ok, row, text, why=""):
        self.out("PASS" if ok else "FAIL", row, text + ("" if ok or not why else ": " + why))

    def call(self, to, sig, args, returns, block=None):
        """eth_call at "latest", or at a fixed block for a point-in-time read (needs an endpoint that serves old state)."""
        data = "0x" + keccak256(sig.encode()).hex()[:8] + "".join(a[2:].rjust(64, "0") for a in args)
        tag = "latest" if block is None else hex(block)
        ans = self.t.ask("sepolia", "eth_call", [{"to": to.lower(), "data": data}, tag])
        if ans["reverted"]:
            raise NoAnswer("the call reverted")
        return decode(ans["result"], returns)

    def date_of(self, block):
        b = self.t.ask("sepolia", "eth_getBlockByNumber", [hex(block), False])
        return datetime.datetime.fromtimestamp(b["timestamp"], datetime.timezone.utc).strftime("%Y-%m-%d")

    # --- one method per check type; each prints its own lines
    def c_chain_id(self, c):
        got = self.t.ask("sepolia", "eth_chainId", [])["chain_id"]
        self.check(got == c["expect"], c["row"], "%s (%d)" % (c["label"], got))

    def c_call(self, c):
        got = self.call(c["to"], c["sig"], c.get("args", []), c["returns"], c.get("block"))
        want = c["expect"]
        cmp_ = c.get("compare", "eq")
        if isinstance(want, str):
            want = want.lower()
        shown = fmt(got) + (" at Sepolia block %d" % c["block"] if "block" in c else "")
        if cmp_ == "info":
            same = "the recorded value" if got == want else "not the recorded %s" % fmt(want)
            self.out("INFO", c["row"], "%s: %s (%s)" % (c["label"], shown, same))
        elif cmp_ == "ne":
            self.check(got != want, c["row"], "%s: %s, not %s" % (c["label"], shown, fmt(want)))
        elif cmp_ == "ge":
            self.check(got >= want, c["row"], "%s: %s (recorded floor %s)" % (c["label"], shown, fmt(want)))
        else:
            self.check(got == want, c["row"], "%s: %s" % (c["label"], shown), "expected %s" % fmt(want))

    def c_tx(self, c):
        r = self.t.ask("sepolia", "eth_getTransactionReceipt", [c["tx"]])
        bad = []
        if r["status"] != 1:
            bad.append("status %d" % r["status"])
        if r["block"] != c["block"]:
            bad.append("block %d, expected %d" % (r["block"], c["block"]))
        if "to" in c and r["to"] != c["to"].lower():
            bad.append("to %s, expected %s" % (r["to"], c["to"]))
        if "creates" in c and r["created"] != c["creates"].lower():
            bad.append("created %s, expected %s" % (r["created"], c["creates"]))
        if "from" in c and r["from"] != c["from"].lower():
            bad.append("from %s, expected %s" % (r["from"], c["from"]))
        if "event" in c:
            topic = "0x" + keccak256(c["event"]["sig"].encode()).hex()
            if [c["event"]["from"].lower(), topic] not in r["logs"]:
                bad.append("no %s event from %s" % (c["event"]["sig"].split("(")[0], short(c["event"]["from"].lower())))
        parts = ["status %d" % r["status"], "block %d" % r["block"]]
        if "to" in c:
            parts.append("to " + short(r["to"]))
        if "creates" in c:
            parts.append("creates " + short(r["created"]))
        if "from" in c:
            parts.append("from " + short(r["from"]))
        if "event" in c:
            parts.append("%s event from %s" % (c["event"]["sig"].split("(")[0], short(c["event"]["from"].lower())))
        if "date" in c:
            d = self.date_of(r["block"])
            parts.append(d + " UTC")
            if d != c["date"]:
                bad.append("block date %s, the row says %s" % (d, c["date"]))
        self.check(not bad, c["row"], "%s: tx %s %s" % (c["label"], short(c["tx"]), ", ".join(parts)), "; ".join(bad))

    def _receipt_files(self, rel):
        d = os.path.join(self.root, rel)

        def rd(name):
            with open(os.path.join(d, name), encoding="utf-8") as f:
                return "".join(f.read().split())
        seal, image, journal = rd("seal.hex"), rd("image-id.txt"), rd("journal.hex")
        digest = "0x" + hashlib.sha256(bytes.fromhex(journal[2:] if journal.startswith("0x") else journal)).hexdigest()
        return seal.lower(), image.lower(), digest

    def _verify(self, verifier, seal, image, digest):
        s = bytes.fromhex(seal[2:])
        data = ("0x" + keccak256(b"verify(bytes,bytes32,bytes32)").hex()[:8] + "%064x" % 0x60 + image[2:] + digest[2:]
                + "%064x" % len(s) + (s + b"\0" * (-len(s) % 32)).hex())
        return self.t.ask("sepolia", "eth_call", [{"to": verifier.lower(), "data": data}, "latest"])

    def c_seal(self, c):
        seal, image, digest = self._receipt_files(c["receipt"])
        ans = self._verify(c["verifier"], seal, image, digest)
        self.check(not ans["reverted"], c["row"], "%s: RISC Zero 3.1.0 verify(seal, image %s, journal digest %s) accepts the committed seal (%s)"
                   % (c["label"], short(image), short(digest), c["receipt"].rsplit("/", 1)[-1]), "reverted %s" % ans.get("data"))
        tx = self.t.ask("sepolia", "eth_getTransactionByHash", [c["tx"]])
        self.check(seal[2:] in tx["input"], c["row"], "%s: tx %s carries that seal" % (c["label"], short(c["tx"])))

    def c_seal_changed(self, c):
        seal, image, digest = self._receipt_files(c["receipt"])
        changed = seal[:-2] + "%02x" % (int(seal[-2:], 16) ^ 1)
        ans = self._verify(c["verifier"], changed, image, digest)
        self.check(ans["reverted"], c["row"], "%s: refused (reverted%s)" % (c["label"], ", data " + ans["data"] if ans.get("data") else ""),
                   "it was accepted")

    def c_block_hash(self, c):
        b = self.t.ask("sepolia", "eth_getBlockByNumber", [hex(c["block"]), False])
        h = b["hash"]
        self.check(h.startswith(c["prefix"].lower()) and h.endswith(c["suffix"].lower()), c["row"],
                   "%s: hash %s (recorded %s…%s)" % (c["label"], h, c["prefix"], c["suffix"]))

    def c_lez_tx(self, c):
        r = self.t.ask("lez", "getTransaction", [c["tx"]])
        if r.get("missing"):
            self.check(False, c["row"], "%s: tx %s" % (c["label"], c["tx"][:8]), "not found")
            return
        want = c["block"]
        lo, hi = (want, want) if isinstance(want, int) else want
        bad = [] if lo <= r["block"] <= hi else ["expected block %s" % (want if lo == hi else "%d..%d" % (lo, hi))]
        text = "%s: tx %s… in LEZ block %d, %s bytes" % (c["label"], c["tx"][:8], r["block"], "{:,}".format(r["bytes"]))
        if "bytes" in c and r["bytes"] != c["bytes"]:
            bad.append("expected %d bytes" % c["bytes"])
        self.check(not bad, c["row"], text, "; ".join(bad))

    def c_lc_store(self, c):
        row, prog = c["row"], c["program"]
        addr = store_pda(prog, c["instance"])
        if "store" in c:
            self.check(addr == c["store"], row, "%s: the store address derived from program and instance is the recorded %s" % (c["label"], c["store"]),
                       "derived %s" % addr)
        acct = self.t.ask("lez", "getAccount", [addr])
        raw = acct["actor_states"].get(prog)
        if raw is None:
            self.check(False, row, "%s: account %s" % (c["label"], addr), "holds no state of program " + prog)
            return
        try:
            s = parse_lc_summary(bytes.fromhex(raw))
        except ValueError as e:
            self.check(False, row, "%s: account %s" % (c["label"], addr), "does not parse as LcSummary (%s)" % e)
            return
        self.check(s["instance"] == c["instance"].lower() and s["chain_id"] == c["chain_id"], row,
                   "%s %s: instance %s, chain %d" % (c["label"], addr, short(s["instance"]), s["chain_id"]))
        self.check(s["slot"] >= c["min_slot"], row, "%s: head at Sepolia finalized slot %d, period %d (recorded floor: slot %d, period %d)"
                   % (c["label"], s["slot"], s["slot"] // SLOTS_PER_PERIOD, c["min_slot"], c["min_slot"] // SLOTS_PER_PERIOD))
        if "not_current" in c:  # a committee the store held before a handoff: never current again
            self.check(s["current_committee"] != c["not_current"].lower(), row, "%s: current committee %s, no longer %s (the committee before the handoff)"
                       % (c["label"], short(s["current_committee"]), short(c["not_current"].lower())))
        if "in_period" in c:  # committees recorded for one period: compared only while the head is in it
            p = c["in_period"]
            want = (p["current"].lower(), p["next"].lower())
            got = (s["current_committee"], s["next_committee"])
            head = s["slot"] // SLOTS_PER_PERIOD
            if head == p["period"]:
                self.check(got == want, row, "%s: in period %d, current committee %s, next %s" % (c["label"], p["period"], short(got[0]), short(got[1])),
                           "recorded %s and %s" % (short(want[0]), short(want[1])))
            else:  # the head only moves forward; past the period the recorded pair is history
                self.check(head > p["period"], row, "%s: the head has moved past period %d (now %d); recorded for %d: current committee %s, next %s"
                           % (c["label"], p["period"], head, p["period"], short(want[0]), short(want[1])))
        anchor = "anchor block %d" % s["anchor_number"] if s["anchor_number"] is not None else "anchor block number not stored"
        self.out("INFO", row, "%s: %d updates, %s, current committee %s, next committee %s" % (
            c["label"], s["updates"], anchor, short(s["current_committee"]), short(s["next_committee"]) if s["next_committee"] else "not known"))

    def c_lez_nullifier(self, c):
        """A bridge claim nullifier spent: its PDA (seed SHA256(NULLIFIER_SEED_DOMAIN | nf), lez-bridge/bridge/src/lib.rs)
        holds the bridge program's marker state."""
        prog = c["program"]
        seed = sha256(b"/RFP022/bridge/claim-nullifier/v1", bytes.fromhex(c["nullifier"][2:]))
        addr = b58e(sha256(b"/LEE/v0.2/AccountId/PDA/" + b"\0" * 8, b58d(prog), seed))
        raw = self.t.ask("lez", "getAccount", [addr])["actor_states"].get(prog)
        self.check(raw == "01", c["row"], "%s: nullifier %s, PDA %s holds %s" % (c["label"], short(c["nullifier"]), addr,
                   "the bridge's spent marker" if raw == "01" else "state %s" % raw), "expected the spent marker 01")

    def c_page(self, c):
        p = self.t.ask("http", "GET", [c["url"]])
        self.check(p["status"] == c["status"] and p["title"] == c["title"], c["row"],
                   "%s %s: HTTP %d, title \"%s\"" % (c["label"], c["url"], p["status"], p["title"]))
        mf = os.path.join(self.root, c["manifest"])
        want = ""
        if os.path.exists(mf):
            with open(mf, encoding="utf-8") as f:
                for line in f:
                    parts = line.split()
                    if len(parts) == 2 and parts[1] == "index.html":
                        want = parts[0]
        same = "equals" if p["sha256"] == want else "differs from"
        self.out("INFO", c["row"], "the served index.html %s the checkout's %s entry" % (same, c["manifest"]))


def parse_lc_summary(b):
    """LcSummary (borsh, lez-bridge/attest/core): instance | chain u64 | Finalized{BeaconHeader{slot u64,
    proposer u64, parent, state, body}, beacon_root, ExecutionAnchor{block_hash, Option<state_root>,
    Option<number u64>}} | current_committee | Option<next_committee> | updates u64."""
    pos = [0]

    def take(n):
        if pos[0] + n > len(b):
            raise ValueError("short at byte %d" % pos[0])
        v = b[pos[0]:pos[0] + n]
        pos[0] += n
        return v

    def u64():
        return int.from_bytes(take(8), "little")

    def option(read):
        tag = take(1)[0]
        if tag not in (0, 1):
            raise ValueError("bad Option tag %d" % tag)
        return read() if tag else None
    s = {"instance": "0x" + take(32).hex(), "chain_id": u64(), "slot": u64()}
    u64()
    take(96)       # parent, state and body roots
    take(32)       # beacon root
    take(32)       # execution block hash
    option(lambda: take(32))
    s["anchor_number"] = option(u64)
    s["current_committee"] = "0x" + take(32).hex()
    nxt = option(lambda: take(32))
    s["next_committee"] = "0x" + nxt.hex() if nxt is not None else None
    s["updates"] = u64()
    if pos[0] != len(b):
        raise ValueError("%d bytes left over" % (len(b) - pos[0]))
    return s


def decode(result, returns):
    h = result[2:]
    if returns == "address":
        return "0x" + h[24:64]
    if returns == "bytes32":
        return "0x" + h[:64]
    if returns == "bytes4":
        return "0x" + h[:8]
    if returns == "uint":
        return int(h[:64], 16)
    if returns == "bool":
        return int(h[:64], 16) == 1
    if returns == "string":
        off = int(h[:64], 16) * 2
        n = int(h[off:off + 64], 16)
        return bytes.fromhex(h[off + 64:off + 64 + 2 * n]).decode("utf-8")
    raise ValueError(returns)


def fmt(v):
    return v if isinstance(v, str) else json.dumps(v)


def short(v):
    if not v:
        return str(v)
    return v[:10] + "…" + v[-4:] if len(v) > 20 else v


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--root", required=True)
    g = ap.add_mutually_exclusive_group()
    g.add_argument("--offline", metavar="DIR", help="replay the answers saved in DIR/" + ANSWERS + "; no network")
    g.add_argument("--save", metavar="DIR", help="ask the endpoints and save their answers to DIR/" + ANSWERS)
    a = ap.parse_args()
    with open(os.path.join(a.root, "docs", "evidence-checks.json"), encoding="utf-8") as f:
        data = json.load(f)
    if a.offline:
        if not os.path.isfile(os.path.join(a.offline, ANSWERS)):
            print("no %s in %s (a saved set, e.g. docs/evidence-answers)" % (ANSWERS, a.offline), file=sys.stderr)
            return 2
        t = Offline(os.path.join(a.offline, ANSWERS))
        print("offline: replaying %s (no network)" % os.path.join(a.offline, ANSWERS))
    else:
        sep = [u.strip() for u in os.environ.get("SEPOLIA_RPC", "").split(",") if u.strip()] or data["endpoints"]["sepolia"]
        lez = os.environ.get("LEZ_RPC", "").strip() or data["endpoints"]["lez"]
        t = Online(sep, lez)
        print("online: Sepolia %s; LEZ %s" % (" then ".join(host_of(u) for u in sep), host_of(lez)))
    run = Run(t, a.root, data)
    row = None
    for c in data["checks"]:
        if c["row"] != row:
            row = c["row"]
            print("\n== %s: %s" % (row, data["rows"][row]))
        try:
            getattr(run, "c_" + c["type"])(c)
        except NoAnswer as e:
            run.out("ERROR", c["row"], "%s: no answer (%s)" % (c["label"], e))
    n = run.n
    print("\n%d checks: %d PASS, %d FAIL, %d ERROR (no answer)" % (sum(n.values()), n["PASS"], n["FAIL"], n["ERROR"]))
    if a.save:
        os.makedirs(a.save, exist_ok=True)
        out = {"about": "Normalised answers of the public endpoints to scripts/verify-evidence.sh; replay with --offline.",
               "saved_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
               "answers": dict(sorted(t.saved.items()))}
        with open(os.path.join(a.save, ANSWERS), "w", encoding="utf-8") as f:
            json.dump(out, f, indent=1, ensure_ascii=False)
            f.write("\n")
        print("saved %d answers to %s" % (len(t.saved), os.path.join(a.save, ANSWERS)))
    return 0 if n["FAIL"] == 0 and n["ERROR"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
