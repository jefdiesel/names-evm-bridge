#!/usr/bin/python3
"""Offline tests for leaves.py (stdlib unittest): fake Sepolia RPCs on loopback that prune old
history, drop old logs without an error, refuse large ranges or rate-limit. Synthetic deposits.

  python3 -I public-demo/attester/test_leaves.py      # from the repository root
"""
import importlib.util, json, os, subprocess, sys, threading, unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
os.environ.update(FROM_BLOCK="1000", RPC_RETRIES="3", LOG_CHUNK="1000")
spec = importlib.util.spec_from_file_location("leaves", os.path.join(HERE, "leaves.py"))
lv = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lv)
lv.time.sleep = lambda s: None   # no real pauses in the tests

ETH0 = "00" * 20
HEAD, FINAL = 20000, 19930
# Synthetic deposits (block, token, denomination index, commitment): five just after the vault's
# creation, three much later.
DEPOSITS = [(b, ETH0, i % 3, bytes([i + 1]) * 32) for i, b in enumerate([1017, 1018, 1019, 1020, 1021, 13996, 16826, 17082])]


def leaf(token, di, c):
    return lv.h(lv.TAG, lv.CFG_ID, bytes.fromhex(token), lv.DENOMS[token][di].to_bytes(32, "big"), c)


def w(n):
    return n.to_bytes(32, "big").hex()


def log_of(i):
    b, token, di, c = DEPOSITS[i]
    root = lv.root_of([leaf(*d[1:]) for d in DEPOSITS[: i + 1]])
    return {"address": lv.VAULT.lower(), "blockNumber": hex(b), "removed": False,
            "topics": [lv.DEPOSIT_TOPIC, "0x" + token.rjust(64, "0")],
            "data": "0x" + w(di) + c.hex() + w(i) + root.hex()}


class Fake:
    """mode: full | pruned (refuses ranges below KEPT) | drops (answers without logs below KEPT)
    | narrow (refuses more than 1,000 blocks, rate-limits every third call) | lies (no error,
    but the vault counts one deposit more than the logs show at the head)."""
    KEPT = 10000

    def __init__(self, mode):
        self.mode, self.calls = mode, 0

    def answer(self, method, params):
        self.calls += 1
        if method == "eth_getBlockByNumber":
            n = FINAL if params[0] == "finalized" else HEAD
            return {"result": {"number": hex(n)}}
        if method == "eth_call":
            block = int(params[1], 16)
            n = sum(1 for d in DEPOSITS if d[0] <= block)
            if params[0]["data"] == lv.SEL_DEPOSIT_COUNT:
                return {"result": "0x" + w(n + (1 if self.mode == "lies" else 0))}
            return {"result": "0x" + lv.root_of([leaf(*d[1:]) for d in DEPOSITS[:n]]).hex()}
        if method == "eth_getLogs":
            f = params[0]
            lo, hi = int(f["fromBlock"], 16), int(f["toBlock"], 16)
            if self.mode == "pruned" and lo < self.KEPT:
                return {"error": {"code": 4444, "message": "pruned history unavailable"}}
            if self.mode == "narrow":
                if hi - lo + 1 > 1000:
                    return {"error": {"code": -32005, "message": "Log response size exceeded. Maximum allowed number of requested blocks is 1000"}}
                if self.calls % 3 == 0:
                    return {"error": {"code": -32005, "message": "Rate limit exceeded. Upgrade your plan"}}
            floor = self.KEPT if self.mode == "drops" else 0
            return {"result": [log_of(i) for i, d in enumerate(DEPOSITS) if lo <= d[0] <= hi and d[0] >= floor]}
        return {"error": {"code": -32601, "message": "no such method"}}


def serve(fake):
    class H(BaseHTTPRequestHandler):
        def do_POST(self):
            req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            body = json.dumps(dict(fake.answer(req["method"], req["params"]), jsonrpc="2.0", id=req["id"])).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a):
            pass

    srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, "http://127.0.0.1:%d/" % srv.server_address[1]


class Leaves(unittest.TestCase):
    def setUp(self):
        self.servers = []

    def tearDown(self):
        for s in self.servers:
            s.shutdown()
            s.server_close()

    def url(self, mode):
        fake = Fake(mode)
        srv, url = serve(fake)
        self.servers.append(srv)
        return fake, url

    def want(self, upto):
        return [leaf(*d[1:]) for d in DEPOSITS if d[0] <= upto]

    def test_full_history(self):
        _, url = self.url("full")
        leaves, root, block = lv.leaves_via(url, "finalized")
        self.assertEqual((leaves, block), (self.want(FINAL), FINAL))
        self.assertEqual(root, "0x" + lv.root_of(leaves).hex())

    def test_pruned_history_is_an_error_naming_it(self):
        fake, url = self.url("pruned")
        with self.assertRaises(lv.RpcError) as e:
            lv.leaves_via(url, "finalized")
        self.assertEqual(e.exception.kind, "pruned")
        self.assertIn("pruned history unavailable", str(e.exception))
        self.assertEqual(fake.calls, 2, "one block read, one log read: no retries against pruned history")

    def test_silently_missing_logs_are_refused(self):
        _, url = self.url("drops")
        with self.assertRaises(ValueError) as e:
            lv.leaves_via(url, "finalized")
        self.assertIn("skip from leaf 0 to leaf 5", str(e.exception))

    def test_a_count_the_logs_do_not_reach_is_refused(self):
        _, url = self.url("lies")
        with self.assertRaises(ValueError) as e:
            lv.leaves_via(url, "finalized")
        self.assertIn("depositCount() at block 19930 is 9", str(e.exception))

    def test_narrow_and_rate_limited_provider_is_read_in_windows(self):
        fake, url = self.url("narrow")
        leaves, _, _ = lv.leaves_via(url, "latest")
        self.assertEqual(leaves, self.want(HEAD))
        self.assertGreater(fake.calls, (HEAD - 1000) // 1000)

    def run_cli(self, rpcs):
        env = dict(os.environ, SEPOLIA_RPC=",".join(rpcs), PYTHONDONTWRITEBYTECODE="1")
        return subprocess.run([sys.executable, "-I", os.path.join(HERE, "leaves.py")], env=env, capture_output=True, text=True, timeout=60)

    def test_falls_back_in_order_and_says_so_after_the_summary(self):
        _, pruned = self.url("pruned")
        _, drops = self.url("drops")
        _, full = self.url("full")
        r = self.run_cli([pruned, drops, full])
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stdout.split(), ["0x" + x.hex() for x in self.want(FINAL)])
        err = r.stderr.splitlines()
        self.assertTrue(err[0].startswith("# 8 leaves, root 0x") and err[0].endswith("via " + full), err[0])
        self.assertIn("# skipped " + pruned, err[1])
        self.assertIn("# skipped " + drops, err[2])

    def test_no_complete_rpc_prints_nothing(self):
        _, pruned = self.url("pruned")
        _, drops = self.url("drops")
        r = self.run_cli([pruned, drops])
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(r.stdout, "")
        self.assertIn("no Sepolia RPC gave the vault's complete deposit history", r.stderr)
        self.assertIn(pruned, r.stderr)
        self.assertIn(drops, r.stderr)


if __name__ == "__main__":
    unittest.main()
