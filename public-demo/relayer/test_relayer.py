#!/usr/bin/python3
"""Offline tests for relayer.py (stdlib unittest; no network beyond loopback, no CLI, no wallet).

  python3 -I public-demo/relayer/test_relayer.py      # from the repository root
"""
import http.client, importlib.util, json, os, shutil, stat, tempfile, threading, time, types, unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TMP = tempfile.mkdtemp(prefix="relayer-test-")
os.environ.update(P=TMP, RELAY_STATE=TMP + "/relay/state.json", RELAY_LOG=TMP + "/relayer.log", RELAY_WALLET=TMP + "/wallet",
                  PAYER="RLYpayer1111111111111111111111111111111111", TOKEN_PROGRAM="TP11111111111111111111111111111111111111111",
                  DAILY_MAX="3", IP_DAILY_MAX="2", MAX_RELAYS="2", IP_DAILY_CHECKS="5", MIN_PAYER_BALANCE="1000", DESK_STATE="",
                  DRY_RUN="0", RELAY_QUEUE="2", MAX_DROPS="2")
spec = importlib.util.spec_from_file_location("relayer", os.path.join(HERE, "relayer.py"))
rl = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rl)
os.makedirs(TMP + "/relay", exist_ok=True)
os.makedirs(TMP + "/wallet", exist_ok=True)
rl.RECONCILE_EVERY = 0
rl.DROP_CONFIRM = 0          # one test below sets it back

TX = "ab" * 100
DAY = 1_800_000_000  # a fixed time
MIDNIGHT = DAY - DAY % 86400 + 86400
PAYER = os.environ["PAYER"]


class FakeCli:
    """`relay-cosign`: the check and the submit. A tx hash per nullifier; the nonces it reports
    are those of the fake chain at the check."""

    def __init__(self, chain):
        self.chain, self.calls, self.nf, self.submit_answer, self.check_answer = chain, [], "0x" + "11" * 32, None, None

    def __call__(self, tx, submit):
        self.calls.append(submit)
        if not submit and self.check_answer:
            return self.check_answer
        base = {"ok": True, "nullifier": self.nf, "commitment": "0x" + "cc" * 32, "claimer": "Claimer1", "leaf_index": 3,
                "gas_limit": 190210, "max_fee": "1700000", "cycles": 152168, "payer_balance": "100000000",
                "payer_nonce": str(self.chain.nonces.get(PAYER, 0)), "claimer_nonce": str(self.chain.nonces.get("Claimer1", 0))}
        if submit:
            return self.submit_answer or dict(base, tx_hash=self.hash())
        return base

    def hash(self):
        return "e" + self.nf[3:66]


class FakeChain:
    """Account nonces, transactions in blocks, and the chain's identity."""

    def __init__(self):
        self.nonces, self.blocks, self.fail, self.ident = {}, {}, False, ("chan-A", "genesis-A")

    def nonce(self, account):
        if self.fail:
            raise OSError("connection refused")
        return self.nonces.get(account, 0)

    def tx_block(self, h):
        if self.fail:
            raise OSError("connection refused")
        return self.blocks.get(h)

    def identity(self):
        return self.ident

    def land(self, h, block=7):
        """The transaction is in a block: both signers' nonces move."""
        self.blocks[h] = block
        self.nonces[PAYER] = self.nonces.get(PAYER, 0) + 1
        self.nonces["Claimer1"] = self.nonces.get("Claimer1", 0) + 1


def body(check_only=False, tx=TX):
    return json.dumps({"tx": tx, "check_only": check_only})


def read(path):
    try:
        with open(path) as f:
            return f.read()
    except FileNotFoundError:
        return ""


class Relayer(unittest.TestCase):
    def setUp(self):
        self.state = rl.load_state()
        self.state.update(nullifiers={}, window={"relayed": [], "ips": {}}, ipkeys={}, inflight=None)
        self.chain = FakeChain()
        self.cli = FakeCli(self.chain)
        rl.DESK_STATE = ""
        rl._last_reconcile[0] = 0.0

    def go(self, b=None, ip="1.2.3.4", now=DAY, desk=lambda: set()):
        return rl.handle(body() if b is None else b, ip, self.state, now=now, cli=self.cli, desk=desk, chain=self.chain)

    def land(self, block=7):
        self.chain.land(self.cli.hash(), block)

    def relayed_counts(self, now=DAY):
        return len([t for t in self.state["window"]["relayed"] if t > now - 86400])

    # ------------------------------------------------------------ as before, with inclusion

    def test_relays_once_per_nullifier_and_writes_the_intent_first(self):
        seen = []
        real = rl.save_state
        rl.save_state = lambda s: seen.append(json.loads(json.dumps(s)))
        try:
            out = self.go()
        finally:
            rl.save_state = real
        self.assertEqual(out, {"ok": True, "tx_hash": self.cli.hash()})
        self.assertEqual(self.cli.calls, [False, True])
        self.assertEqual(seen[-2]["nullifiers"][self.cli.nf]["status"], "pending")  # saved before the submit
        self.assertEqual(self.state["nullifiers"][self.cli.nf]["status"], "sent")    # not final until in a block
        self.land()
        again = self.go(now=DAY + 10)
        self.assertEqual(self.state["nullifiers"][self.cli.nf]["status"], "submitted")
        self.assertFalse(again["ok"])
        self.assertIn("already relayed", again["reason"])
        self.assertEqual(self.cli.calls, [False, True, False])  # checked, never signed again

    def test_one_retry_when_the_first_relayed_claim_did_not_apply(self):
        self.go()
        self.land()                                 # included, but reverted (the nullifier stays unspent)
        self.assertIn("already relayed", self.go(now=DAY + rl.RETRY_AFTER - 1)["reason"])   # too soon
        self.assertTrue(self.go(now=DAY + rl.RETRY_AFTER + 1)["ok"])                       # the retry
        g = self.state["nullifiers"][self.cli.nf]
        self.assertEqual((g["relays"], g["txs"]), (2, [self.cli.hash()]))
        self.chain.blocks[self.cli.hash()] = 9
        self.chain.nonces[PAYER] += 1
        self.assertIn("already relayed", self.go(ip="8.8.8.8", now=DAY + 10 * rl.RETRY_AFTER)["reason"])  # no third

    def test_check_only_signs_nothing(self):
        out = self.go(body(check_only=True))
        self.assertEqual(out, {"ok": True, "checked": True})
        self.assertEqual(self.cli.calls, [False])
        self.assertEqual(self.state["nullifiers"], {})

    def test_a_refusal_at_the_check_is_passed_on_with_its_retry_flag(self):
        self.cli.check_answer = {"ok": False, "reason": "stale nonce: rebuild", "retry": True}
        self.assertEqual(self.go(), {"ok": False, "reason": "stale nonce: rebuild", "retry": True})
        self.assertEqual(self.cli.calls, [False])

    def test_a_refusal_at_submit_frees_the_nullifier_and_the_counters(self):
        self.cli.submit_answer = {"ok": False, "reason": "stale nonce", "retry": True}
        out = self.go()
        self.assertEqual(out["retry"], True)
        self.assertEqual(self.state["nullifiers"], {})
        self.assertEqual(self.relayed_counts(), 0)
        self.assertIsNone(self.state["inflight"])

    def test_a_cli_failure_at_submit_is_uncertain_and_blocks_the_nullifier(self):
        self.cli.submit_answer = {"ok": False, "reason": "relay error: timed out", "retry": True, "cli_failed": True}
        out = self.go()
        self.assertFalse(out["ok"])
        self.assertFalse(out["retry"])
        self.assertEqual(self.state["nullifiers"][self.cli.nf]["status"], "uncertain")
        self.cli.submit_answer = None
        self.assertIn("busy", self.go(now=DAY + 5)["reason"])                       # its payer nonce may be in use
        self.assertIn("uncertain", self.go(now=DAY + rl.STUCK_AFTER + 5)["reason"])  # unblocked, still the operator's

    def test_caps_per_client_per_24h_and_overall(self):
        for i in range(2):
            self.cli.nf = "0x%064x" % (i + 1)
            self.assertTrue(self.go(now=DAY + i)["ok"])
            self.land()
        self.cli.nf = "0x%064x" % 9
        self.assertIn("from this address", self.go(now=DAY + 3)["reason"])
        self.assertTrue(self.go(ip="5.6.7.8", now=DAY + 4)["ok"])  # third of DAILY_MAX 3
        self.land()
        self.cli.nf = "0x%064x" % 10
        self.assertIn("24 h cap", self.go(ip="9.9.9.9", now=DAY + 5)["reason"])
        self.assertTrue(self.go(ip="9.9.9.9", now=DAY + 86401)["ok"])  # 24 h later

    def test_checks_per_client_are_capped_too(self):
        for _ in range(5):
            self.go(body(check_only=True), ip="7.7.7.7")
        self.assertIn("too many requests", self.go(body(check_only=True), ip="7.7.7.7")["reason"])

    def test_desk_only_and_the_balance_floor(self):
        rl.DESK_STATE = "/nonexistent"
        out = self.go(desk=lambda: {"0x" + "dd" * 32})
        self.assertIn("deposit page", out["reason"])
        self.assertTrue(self.go(desk=lambda: {"0x" + "cc" * 32})["ok"])
        self.land()
        rl.DESK_STATE = ""
        self.cli.nf = "0x" + "22" * 32
        self.cli.check_answer = dict(FakeCli(self.chain)(TX, False), nullifier=self.cli.nf, payer_balance="1700500")
        self.assertIn("out of funds", self.go()["reason"])

    def test_malformed_requests_never_reach_the_cli(self):
        for b in ["", "{", json.dumps({"tx": 5}), json.dumps({"tx": "zz"}), json.dumps({"tx": "ab" * 9000}), "x" * (rl.MAX_BODY + 1)]:
            self.assertFalse(self.go(b)["ok"], b[:30])
        self.assertEqual(self.cli.calls, [])

    def test_desk_commitments_reads_the_daemon_state_and_skips_refused_events(self):
        p = TMP + "/grants.json"
        with open(p, "w") as f:
            json.dump({"events": {"0xAA": {}, "0xbb": {"refused": "x"}}}, f)
        rl.DESK_STATE = p
        self.assertEqual(rl.desk_commitments(), {"0xaa"})

    # ------------------------------------------------------------ M1: one co-signed claim in flight

    def test_m1_a_second_claim_in_the_same_block_is_busy_not_dropped(self):
        self.assertTrue(self.go()["ok"])                           # claim A: sent, payer nonce 0
        self.cli.nf = "0x" + "22" * 32                             # claim B, built on the same payer nonce
        out = self.go(ip="5.6.7.8", now=DAY + 2)
        self.assertEqual((out["ok"], out["retry"], out["retry_after"]), (False, True, rl.BUSY_RETRY))
        self.assertIn("stale nonce", out["reason"])                 # what the Names Wallet module re-signs on
        self.assertEqual(self.cli.calls, [False, True])            # B never reached the CLI
        self.assertNotIn(self.cli.nf, self.state["nullifiers"])    # and holds no record or slot
        self.assertEqual(self.relayed_counts(), 1)
        self.chain.land("e" + ("0x" + "11" * 32)[3:66])            # A lands; B's CLI rebuilds on the new nonce
        out = self.go(ip="5.6.7.8", now=DAY + 20)
        self.assertTrue(out["ok"])
        self.assertEqual(self.state["nullifiers"]["0x" + "11" * 32]["status"], "submitted")
        self.assertEqual(self.state["nullifiers"][self.cli.nf]["payer_nonce"], 1)

    def test_a_check_of_another_claim_is_not_busy(self):
        self.go()
        self.cli.nf = "0x" + "22" * 32
        self.assertEqual(self.go(body(check_only=True), now=DAY + 1), {"ok": True, "checked": True})

    def test_a_dropped_submission_frees_the_deposit_and_its_counters(self):
        self.go()
        self.chain.nonces[PAYER] = 1                    # another transaction took the payer's nonce; ours is not in a block
        self.assertIsNone(self.state["nullifiers"][self.cli.nf].get("block"))
        out = self.go(now=DAY + 20)                     # the claimer tries again: relayed, not "already relayed"
        self.assertTrue(out["ok"], out)
        g = self.state["nullifiers"][self.cli.nf]
        self.assertEqual((g["status"], g["relays"], g["drops"], g["payer_nonce"]), ("sent", 1, 1, 1))
        self.assertEqual(self.relayed_counts(DAY + 20), 1)   # the dropped one gave its slot back

    def test_drop_evidence_must_hold_for_a_while_before_the_slot_is_given_back(self):
        rl.DROP_CONFIRM = 20
        try:
            self.go()
            self.chain.nonces[PAYER] = 1
            self.cli.nf = "0x" + "22" * 32
            self.assertIn("busy", self.go(now=DAY + 16)["reason"])        # evidence seen once: still busy
            self.assertEqual(self.relayed_counts(DAY + 16), 1)
            self.assertTrue(self.go(now=DAY + 40)["ok"])                  # held 24 s: dropped, the next one goes
            self.assertEqual(self.state["nullifiers"]["0x" + "11" * 32]["status"], "dropped")
        finally:
            rl.DROP_CONFIRM = 0

    def test_the_claimers_own_transaction_drops_it_too_and_drops_are_capped(self):
        for i in range(rl.MAX_DROPS):
            self.assertTrue(self.go(now=DAY + 100 * i)["ok"])
            self.chain.nonces["Claimer1"] = self.chain.nonces.get("Claimer1", 0) + 1   # the claimer's own tx landed first
        out = self.go(now=DAY + 1000)
        self.assertIn("dropped %d times" % rl.MAX_DROPS, out["reason"])
        self.assertEqual(self.state["nullifiers"][self.cli.nf]["status"], "dropped")
        self.assertIsNone(self.state["inflight"])

    def test_a_failed_chain_read_keeps_it_busy(self):
        self.go()
        self.chain.fail = True
        self.cli.nf = "0x" + "22" * 32
        self.assertIn("busy", self.go(now=DAY + 30)["reason"])

    def test_a_stuck_submission_becomes_uncertain_and_unblocks_others(self):
        self.go()
        self.cli.nf = "0x" + "22" * 32
        self.assertIn("busy", self.go(now=DAY + rl.STUCK_AFTER - 1)["reason"])
        self.assertTrue(self.go(now=DAY + rl.STUCK_AFTER + 1)["ok"])
        self.assertEqual(self.state["nullifiers"]["0x" + "11" * 32]["status"], "uncertain")
        # Later both nonces move with the second claim; the first, never in a block, is dropped then.
        self.chain.land(self.cli.hash())
        self.go(body(check_only=True), now=DAY + rl.STUCK_AFTER + 30)
        self.assertEqual(self.state["nullifiers"]["0x" + "11" * 32]["status"], "dropped")
        self.assertEqual(self.state["nullifiers"][self.cli.nf]["status"], "submitted")

    # ------------------------------------------------------------ L4: a lost answer

    def test_a_lost_send_answer_with_its_hash_is_settled_from_the_chain(self):
        self.cli.submit_answer = {"ok": False, "reason": "relay error: the sequencer's answer was lost", "retry": False,
                                  "uncertain": True, "tx_hash": self.cli.hash(), "detail": "error sending request for url (http://127.0.0.1:3290/)"}
        out = self.go()
        self.assertEqual((out["ok"], out["retry"]), (False, False))
        self.assertEqual(self.state["nullifiers"][self.cli.nf]["status"], "uncertain")
        self.assertEqual(self.relayed_counts(), 1)                   # counted: it may have been charged
        self.land()                                                  # it had gone out and landed
        self.go(body(check_only=True), now=DAY + 20)
        self.assertEqual(self.state["nullifiers"][self.cli.nf]["status"], "submitted")
        self.assertNotIn("127.0.0.1", read(rl.STATE))

    def test_a_lost_answer_without_a_hash_and_only_the_claimer_moved_is_dropped(self):
        self.cli.submit_answer = {"ok": False, "reason": "relay error: timed out", "retry": True, "cli_failed": True}
        self.go()
        self.chain.nonces["Claimer1"] = 1
        self.go(body(check_only=True), now=DAY + 20)
        self.assertEqual(self.state["nullifiers"][self.cli.nf]["status"], "dropped")

    # ------------------------------------------------------------ --release

    def test_release_frees_a_never_included_record_and_refuses_a_landed_one(self):
        nf = "0x" + "33" * 32
        # A record from before 2026-11-14, when `submitted` meant "sent" (the review's M1 victim).
        self.state["nullifiers"][nf] = {"status": "submitted", "at": DAY, "tx_hash": "f" * 64, "txs": [], "relays": 1, "claimer": "C"}
        rl.save_state(self.state)
        self.assertIsNone(rl.release(nf, self.chain))
        self.assertNotIn(nf, rl.load_state()["nullifiers"])
        self.state["nullifiers"][nf] = {"status": "submitted", "at": DAY, "tx_hash": "f" * 64, "txs": [], "relays": 1, "claimer": "C"}
        rl.save_state(self.state)
        self.chain.blocks["f" * 64] = 12
        self.assertIn("block 12", rl.release(nf, self.chain))
        self.assertIn(nf, rl.load_state()["nullifiers"])

    # ------------------------------------------------------------ M4, L3: clients and the window

    def test_ipv6_clients_share_a_budget_per_64_and_ipv4_mapped_is_ipv4(self):
        self.assertEqual(rl.client_key("2001:db8:1:2:aaaa::1"), "2001:db8:1:2::/64")
        self.assertEqual(rl.client_key("::ffff:198.51.100.7"), "198.51.100.7")
        self.assertEqual(rl.client_key("198.51.100.7"), "198.51.100.7")
        for i in range(2):
            self.cli.nf = "0x%064x" % (i + 1)
            self.assertTrue(self.go(ip="2001:db8:1:2::%x" % (i + 1), now=DAY + i)["ok"])
            self.land()
        self.cli.nf = "0x%064x" % 3
        self.assertIn("from this address", self.go(ip="2001:db8:1:2:ffff::9", now=DAY + 3)["reason"])
        self.assertTrue(self.go(ip="2001:db8:1:3::1", now=DAY + 4)["ok"])     # another /64

    def test_the_window_rolls_over_24h_not_at_utc_midnight(self):
        t0 = MIDNIGHT - 60
        for i in range(3):
            self.cli.nf = "0x%064x" % (i + 1)
            self.assertTrue(self.go(ip="10.0.0.%d" % i, now=t0 + i)["ok"])
            self.land()
        self.cli.nf = "0x%064x" % 4
        self.assertIn("24 h cap", self.go(ip="10.0.1.1", now=MIDNIGHT + 60)["reason"])   # the review's 40-in-3-minutes
        self.assertTrue(self.go(ip="10.0.1.1", now=t0 + 86401)["ok"])

    def test_prune_drops_old_events_day_keys_and_client_tags(self):
        self.go()
        self.land()
        self.go(body(check_only=True), ip="9.8.7.6", now=DAY + 5)
        self.assertTrue(self.state["window"]["ips"])
        rl.prune(self.state, DAY + 3 * 86400)
        self.assertEqual((self.state["window"]["relayed"], self.state["window"]["ips"]), ([], {}))
        self.assertEqual(list(self.state["ipkeys"]), [])
        self.assertNotIn("ip_tag", self.state["nullifiers"][self.cli.nf])

    # ------------------------------------------------------------ M3, L6, L7: nothing private kept

    def test_no_address_secret_or_transaction_in_state_or_log_and_files_are_600(self):
        marker = "5ec2e7" * 20                        # stands for the secret inside the claim's bytes
        tx = "ab" * 50 + marker + "cd" * 50
        ip = "203.0.113.77"
        self.cli.check_answer = {"ok": False, "reason": "claim refused: 0x%s" % marker, "retry": False,
                                 "detail": "error sending request for url (http://127.0.0.1:3290/) " + marker}
        self.assertFalse(self.go(body(tx=tx), ip=ip)["ok"])     # refused at the check
        self.cli.check_answer = {"ok": False, "reason": "relay error: error sending request for url (http://127.0.0.1:3290/)", "retry": True}
        out = self.go(body(tx=tx), ip=ip)
        self.assertEqual(out["reason"], rl.GENERIC_ERROR)   # L7: no internal URL to the client
        self.cli.check_answer = None
        self.assertTrue(self.go(body(check_only=True, tx=tx), ip=ip)["ok"])
        self.assertTrue(self.go(body(tx=tx), ip=ip)["ok"])
        for path in (rl.STATE, rl.LOG):
            text = read(path)
            self.assertTrue(text, path)
            self.assertNotIn(marker, text, path)
            self.assertNotIn(ip, text, path)
            self.assertNotIn(tx[:40], text, path)
            self.assertEqual(stat.S_IMODE(os.stat(path).st_mode), 0o600, path)
        self.assertEqual([n for n in os.listdir(TMP + "/relay") if n.startswith("tx-")], [])

    def test_the_client_address_header_is_trusted_only_with_the_filters_secret(self):
        secret = "s" * 48
        rl.AUTH = secret
        try:
            h = lambda **hd: types.SimpleNamespace(headers=hd, client_address=("127.0.0.1", 5000))
            self.assertEqual(rl.Handler.client_ip(h(**{"CF-Connecting-IP": "198.51.100.1", "X-Relay-Auth": secret})), "198.51.100.1")
            self.assertEqual(rl.Handler.client_ip(h(**{"CF-Connecting-IP": "198.51.100.1", "X-Relay-Auth": "wrong" * 10})), "127.0.0.1")
            self.assertEqual(rl.Handler.client_ip(h(**{"CF-Connecting-IP": "198.51.100.1"})), "127.0.0.1")
            rl.AUTH = None   # no secret configured: never trusted
            self.assertEqual(rl.Handler.client_ip(h(**{"CF-Connecting-IP": "198.51.100.1", "X-Relay-Auth": secret})), "127.0.0.1")
        finally:
            rl.AUTH = None

    def test_the_shared_secret_file_must_be_600(self):
        p = TMP + "/auth"
        with open(p, "w") as f:
            f.write("x" * 40)
        os.chmod(p, 0o644)
        rl.AUTH_FILE = p
        try:
            with self.assertRaises(SystemExit):
                rl.load_auth()
            os.chmod(p, 0o600)
            self.assertEqual(rl.load_auth(), "x" * 40)
        finally:
            rl.AUTH_FILE = ""

    # ------------------------------------------------------------ M2: one payer key, one chain

    def test_the_genesis_hash_is_read_from_the_header_not_the_whole_block(self):
        import base64
        hdr = (1).to_bytes(8, "little") + b"\0" * 32 + b"\x5a" * 32 + (0).to_bytes(8, "little")
        body_ = (0).to_bytes(4, "little") * 2
        pending, final = (base64.b64encode(hdr + body_ + bytes([s])).decode() for s in (0, 2))   # bedrock_status
        self.assertEqual(rl.genesis_hash(pending), "5a" * 32)
        self.assertEqual(rl.genesis_hash(final), "5a" * 32)
        self.assertIsNone(rl.genesis_hash(base64.b64encode((2).to_bytes(8, "little") + b"\0" * 72).decode()))  # not block 1
        self.assertIsNone(rl.genesis_hash("not base64!"))
        self.assertEqual(rl.genesis_hash({"header": {"hash": "ab"}}), "ab")

    def test_the_payer_is_bound_to_one_chain(self):
        b = rl.binding_path()
        if os.path.exists(b):
            os.unlink(b)
        self.assertIn("no chain binding", rl.check_binding(self.chain))
        self.chain.nonces[PAYER] = 1
        self.assertIn("nonce on this chain is 1", rl.bind_payer(self.chain))     # it has signed here already
        self.chain.nonces[PAYER] = 0
        self.assertIsNone(rl.bind_payer(self.chain))
        self.assertEqual(stat.S_IMODE(os.stat(b).st_mode), 0o600)
        self.assertIsNone(rl.check_binding(self.chain))
        self.assertIn("exists already", rl.bind_payer(self.chain))
        self.chain.ident = ("chan-B", "genesis-B")                                # a re-genesis (new channel)
        self.assertIn("bound to another chain", rl.check_binding(self.chain))
        self.chain.ident = ("chan-A", "genesis-A2")                               # same channel id, other genesis
        self.assertIn("bound to another chain", rl.check_binding(self.chain))
        os.unlink(b)



class FakeBurnCli:
    """`relay-cosign --kind burn`: the check and the submit of a public burn."""

    def __init__(self, chain):
        self.chain, self.calls, self.exit, self.submit_answer, self.check_answer, self.kinds = chain, [], "0x" + "e1" * 32, None, None, []

    def __call__(self, tx, submit, kind="claim"):
        self.calls.append(submit)
        self.kinds.append(kind)
        if not submit and self.check_answer:
            return self.check_answer
        base = {"ok": True, "kind": "burn", "exit_commitment": self.exit, "burner": "Claimer1", "eth_token": "0x" + "00" * 20,
                "denom": "10000000000000000", "gas_limit": 120000, "max_fee": "1700000", "cycles": 90000, "payer_balance": "100000000",
                "payer_nonce": str(self.chain.nonces.get(PAYER, 0)), "burner_nonce": str(self.chain.nonces.get("Claimer1", 0))}
        if submit:
            return self.submit_answer or dict(base, tx_hash=self.hash())
        return base

    def hash(self):
        return "b" + self.exit[3:66]


class BurnRelay(unittest.TestCase):
    """POST /relay/burn ("Send back to Ethereum"): the same order of checks as a claim, its own caps."""

    def setUp(self):
        self.state = rl.load_state()
        self.state.update(nullifiers={}, window={"relayed": [], "ips": {}}, ipkeys={}, inflight=None)
        self.chain = FakeChain()
        self.cli = FakeBurnCli(self.chain)
        rl._last_reconcile[0] = 0.0
        self.caps = (rl.BURN_DAILY_MAX, rl.IP_DAILY_BURNS)
        rl.BURN_DAILY_MAX, rl.IP_DAILY_BURNS = 3, 2

    def tearDown(self):
        rl.BURN_DAILY_MAX, rl.IP_DAILY_BURNS = self.caps

    def go(self, b=None, ip="1.2.3.4", now=DAY):
        return rl.handle_burn(body() if b is None else b, ip, self.state, now=now, cli=self.cli, chain=self.chain)

    def test_a_burn_is_checked_as_a_burn_relayed_once_and_settled_from_the_chain(self):
        out = self.go()
        self.assertEqual(out, {"ok": True, "tx_hash": self.cli.hash()})
        self.assertEqual((self.cli.calls, self.cli.kinds), ([False, True], ["burn", "burn"]))
        g = self.state["nullifiers"]["exit:" + self.cli.exit]
        self.assertEqual((g["kind"], g["status"]), ("burn", "sent"))
        self.assertIn("busy", self.go(now=DAY + 2)["reason"])                 # its payer nonce is in use
        self.chain.land(self.cli.hash())
        again = self.go(now=DAY + 10)
        self.assertEqual(self.state["nullifiers"]["exit:" + self.cli.exit]["status"], "submitted")
        self.assertIn("already relayed", again["reason"])                    # never twice: no nullifier says it applied
        self.assertEqual(self.cli.calls, [False, True, False])

    def test_burn_caps_are_their_own_and_claims_keep_theirs(self):
        for i in range(2):
            self.cli.exit = "0x%064x" % (i + 1)
            self.assertTrue(self.go(now=DAY + i)["ok"])
            self.chain.land(self.cli.hash())
        self.cli.exit = "0x%064x" % 9
        self.assertIn("from this address", self.go(now=DAY + 3)["reason"])
        self.assertTrue(self.go(ip="5.6.7.8", now=DAY + 4)["ok"])
        self.chain.land(self.cli.hash())
        self.cli.exit = "0x%064x" % 10
        self.assertIn("cap for burns", self.go(ip="9.9.9.9", now=DAY + 5)["reason"])
        self.assertEqual(self.state["window"]["relayed"], [])                 # the claims' count is untouched

    def test_a_refused_or_dropped_burn_gives_its_slot_back(self):
        self.cli.submit_answer = {"ok": False, "reason": "stale nonce", "retry": True}
        self.assertTrue(self.go()["retry"])
        self.assertEqual((self.state["nullifiers"], self.state["window"]["burned"], self.state["inflight"]), ({}, [], None))
        self.cli.submit_answer = None
        self.assertTrue(self.go(now=DAY + 1)["ok"])
        self.chain.nonces["Claimer1"] = 5                                     # the burner's nonce moved: never included
        self.go(b=body(check_only=True), now=DAY + 30)
        g = self.state["nullifiers"]["exit:" + self.cli.exit]
        self.assertEqual(g["status"], "dropped")
        self.assertEqual(self.state["window"]["burned"], [])                 # the global slot comes back
        self.assertEqual(rl.count(self.state, rl.tags(self.state, "1.2.3.4", DAY + 30), "b", DAY + 30), 1)  # the sender's does not
        self.assertTrue(self.go(now=DAY + 40)["ok"])                         # relayed again (IP_DAILY_BURNS 2)

    def test_a_sender_cannot_loop_dropped_burns_past_its_own_daily_count(self):
        for i in range(2):
            self.cli.exit = "0x%064x" % (100 + i)                           # a fresh exit key each round
            self.assertTrue(self.go(now=DAY + 100 * i)["ok"])
            self.chain.nonces["Claimer1"] = self.chain.nonces.get("Claimer1", 0) + 1   # its own tx on that nonce: dropped
            self.go(b=body(check_only=True), now=DAY + 100 * i + 30)
            self.assertEqual(self.state["nullifiers"]["exit:" + self.cli.exit]["status"], "dropped")
        self.cli.exit = "0x%064x" % 200
        self.assertIn("too many relayed burns", self.go(now=DAY + 300)["reason"])
        self.assertTrue(self.go(ip="5.6.7.8", now=DAY + 301)["ok"])        # others are not held up

    def test_a_claim_in_flight_makes_a_burn_wait_and_the_reverse(self):
        claims = FakeCli(self.chain)
        self.assertTrue(rl.handle(body(), "9.9.9.9", self.state, now=DAY, cli=claims, desk=lambda: set(), chain=self.chain)["ok"])
        self.assertIn("stale nonce", self.go(now=DAY + 1)["reason"])
        self.chain.land(claims.hash())
        self.assertTrue(self.go(now=DAY + 20)["ok"])
        claims.nf = "0x" + "22" * 32
        self.assertIn("stale nonce", rl.handle(body(), "9.9.9.9", self.state, now=DAY + 21, cli=claims, desk=lambda: set(), chain=self.chain)["reason"])

    def test_a_check_refusal_and_a_non_burn_answer_are_refused(self):
        self.cli.check_answer = {"ok": False, "reason": "the burn does not notify this bridge's config", "retry": False}
        self.assertFalse(self.go()["ok"])
        self.cli.check_answer = {"ok": True, "nullifier": "0x" + "11" * 32}
        self.assertEqual(self.go(now=DAY + 1)["reason"], rl.GENERIC_ERROR)
        self.assertEqual(self.state["nullifiers"], {})

    def test_the_balance_floor_and_malformed_requests(self):
        self.assertFalse(self.go(b="{")["ok"])
        self.assertFalse(self.go(b=json.dumps({"tx": "zz"}))["ok"])
        self.assertEqual(self.cli.calls, [])
        real = FakeBurnCli.__call__
        self.cli.check_answer = dict(real(self.cli, TX, False, "burn"), payer_balance="1000")
        self.assertIn("out of funds", self.go()["reason"])

class Queue(unittest.TestCase):
    """L2: one request runs, RELAY_QUEUE wait, the rest are turned away at once."""

    def test_requests_beyond_the_queue_are_busy(self):
        real = rl.handle
        rl.handle = lambda body, ip, state: (time.sleep(0.6), {"ok": True, "checked": True})[1]
        srv = rl.Server(("127.0.0.1", 0), rl.Handler)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        out = []

        def post():
            c = http.client.HTTPConnection("127.0.0.1", srv.server_address[1], timeout=10)
            c.request("POST", "/relay", body(), {"Content-Type": "application/json"})
            out.append(json.loads(c.getresponse().read()))
            c.close()

        try:
            ts = [threading.Thread(target=post) for _ in range(5)]
            for t in ts:
                t.start()
            for t in ts:
                t.join()
        finally:
            rl.handle = real
            srv.shutdown()
            srv.server_close()
        busy = [o for o in out if not o.get("ok")]
        self.assertEqual((len(out), len(busy)), (5, 5 - (1 + rl.RELAY_QUEUE)))
        self.assertTrue(all(o["retry"] and "waiting" in o["reason"] for o in busy))



class OpenRelay(unittest.TestCase):
    """Branch open-relay: REQUIRE_REFUND asks relay-cosign to take only refunded claims (never for a
    burn), and the info answer says so; nothing in the relay is registered anywhere."""

    def setUp(self):
        self.seen, self.saved = [], (rl.subprocess.run, rl.REQUIRE_REFUND)

        def fake_run(args, **kw):
            self.seen.append(args)
            return types.SimpleNamespace(stdout='RELAY {"ok": true}\n', stderr="", returncode=0)
        rl.subprocess.run = fake_run

    def tearDown(self):
        rl.subprocess.run, rl.REQUIRE_REFUND = self.saved

    def test_require_refund_reaches_the_cli_for_claims_only(self):
        rl.REQUIRE_REFUND = True
        self.assertEqual(rl.run_cli(TX, False), {"ok": True})
        rl.run_cli(TX, True, kind="burn")
        self.assertIn("--require-refund", self.seen[0])
        self.assertNotIn("--require-refund", self.seen[1])
        rl.REQUIRE_REFUND = False
        rl.run_cli(TX, False)
        self.assertNotIn("--require-refund", self.seen[2])

if __name__ == "__main__":
    try:
        unittest.main()
    finally:
        shutil.rmtree(TMP, ignore_errors=True)
