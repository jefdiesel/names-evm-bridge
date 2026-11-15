#!/usr/bin/python3
"""Offline tests for fee-grants.py (stdlib unittest; no network, no CLI, no wallet).

  python3 -I public-demo/attester/test_fee_grants.py      # from the repository root
"""
import importlib.util, json, os, shutil, subprocess, sys, tempfile, unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TMP = tempfile.mkdtemp(prefix="fee-grants-test-")
os.environ.update(P=TMP, GRANTS_STATE=TMP + "/grants/state.json", GRANTS_LOG=TMP + "/fee-grants.log",
                  ATTESTER_STATE=TMP + "/attester.json", DESK="0x" + "de" * 20, DESK_FROM_BLOCK="100",
                  GRANT_AMOUNT="20100000", DAILY_MAX_GRANTS="2", DAILY_MAX_AMOUNT="1000000000",
                  MIN_OPERATOR_BALANCE="1000000000")
spec = importlib.util.spec_from_file_location("fee_grants", os.path.join(HERE, "fee-grants.py"))
fg = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fg)
FG_ETH = fg.eth   # the real fallback reader (Base replaces fg.eth with the fake)

# P2: public-demo/claim-p2-public-20261113.txt (not published) ("account Public/6NAoZK…, public key f31c4c…")
PK_P2 = "f31c4c105101bf5363ffebb605e318a92cc19e18689c1e57f823f7271ee5a1c3"
ID_P2 = "6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f"
DESK = "0x" + "de" * 20
VAULT = fg.VAULT
ETH0 = "0x" + "00" * 20


def w(n):
    return n.to_bytes(32, "big").hex()


def fee_log(c, pk, leaf, fee, tx="0x" + "aa" * 32, block=200, log_index=3, token=ETH0, denom_index=1):
    return {"address": DESK, "topics": [fg.FEE_PAID_TOPIC, "0x" + token[2:].rjust(64, "0"), "0x" + pk, c],
            "data": "0x" + w(denom_index) + w(leaf) + w(fee), "transactionHash": tx,
            "blockNumber": hex(block), "logIndex": hex(log_index)}


def dep_log(c, leaf):
    return {"address": VAULT, "topics": [fg.DEPOSIT_TOPIC, "0x" + "00" * 32],
            "data": "0x" + w(1) + c[2:] + w(leaf) + "11" * 32, "logIndex": "0x2"}


class FakeChains:
    """Sepolia + LEZ JSON-RPC and the CLI send, in memory."""

    def __init__(self):
        self.logs, self.receipts, self.balances = [], {}, {fg.OPERATOR: 10**14, fg.config_pda(fg.BRIDGE): 10**11}
        self.channel, self.fin, self.sends, self.send_mode = "ch1", 1000, [], "ok"

    def eth(self, method, params):
        if method == "eth_getBlockByNumber":
            return {"number": hex(self.fin), "timestamp": hex(int(fg.time.time()))}
        if method == "eth_getLogs":
            lo, hi = int(params[0]["fromBlock"], 16), int(params[0]["toBlock"], 16)
            return [l for l in self.logs if lo <= int(l["blockNumber"], 16) <= hi]
        if method == "eth_getTransactionReceipt":
            return self.receipts[params[0]]
        raise AssertionError(method)

    def lez(self, method, params):
        if method == "getChannelId":
            return self.channel
        if method == "getAccount":
            b = self.balances.get(params[0])
            return None if b is None else {"nonce": 0, "data": {"actor_states": {fg.NATIVE_PROGRAM: list(b.to_bytes(16, "little"))}}}
        raise AssertionError(method)

    def send(self, pk_hex):
        self.sends.append(pk_hex)
        acct = fg.account_id(bytes.fromhex(pk_hex))
        if self.send_mode == "ok":
            self.balances[acct] = self.balances.get(acct, 0) + fg.GRANT
            self.balances[fg.OPERATOR] -= fg.GRANT
            return 0, "bb" * 32, 77, "Transaction hash is %s\nTransaction is included in block 77\n" % ("bb" * 32)
        if self.send_mode == "refused":
            return 1, None, None, "Error: insufficient funds\n"
        return 1, "cc" * 32, None, "timeout waiting for inclusion\n"   # "lost": sent, nothing arrived

    def add_deposit(self, c, pk, leaf, fee=3 * 10**15, block=200, tx=None):
        tx = tx or "0x" + ("%064x" % (leaf + 1))
        lg = fee_log(c, pk, leaf, fee, tx=tx, block=block)
        self.logs.append(lg)
        self.receipts[tx] = {"status": "0x1", "logs": [dep_log(c, leaf), lg]}


class Base(unittest.TestCase):
    def setUp(self):
        shutil.rmtree(TMP, ignore_errors=True)
        os.makedirs(TMP + "/grants")
        self.f = FakeChains()
        fg.eth, fg.lez, fg.send_grant = self.f.eth, self.f.lez, self.f.send
        fg.wait_balance = lambda acct, want, seconds=90: fg.lez_balance(acct)
        fg.DRY = False
        fg.REGRANT_ON_RESET = False
        self.attest(count=10, block=900)

    def attest(self, count, block):
        with open(fg.ATTESTER_STATE, "w") as f:
            json.dump({"root": "0x" + "ee" * 32, "deposit_count": count, "sepolia_block": block}, f)

    def run_poll(self, state=None):
        state = state or fg.load_state()
        fg.poll(state, {})
        return state


class Pure(unittest.TestCase):
    def test_account_id_vector(self):
        self.assertEqual(fg.account_id(bytes.fromhex(PK_P2)), ID_P2)

    def test_base58_roundtrip(self):
        self.assertEqual(fg.b58encode(fg.b58decode(ID_P2)), ID_P2)
        self.assertEqual(len(fg.b58decode(ID_P2)), 32)
        self.assertEqual(fg.b58decode("11111111111111111111111111111111"), b"\0" * 32)

    def test_config_pda_matches_wallet_read(self):
        # public-demo/WALLET-READ.md (not published) §3
        self.assertEqual(fg.config_pda("JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7"), "HugSoWM7Hp1eXMqknfCewWNMoraweLG3HkezfNGtqKf3")

    def test_public_key_validity(self):
        self.assertTrue(fg.valid_public_key(bytes.fromhex(PK_P2)))
        self.assertFalse(fg.valid_public_key(b"\0" * 32))
        self.assertFalse(fg.valid_public_key(b"\xff" * 32))             # x >= p
        self.assertFalse(fg.valid_public_key(bytes.fromhex(PK_P2)[:31]))
        # about half of all x are not on the curve: find one and check it is refused
        x = 5
        while pow((x ** 3 + 7) % fg.SECP_P, (fg.SECP_P - 1) // 2, fg.SECP_P) == 1:
            x += 1
        self.assertFalse(fg.valid_public_key(x.to_bytes(32, "big")))

    def test_native_of(self):
        # a native balance of 23,786,858: u128 LE bytes 6a f5 6a 01 (as in WALLET-READ.md §4)
        acct = {"nonce": 1, "data": {"actor_states": {fg.NATIVE_PROGRAM: [106, 245, 106, 1] + [0] * 12}}}
        self.assertEqual(fg.native_of(acct), 23786858)
        self.assertEqual(fg.native_of(None), 0)
        self.assertEqual(fg.native_of({"data": {"actor_states": {}}}), 0)
        with self.assertRaises(ValueError):
            fg.native_of({"data": {"actor_states": {fg.NATIVE_PROGRAM: [1, 2, 3]}}})

    def test_decode_fee_paid(self):
        c = "0x" + "12" * 32
        ev = fg.decode_fee_paid(fee_log(c, PK_P2, 7, 3000000, token="0xac207b9bfebf3ef7c26297ea6abadeeccc4aa71e", denom_index=0))
        self.assertEqual((ev["token"], ev["pk"], ev["commitment"], ev["denom_index"], ev["leaf_index"], ev["fee"]),
                         ("0xac207b9bfebf3ef7c26297ea6abadeeccc4aa71e", PK_P2, c, 0, 7, 3000000))
        bad = fee_log(c, PK_P2, 7, 1)
        bad["topics"] = bad["topics"][:3]
        with self.assertRaises(ValueError):
            fg.decode_fee_paid(bad)
        bad = fee_log(c, PK_P2, 7, 1)
        bad["data"] = bad["data"][:-2]
        with self.assertRaises(ValueError):
            fg.decode_fee_paid(bad)
        bad = fee_log(c, PK_P2, 7, 1)
        bad["topics"][1] = "0x" + "ff" * 32                               # not an address
        with self.assertRaises(ValueError):
            fg.decode_fee_paid(bad)

    def test_receipt_check(self):
        c = "0x" + "34" * 32
        lg = fee_log(c, PK_P2, 4, 1)
        ev = fg.decode_fee_paid(lg)
        ok = {"status": "0x1", "logs": [dep_log(c, 4), lg]}
        self.assertIsNone(fg.deposit_in_receipt(ok, DESK, VAULT, ev))
        self.assertIn("failed", fg.deposit_in_receipt(dict(ok, status="0x0"), DESK, VAULT, ev))
        self.assertIn("no vault Deposit", fg.deposit_in_receipt({"status": "0x1", "logs": [dep_log(c, 5), lg]}, DESK, VAULT, ev))
        other = dict(lg, address="0x" + "99" * 20)
        self.assertIn("not the desk", fg.deposit_in_receipt({"status": "0x1", "logs": [dep_log(c, 4), other]}, DESK, VAULT, ev))

    def test_deposit_topic_matches_the_page(self):
        node = shutil.which("node")
        if not node:
            self.skipTest("node not installed")
        root = os.path.dirname(os.path.dirname(HERE))
        out = subprocess.run([node, "-e", "import('./web/deposit/src/lib/vault.js').then(v=>console.log(v.DEPOSIT_TOPIC))"],
                             cwd=root, capture_output=True, text=True, timeout=30)
        self.assertEqual(out.stdout.strip(), fg.DEPOSIT_TOPIC)

    def test_fmt_units(self):
        self.assertEqual(fg.fmt_units(3 * 10**15, 18), "0.003")
        self.assertEqual(fg.fmt_units(3_000_000, 6), "3")
        self.assertEqual(fg.fmt_units(0, 6), "0")


class Daemon(Base):
    def test_grants_once_and_never_again(self):
        c = "0x" + "01" * 32
        self.f.add_deposit(c, PK_P2, leaf=6)
        s = self.run_poll()
        self.assertEqual(self.f.sends, [PK_P2])
        self.assertEqual(self.f.balances[ID_P2], fg.GRANT)
        g = s["grants"][c]
        self.assertEqual((g["status"], g["sent"][0]["lez_tx"], g["sent"][0]["lez_block"]), ("sent", "bb" * 32, 77))
        self.run_poll(s)                    # the same event again, same process
        self.run_poll()                     # and after a restart (state from disk)
        s = fg.load_state()
        s["scanned"] = 100                  # rescan the whole range: the same event is seen again
        fg.save_state(s)
        self.run_poll()
        self.assertEqual(len(self.f.sends), 1)
        with open(fg.LOG) as f:
            text = f.read()
        self.assertIn("GRANT 20100000 native LEZ -> " + ID_P2, text)
        self.assertIn("Sepolia tx 0x" + "%064x" % 7, text)
        self.assertIn("LEZ tx " + "bb" * 32, text)

    def test_zero_key_gets_no_grant_but_counts_in_the_report(self):
        self.f.add_deposit("0x" + "02" * 32, "00" * 32, leaf=6, fee=3 * 10**15)
        s = self.run_poll()
        self.assertEqual(self.f.sends, [])
        line = fg.report_line(s, 1, 2)
        self.assertIn("desk deposits 1 (0 named an account)", line)
        self.assertIn("0.003 Sepolia ETH (3000000000000000 base units)", line)

    def test_waits_for_the_attested_root(self):
        c = "0x" + "03" * 32
        self.f.add_deposit(c, PK_P2, leaf=12, block=950)
        self.attest(count=10, block=900)
        s = self.run_poll()
        self.assertEqual(self.f.sends, [])
        self.attest(count=13, block=960)
        self.run_poll(s)
        self.assertEqual(self.f.sends, [PK_P2])

    def test_no_attester_state_no_grant(self):
        os.remove(fg.ATTESTER_STATE)
        self.f.add_deposit("0x" + "04" * 32, PK_P2, leaf=1)
        self.run_poll()
        self.assertEqual(self.f.sends, [])

    def test_refuses_invalid_key_and_forged_receipt(self):
        x = 5
        while pow((x ** 3 + 7) % fg.SECP_P, (fg.SECP_P - 1) // 2, fg.SECP_P) == 1:
            x += 1
        self.f.add_deposit("0x" + "05" * 32, "%064x" % x, leaf=1)
        c = "0x" + "06" * 32
        self.f.add_deposit(c, PK_P2, leaf=2)
        self.f.receipts[self.f.logs[-1]["transactionHash"]]["logs"][0] = dep_log("0x" + "07" * 32, 2)  # different commitment
        s = self.run_poll()
        self.assertEqual(self.f.sends, [])
        self.assertIn("not a valid LEZ public key", s["events"]["0x" + "05" * 32]["refused"])
        self.assertIn("no vault Deposit", s["events"][c]["refused"])

    def test_daily_cap(self):
        keys = []
        for i in range(3):
            k = 7 + i
            while not fg.valid_public_key(k.to_bytes(32, "big")):
                k += 100
            keys.append("%064x" % k)
            self.f.add_deposit("0x" + ("%064x" % (0x100 + i)), keys[-1], leaf=i)
        self.run_poll()
        self.assertEqual(len(self.f.sends), 2)            # DAILY_MAX_GRANTS = 2

    def test_low_balance_stop(self):
        self.f.balances[fg.OPERATOR] = fg.MIN_OPERATOR_BALANCE + fg.GRANT - 1
        self.f.add_deposit("0x" + "08" * 32, PK_P2, leaf=1)
        self.run_poll()
        self.assertEqual(self.f.sends, [])
        self.f.balances[fg.OPERATOR] = 10**14
        self.run_poll()
        self.assertEqual(self.f.sends, [PK_P2])

    def test_refused_send_is_retried_lost_send_is_not(self):
        c = "0x" + "09" * 32
        self.f.add_deposit(c, PK_P2, leaf=1)
        self.f.send_mode = "refused"
        s = self.run_poll()
        self.assertEqual(s["grants"][c]["status"], "failed")
        self.f.send_mode = "ok"
        self.run_poll(s)
        self.assertEqual(s["grants"][c]["status"], "sent")
        self.assertEqual(len(self.f.sends), 2)

        c2 = "0x" + "0a" * 32
        k = 11
        while not fg.valid_public_key(k.to_bytes(32, "big")):
            k += 1
        self.f.fin = 1100                                  # a later deposit, past the scanned range
        self.attest(count=10, block=1100)
        self.f.add_deposit(c2, "%064x" % k, leaf=2, block=1050)
        self.f.send_mode = "lost"
        self.run_poll(s)
        self.assertEqual(s["grants"][c2]["status"], "uncertain")
        self.f.send_mode = "ok"
        self.run_poll(s)
        self.assertEqual(len(self.f.sends), 3)            # never resent while uncertain

    def test_cli_that_raises_never_leaves_pending(self):
        c = "0x" + "0e" * 32
        self.f.add_deposit(c, PK_P2, leaf=1)
        def boom(pk):
            raise fg.subprocess.TimeoutExpired("lez-bridge", 300)
        fg.send_grant = boom
        s = self.run_poll()
        self.assertEqual(s["grants"][c]["status"], "uncertain")   # timed out, nothing arrived: hold
        def missing(pk):
            raise FileNotFoundError("no CLI")
        c2 = "0x" + "0f" * 32
        self.f.fin = 1100
        self.attest(count=10, block=1100)
        k = 13
        while not fg.valid_public_key(k.to_bytes(32, "big")):
            k += 1
        self.f.add_deposit(c2, "%064x" % k, leaf=2, block=1050)
        fg.send_grant = missing
        self.run_poll(s)
        self.assertEqual(s["grants"][c2]["status"], "failed")     # the CLI never ran: retryable

    def test_pending_at_restart_is_not_resent(self):
        c = "0x" + "0b" * 32
        self.f.add_deposit(c, PK_P2, leaf=1)
        s = fg.load_state()
        fg.scan(s, 1000)
        s["grants"][c] = {"status": "pending", "channel": "ch1", "account": ID_P2, "pk": PK_P2,
                          "amount": fg.GRANT, "balance_before": 0, "attempts": 1, "sent": []}
        fg.save_state(s)
        s = fg.load_state()
        fg.reconcile_pending(s)
        self.assertEqual(s["grants"][c]["status"], "uncertain")   # nothing arrived: hold, don't resend
        self.run_poll(s)
        self.assertEqual(self.f.sends, [])
        # had it arrived, it is recorded as sent
        s["grants"][c]["status"] = "pending"
        self.f.balances[ID_P2] = fg.GRANT
        fg.reconcile_pending(s)
        self.assertEqual(s["grants"][c]["status"], "sent")

    def test_reset_no_regrant_by_default_once_with_flag(self):
        c = "0x" + "0c" * 32
        self.f.add_deposit(c, PK_P2, leaf=1)
        s = self.run_poll()
        self.assertEqual(len(self.f.sends), 1)
        self.f.channel = "ch2"
        self.run_poll(s)
        self.assertEqual(len(self.f.sends), 1)
        fg.REGRANT_ON_RESET = True
        self.run_poll(s)
        self.run_poll(s)
        self.assertEqual(len(self.f.sends), 2)
        self.assertEqual([x["channel"] for x in s["grants"][c]["sent"]], ["ch1", "ch2"])

    def test_dry_run_never_sends(self):
        fg.DRY = True
        self.f.add_deposit("0x" + "0d" * 32, PK_P2, leaf=1)
        s = self.run_poll()
        self.assertEqual(self.f.sends, [])
        self.assertEqual(s["grants"], {})

    def test_float_accounting(self):
        s = self.run_poll()
        self.f.balances[fg.config_pda(fg.BRIDGE)] -= 25_000_000      # one private claim sponsored
        self.run_poll(s)
        self.f.balances[fg.config_pda(fg.BRIDGE)] += 1_000_000_000   # operator top-up
        self.run_poll(s)
        self.assertEqual((s["float"]["drawdown"], s["float"]["topups"]), (25_000_000, 1_000_000_000))
        self.assertIn("float drawdown 25000000", fg.report_line(s, 1, 2))
        fg.track_float(s, 100, "relay")
        fg.track_float(s, 40, "relay")
        line = fg.report_line(s, 1, 2, 40)
        self.assertIn("relay payer: drawdown 60", line)
        self.assertIn("total %d" % (25_000_000 + 60), line)


class SepoliaFallback(Base):
    """SEPOLIA_RPC as an ordered list: a pruned-history error or a null receipt (PublicNode's answers
    for old blocks, 2026-11-14) moves to the next RPC; with none left the poll fails and `scanned`
    stays, and the deposit is never marked refused."""

    def setUp(self):
        super().setUp()
        self.real_eth, self.real_post, self.real_list = FG_ETH, fg.post, fg.SEPOLIAS
        fg.eth = FG_ETH
        fg.SEPOLIAS = ["https://pruned.invalid/", "https://full.invalid/"]
        self.seen = []
        pruned_mode = {"mode": "error"}
        self.pruned_mode = pruned_mode

        def post(url, method, params, timeout=20):
            self.seen.append((url, method))
            if url == "https://pruned.invalid/":
                if method == "eth_getBlockByNumber":
                    return self.f.eth(method, params)
                if pruned_mode["mode"] == "error":
                    raise RuntimeError('%s: {"code": 4444, "message": "pruned history unavailable"}' % method)
                if method == "eth_getTransactionReceipt":
                    return None                               # a pruned receipt: null, no error
                return self.f.eth(method, params)
            if url == "https://full.invalid/":
                return self.f.eth(method, params)
            return self.f.lez(method, params)
        fg.post = post

    def tearDown(self):
        fg.post, fg.SEPOLIAS = self.real_post, self.real_list

    def test_pruned_logs_move_to_the_next_rpc(self):
        self.f.add_deposit("0x" + "21" * 32, "00" * 32, leaf=6)
        s = self.run_poll()
        self.assertEqual(len(s["events"]), 1)
        self.assertNotIn("refused", s["events"]["0x" + "21" * 32])
        self.assertIn(("https://full.invalid/", "eth_getLogs"), self.seen)

    def test_a_null_receipt_is_never_a_refusal(self):
        self.pruned_mode["mode"] = "null"
        fg.SEPOLIAS = ["https://pruned.invalid/"]
        self.f.add_deposit("0x" + "22" * 32, "00" * 32, leaf=6)
        s = fg.load_state()
        before = s["scanned"]
        with self.assertRaises(RuntimeError) as e:
            fg.poll(s, {})
        self.assertIn("no Sepolia RPC answered eth_getTransactionReceipt", str(e.exception))
        self.assertIn("answered null", str(e.exception))
        self.assertEqual(s["scanned"], before, "the scan did not move past the deposit")
        self.assertEqual(s["events"], {})
        # The next poll, with an RPC that has the receipt, sees it normally.
        fg.SEPOLIAS = ["https://pruned.invalid/", "https://full.invalid/"]
        fg.poll(s, {})
        self.assertNotIn("refused", s["events"]["0x" + "22" * 32])

    def test_scan_refuses_a_null_receipt_itself(self):
        fg.eth = lambda m, p: None if m == "eth_getTransactionReceipt" else self.f.eth(m, p)
        self.f.add_deposit("0x" + "23" * 32, "00" * 32, leaf=6)
        s = fg.load_state()
        with self.assertRaises(RuntimeError) as e:
            fg.scan(s, self.f.fin)
        self.assertIn("no receipt for Sepolia tx", str(e.exception))
        self.assertEqual(s["events"], {})


if __name__ == "__main__":
    try:
        unittest.main(verbosity=2)
    finally:
        shutil.rmtree(TMP, ignore_errors=True)
