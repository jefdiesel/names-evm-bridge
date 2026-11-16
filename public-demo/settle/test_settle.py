#!/usr/bin/python3
"""Offline tests for settle.py (stdlib unittest; stand-in Sepolia, GitHub, prover, cast; real openssl).

  python3 -I public-demo/settle/test_settle.py      # from the repository root
"""
import base64, importlib.util, io, json, os, subprocess, tarfile, tempfile, unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TMP = tempfile.mkdtemp(prefix="settle-test-")
VAULT = "0xafbdf192434e9888bb23dae6f2d15eea6246ed10"
os.environ.update(P=TMP, SETTLE_DIR=TMP + "/settle", SETTLE_LOG=TMP + "/settle.log", VAULTS=VAULT, PER_CLIENT_DAY="2", DAILY_MAX="3",
                  SETTLE_ADDRESS="0x" + "5e" * 20, DRY_RUN="0")
spec = importlib.util.spec_from_file_location("settle", os.path.join(HERE, "settle.py"))
st = importlib.util.module_from_spec(spec)
spec.loader.exec_module(st)
os.makedirs(st.DIR, exist_ok=True)
with open(st.WRAP_KEY_FILE, "w") as f:
    f.write("ab" * 32)
for p in (st.TOKEN_FILE, st.KEYSTORE):
    open(p, "w").close()

CFG = bytes(range(32))
ETH = b"\0" * 20
DENOMS = [10**16, 10**17, 10**18]
DEST = bytes([0x11]) * 20
NOW = 1_800_000_000


class Eth:
    def __init__(self):
        self.recorded, self.spent, self.releasable, self.verify_ok, self.bal, self.t = set(), set(), {}, True, 10**18, NOW

    def call(self, to, data):
        d = st.h2b(data)
        sel, args = d[:4].hex(), d[4:]
        if to == st.VERIFIER:
            if not self.verify_ok:
                raise RuntimeError("reverted: VerificationFailed")
            return b""
        if sel == st.SEL["cfgId"]:
            return CFG
        if sel == st.SEL["exitRootTime"]:
            return st.u256(1 if args[:32] in self.recorded else 0)
        if sel == st.SEL["spent"]:
            return st.u256(1 if args[:32] in self.spent else 0)
        if sel == st.SEL["releasableAt"]:
            return st.u256(self.releasable.get(args[32:64], 0))
        if sel == st.SEL["tokenConfig"]:
            arr = st.u256(len(DENOMS)) + b"".join(st.u256(x) for x in DENOMS)
            return st.u256(1) + st.u256(0) * 4 + st.u256(9 * 32) + st.u256(0) * 3 + arr
        raise AssertionError("unexpected call " + sel)

    def now(self):
        return self.t

    def balance(self, a):
        return self.bal


class GitHub:
    def __init__(self):
        self.dispatched, self.runs = [], {}

    def dispatch(self, fid, digest):
        self.dispatched.append((fid, digest))

    def find_run(self, fid):
        return self.runs.get(fid)

    def seal(self, run_id, fid):
        return "0xe71e6620" + "77" * 256


def exit_material(k, index=1):
    e = st.exit_commitment(k, DEST, ETH, DENOMS[0], 0, CFG)
    leaves = [st.sha(b"other", bytes([i])) for i in range(3)]
    leaves.insert(index, st.exit_leaf(CFG, ETH, DENOMS[0], e))
    return leaves


def journal_of(k, root, relayer=ETH, fee=0):
    return (CFG + root + st.nullifier(k) + b"\0" * 12 + ETH + st.u256(DENOMS[0]) + b"\0" * 12 + DEST + b"\0" * 12 + relayer + st.u256(fee))


class Settle(unittest.TestCase):
    def setUp(self):
        st.W.eth, st.W.gh = Eth(), GitHub()
        st.W.verify = lambda d: True
        st.W.clock = lambda: NOW
        self.sent, self.proved = [], []
        st.W.cast = lambda vault, args: (self.sent.append((vault, args)) or (True, "0x" + "cd" * 32, 9000))
        st.SECRETS.clear()
        self.s = {"jobs": {}, "accepted": [], "clients": {}, "daykeys": {}}

    def prover(self, secret, args, out):
        self.proved.append((secret, args))
        leaves = [st.h2b(x) for x in open(os.path.join(out, "leaves.txt")).read().split()]
        with open(os.path.join(out, "receipt.bin"), "wb") as f:
            f.write(b"r" * 2000)
        with open(os.path.join(out, "journal.hex"), "w") as f:
            f.write("0x" + journal_of(bytes.fromhex(secret), st.tree_root(leaves)).hex())
        return 0, "done"

    def run_all(self, n=8):
        for _ in range(n):
            st.step(self.s)

    def test_a_wallet_without_the_helper_sends_its_exit_and_the_service_does_the_rest(self):
        k = bytes([7]) * 32
        leaves = exit_material(k)
        root = st.tree_root(leaves)
        st.W.eth.recorded.add(root)
        st.W.prover = self.prover
        body = json.dumps({"vault": VAULT, "exit": {"secret": "0x" + k.hex(), "eth_dest": "0x" + DEST.hex(), "token": "0x" + ETH.hex(), "denom": str(DENOMS[0]),
                                                     "max_relay_fee": "0", "cfg_id": "0x" + CFG.hex(), "leaves": ["0x" + x.hex() for x in leaves], "index": 1}})
        out = st.accept(body, "1.2.3.4", self.s)
        jid = "0x" + st.nullifier(k).hex()
        self.assertEqual((out["ok"], out["id"], out["stage"]), (True, jid, "queued"), out)
        on_disk = open(st.STATE).read() + open(os.environ["SETTLE_LOG"]).read()
        self.assertNotIn(k.hex(), on_disk, "the secret is never in the state file or the log")
        st.step(self.s)                                   # proved, then queued for the wrapper
        self.assertEqual(self.proved[0][0], k.hex())
        self.assertIn("/dev/stdin", self.proved[0][1])
        self.assertNotIn(k.hex(), " ".join(self.proved[0][1]), "never on argv")
        self.assertNotIn(jid, st.SECRETS)
        st.step(self.s)                                   # bundle made, workflow dispatched
        j = self.s["jobs"][jid]
        self.assertEqual(j["stage"], "wrapping")
        fid, digest = st.W.gh.dispatched[0]
        self.assertEqual(digest, "0x" + st.sha(journal_of(k, root)).hex())
        served = st.bundle(json.dumps({"fetch_id": fid}), self.s)
        plain = subprocess.run(["openssl", "enc", "-d", "-aes-256-cbc", "-pbkdf2", "-iter", "200000", "-md", "sha256", "-pass", "env:WRAP_KEY"],
                               input=base64.b64decode(served["bundle"]), capture_output=True, env=dict(os.environ, WRAP_KEY="ab" * 32)).stdout
        names = tarfile.open(fileobj=io.BytesIO(plain)).getnames()
        self.assertEqual(sorted(names), ["journal.hex", "receipt.bin"], "the bundle the workflow decrypts")
        self.assertEqual(st.bundle(json.dumps({"fetch_id": "0" * 32}), self.s)["code"], "unknown")
        st.W.gh.runs[fid] = {"id": 77, "status": "completed", "conclusion": "success"}
        st.step(self.s)                                   # the seal, checked on Sepolia
        self.assertEqual(j["stage"], "releasable_at")
        self.assertFalse(os.path.exists(os.path.join(j["dir"], "bundle.bin")), "the bundle is deleted after the run")
        self.assertEqual(st.bundle(json.dumps({"fetch_id": fid}), self.s)["code"], "unknown")
        st.W.eth.releasable[root] = NOW + 3600
        st.step(self.s)
        self.assertEqual(st.status(json.dumps({"id": jid}), self.s)["releasable_at"], NOW + 3600)
        st.W.eth.t = NOW + 3601
        self.run_all(2)
        self.assertEqual(j["stage"], "released")
        vault, args = self.sent[0]
        self.assertEqual((vault, args[1], args[2], args[4], args[5], args[6], args[7]), (VAULT, "0x" + root.hex(), jid, "0", "0x" + DEST.hex(), st.ZERO20, "0"))
        self.assertTrue(args[0].startswith("0xe71e6620"))
        s = st.status(json.dumps({"id": jid}), self.s)
        self.assertEqual((s["stage"], s["tx"], s["block"]), ("released", "0x" + "cd" * 32, 9000))

    def test_a_wallet_with_the_helper_sends_only_the_receipt(self):
        k = bytes([8]) * 32
        root = st.tree_root(exit_material(k))
        st.W.eth.recorded.add(root)
        body = json.dumps({"vault": VAULT, "receipt_hex": "ab" * 3000, "journal_hex": journal_of(k, root).hex()})
        out = st.accept(body, "1.2.3.4", self.s)
        self.assertTrue(out["ok"], out)
        st.step(self.s)
        self.assertEqual(self.s["jobs"][out["id"]]["stage"], "wrapping")
        st.W.verify = lambda d: False
        k2 = bytes([9]) * 32
        root2 = st.tree_root(exit_material(k2))
        st.W.eth.recorded.add(root2)
        bad = st.accept(json.dumps({"vault": VAULT, "receipt_hex": "ab" * 3000, "journal_hex": journal_of(k2, root2).hex()}), "5.6.7.8", self.s)
        self.assertEqual(bad["code"], "bad_request")

    def test_refusals_before_anything_is_kept(self):
        k = bytes([10]) * 32
        leaves = exit_material(k)
        root = st.tree_root(leaves)
        mk = lambda **o: json.dumps({"vault": o.get("vault", VAULT), "exit": {"secret": "0x" + o.get("k", k).hex(), "eth_dest": "0x" + DEST.hex(), "token": "0x" + ETH.hex(),
                                     "denom": str(DENOMS[0]), "max_relay_fee": "0", "cfg_id": "0x" + CFG.hex(), "leaves": ["0x" + x.hex() for x in leaves], "index": o.get("i", 1)}})
        self.assertEqual(st.accept(mk(), "1.1.1.1", self.s)["code"], "not_recorded")
        st.W.eth.recorded.add(root)
        self.assertEqual(st.accept(mk(i=2), "1.1.1.1", self.s)["code"], "bad_request")
        self.assertEqual(st.accept(mk(vault="0x" + "12" * 20), "1.1.1.1", self.s)["code"], "bad_request")
        st.W.eth.spent.add(st.nullifier(k))
        self.assertEqual(st.accept(mk(), "1.1.1.1", self.s)["code"], "released")
        self.assertEqual((self.s["jobs"], st.SECRETS), ({}, {}))

    def test_caps_memory_waits_and_a_restart(self):
        st.W.prover = lambda secret, args, out: (75, "only 12% of memory is free")
        ids = []
        for n, ip in ((11, "1.1.1.1"), (12, "1.1.1.1"), (13, "1.1.1.1"), (14, "2.2.2.2"), (15, "3.3.3.3")):
            k = bytes([n]) * 32
            leaves = exit_material(k)
            st.W.eth.recorded.add(st.tree_root(leaves))
            out = st.accept(json.dumps({"vault": VAULT, "exit": {"secret": "0x" + k.hex(), "eth_dest": "0x" + DEST.hex(), "token": "0x" + ETH.hex(), "denom": str(DENOMS[0]),
                                          "max_relay_fee": "0", "cfg_id": "0x" + CFG.hex(), "leaves": ["0x" + x.hex() for x in leaves], "index": 1}}), ip, self.s)
            ids.append(out.get("id") or out["code"])
        self.assertEqual(ids[2], "rate_limited")          # PER_CLIENT_DAY 2
        self.assertEqual(ids[4], "rate_limited")          # DAILY_MAX 3
        st.step(self.s)
        j = self.s["jobs"][ids[0]]
        self.assertEqual(j["stage"], "queued")
        self.assertIn("memory", j["text"])
        st.SECRETS.clear()                                # the service restarted
        st.step(self.s)
        self.assertEqual(j["stage"], "failed")
        self.assertIn("send the settlement again", j["error"])

    def test_a_release_sent_before_a_restart_is_not_sent_again(self):
        k = bytes([16]) * 32
        root = st.tree_root(exit_material(k))
        st.W.eth.recorded.add(root)
        out = st.accept(json.dumps({"vault": VAULT, "receipt_hex": "ab" * 3000, "journal_hex": journal_of(k, root).hex()}), "1.2.3.4", self.s)
        j = self.s["jobs"][out["id"]]
        j.update(stage="releasing", seal="0xe71e6620" + "77" * 256)
        st.W.eth.spent.add(st.nullifier(k))
        st.step(self.s)
        self.assertEqual((j["stage"], self.sent), ("released", []))

    def test_the_token_is_not_sent_on_to_the_artifact_storage(self):
        import http.server, threading
        seen = {}

        class B(http.server.BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_GET(self):
                seen["b"] = self.headers.get("Authorization")
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b"zip")

        b = http.server.HTTPServer(("127.0.0.1", 0), B)

        class A(B):
            def do_GET(self):
                seen["a"] = self.headers.get("Authorization")
                self.send_response(302)
                self.send_header("Location", "http://127.0.0.1:%d/blob" % b.server_address[1])
                self.end_headers()

        a = http.server.HTTPServer(("127.0.0.1", 0), A)
        for srv in (a, b):
            threading.Thread(target=srv.serve_forever, daemon=True).start()
        tok = os.path.join(TMP, "tok")
        with open(tok, "w") as f:
            f.write("ghp_test\n")
        try:
            got = st.GitHub(tok)._req("GET", "http://127.0.0.1:%d/zip" % a.server_address[1], raw=True)
        finally:
            for srv in (a, b):
                srv.shutdown()
                srv.server_close()
        self.assertEqual((got, seen["a"], seen["b"]), (b"zip", "Bearer ghp_test", None))

    def test_a_seal_that_cannot_be_fetched_fails_the_job_and_frees_the_wrapper(self):
        k = bytes([17]) * 32
        root = st.tree_root(exit_material(k))
        st.W.eth.recorded.add(root)
        out = st.accept(json.dumps({"vault": VAULT, "receipt_hex": "ab" * 3000, "journal_hex": journal_of(k, root).hex()}), "1.2.3.4", self.s)
        st.step(self.s)
        j = self.s["jobs"][out["id"]]
        fid = st.W.gh.dispatched[0][0]
        st.W.gh.runs[fid] = {"id": 5, "status": "completed", "conclusion": "success"}

        def broken(run_id, fid):
            raise OSError("403")
        st.W.gh.seal = broken
        st.step(self.s)
        st.step(self.s)
        self.assertEqual(j["stage"], "wrapping")
        st.step(self.s)
        self.assertEqual(j["stage"], "failed")

    def test_the_tree_and_the_journal(self):
        self.assertEqual(st.tree_root([]), st.zero_hashes()[32])
        a, b, c = (bytes([i]) * 32 for i in (1, 2, 3))
        z = st.zero_hashes()
        want = st.sha(st.sha(a, b), st.sha(c, z[0]))
        for h in range(2, 32):
            want = st.sha(want, z[h])
        self.assertEqual(st.tree_root([a, b, c]), want)
        j = st.decode_journal(journal_of(bytes([1]) * 32, a))
        self.assertEqual((j["root"], j["recipient"], j["relay_fee"]), (a, DEST, 0))


if __name__ == "__main__":
    unittest.main()
