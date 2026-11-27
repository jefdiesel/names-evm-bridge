#!/usr/bin/python3
"""Offline tests for faucet.py (stdlib unittest; no network, no CLI, no wallet).

  python3 -I public-demo/faucet/test_faucet.py      # from the repository root
"""
import importlib.util, json, os, subprocess, sys, tempfile, unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TMP = tempfile.mkdtemp(prefix="faucet-test-")
os.environ.update(P=TMP, FAUCET_STATE=TMP + "/faucet/state.json", FAUCET_LOG=TMP + "/faucet.log",
                  FAUCET_ACCOUNT="FAUcet1111111111111111111111111111111111111", FAUCET_AMOUNT="10000000",
                  PER_CLIENT_DAY="2", DAILY_MAX="3", MIN_BALANCE="1000", DRY_RUN="0")
spec = importlib.util.spec_from_file_location("faucet", os.path.join(HERE, "faucet.py"))
fc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fc)

DAY = 1_800_000_000
# BIP-340 x-only keys: the generator's x, and 2G's x (valid curve points)
G = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
G2 = "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5"
G3 = "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9"
G4 = "e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd13"


class Chain:
    def __init__(self, bal=10**12):
        self.bal, self.fail = bal, False

    def balance(self, account):
        if self.fail:
            raise OSError("down")
        return self.bal


class Cli:
    def __init__(self):
        self.sent, self.answer = [], None

    def __call__(self, pk):
        self.sent.append(pk)
        return self.answer or (True, "ab" * 32, 777, "")


def body(pk, **kw):
    return json.dumps(dict(public_key=pk, **kw))


class Faucet(unittest.TestCase):
    def setUp(self):
        self.s = {"accounts": {}, "grants": [], "clients": {}, "daykeys": {}}
        self.chain, self.cli = Chain(), Cli()

    def go(self, b, ip="1.2.3.4", now=DAY):
        return fc.handle(b, ip, self.s, now=now, cli=self.cli, chain=self.chain)

    def test_a_grant_then_already_today_then_again_after_a_day(self):
        out = self.go(body(G))
        self.assertEqual((out["ok"], out["tx"], out["block"], out["amount"]), (True, "ab" * 32, 777, "10000000"))
        self.assertEqual(out["account"], fc.account_id(bytes.fromhex(G)))
        self.assertEqual(self.cli.sent, [G])
        self.assertEqual(self.go(body(G), now=DAY + 60)["code"], "already_today")
        self.assertTrue(self.go(body(G), now=DAY + 86401)["ok"])

    def test_bad_keys_and_a_mismatched_account_are_refused_before_anything(self):
        for b in (body("00" * 32), body("zz"), body("ff" * 32), "{", json.dumps({"key": G}), body(G, account="Public/Other1111111111111111111111111111111111")):
            self.assertEqual(self.go(b)["code"], "bad_key", b)
        self.assertTrue(self.go(body(G, account="Public/" + fc.account_id(bytes.fromhex(G))))["ok"])
        self.assertEqual(len(self.cli.sent), 1)

    def test_per_client_and_daily_caps(self):
        self.assertTrue(self.go(body(G))["ok"])
        self.assertTrue(self.go(body(G2), now=DAY + 1)["ok"])
        self.assertEqual(self.go(body(G3), now=DAY + 2)["code"], "rate_limited")
        self.assertTrue(self.go(body(G3), ip="5.6.7.8", now=DAY + 3)["ok"])
        self.assertEqual(self.go(body(G4), ip="9.9.9.9", now=DAY + 4)["code"], "faucet_empty")   # DAILY_MAX 3
        info = fc.info(self.s, now=DAY + 6, chain=self.chain)
        self.assertEqual((info["available"], info["amount"], info["per_account_hours"]), (False, "10000000", 24))
        self.assertTrue(self.go(body(G4), ip="9.9.9.9", now=DAY + 86400 + 5)["ok"])               # rolling window

    def test_the_floor_and_a_down_chain(self):
        self.chain.bal = 10000000 + 999
        self.assertEqual(self.go(body(G))["code"], "faucet_empty")
        self.assertFalse(fc.info(self.s, now=DAY, chain=self.chain)["available"])
        self.chain.bal, self.chain.fail = 10**12, True
        self.assertEqual(self.go(body(G))["code"], "busy")
        self.assertEqual(self.cli.sent, [])
        self.chain.fail = False
        self.assertTrue(fc.info(self.s, now=DAY, chain=self.chain)["available"])

    def test_a_failed_send_gives_everything_back_and_a_lost_answer_is_kept(self):
        self.cli.answer = (False, None, None, "error: connection refused")
        out = self.go(body(G))
        self.assertEqual((out["code"], out["retry"]), ("busy", True))
        self.assertEqual((self.s["accounts"], self.s["grants"], self.s.get("inflight")), ({}, [], None))
        self.cli.answer = (False, "cd" * 32, None, "timed out waiting for the block")
        self.assertEqual(self.go(body(G), now=DAY + 5)["code"], "busy")
        acct = fc.account_id(bytes.fromhex(G))
        self.assertEqual(self.s["accounts"][acct]["status"], "uncertain")
        self.cli.answer = None
        self.assertEqual(self.go(body(G), now=DAY + 3 * 86400)["code"], "busy")   # never resent by itself
        self.assertEqual(len(self.cli.sent), 2)

    def hours(self, n):
        saved = fc.PER_ACCOUNT_HOURS
        fc.PER_ACCOUNT_HOURS = n
        self.addCleanup(setattr, fc, "PER_ACCOUNT_HOURS", saved)

    def test_per_account_hours_0_is_once_per_account_ever(self):
        # The live faucet's env since 2026-11-19: PER_ACCOUNT_HOURS=0, a grant is never forgotten.
        self.hours(0)
        self.assertTrue(self.go(body(G))["ok"])
        out = self.go(body(G), now=DAY + 365 * 86400)
        self.assertEqual((out["ok"], out["code"]), (False, "already_used"))
        self.assertIn(fc.account_id(bytes.fromhex(G)), self.s["accounts"], "prune keeps it")
        self.assertEqual(fc.info(self.s, now=DAY + 365 * 86400, chain=self.chain)["per_account_hours"], 0)
        self.assertEqual(self.cli.sent, [G])

    def test_for_grants_once_per_wallet_ever(self):
        # "for": the wallet the grant is for (LNS registrations); once per wallet, ever, even with
        # PER_ACCOUNT_HOURS 24 (the owners record is never pruned).
        w = fc.account_id(bytes.fromhex(G4))
        out = self.go(body(G, **{"for": "Public/" + w}))
        self.assertTrue(out["ok"], out)
        self.assertEqual(self.s["owners"], {w: {"at": DAY, "via": fc.account_id(bytes.fromhex(G))}})
        for i, b in enumerate((body(G2, **{"for": w}), body(G3, **{"for": "Public/" + w + " "}))):
            out = self.go(b, ip="5.6.7.%d" % i, now=DAY + (i + 2) * 86400)
            self.assertEqual(out["code"], "already_used", b)
        self.assertEqual(self.cli.sent, [G])
        self.assertEqual(self.go(body(G2, **{"for": "not a LEZ id!"}), ip="9.9.9.9")["code"], "bad_key")
        self.assertTrue(self.go(body(G2, **{"for": ""}), ip="9.9.9.9", now=DAY + 5)["ok"], "an empty for is no for")
        self.assertNotIn("", self.s["owners"])

    def test_for_a_wallet_whose_own_account_got_a_grant_is_refused(self):
        w = fc.account_id(bytes.fromhex(G3))
        self.assertTrue(self.go(body(G3))["ok"])
        self.assertEqual(self.go(body(G2, **{"for": w}), ip="5.6.7.8", now=DAY + 1)["code"], "already_used")
        # Known limit (2026-11-27): the reverse order is not refused. A grant for W through another
        # account, then W's own account asking without "for", gets a second grant.
        w2 = fc.account_id(bytes.fromhex(G4))
        self.assertTrue(self.go(body(G, **{"for": w2}), ip="9.9.9.9", now=DAY + 2)["ok"])
        self.assertTrue(self.go(body(G4), ip="9.9.9.8", now=DAY + 3)["ok"])
        self.assertEqual(self.cli.sent, [G3, G, G4])

    def test_for_is_given_back_on_a_failed_send_and_kept_on_a_lost_answer(self):
        w = fc.account_id(bytes.fromhex(G4))
        self.cli.answer = (False, None, None, "error: connection refused")
        self.assertEqual(self.go(body(G, **{"for": w}))["code"], "busy")
        self.assertEqual(self.s["owners"], {})
        self.cli.answer = (False, "cd" * 32, None, "timed out waiting for the block")
        self.assertEqual(self.go(body(G, **{"for": w}), now=DAY + 5)["code"], "busy")
        self.assertIn(w, self.s["owners"])
        self.cli.answer = None
        self.assertEqual(self.go(body(G2, **{"for": w}), ip="5.6.7.8", now=DAY + 6)["code"], "already_used")

    def test_owners_survive_a_restart_and_an_old_state_file_still_loads(self):
        w = fc.account_id(bytes.fromhex(G4))
        self.assertTrue(self.go(body(G, **{"for": w}))["ok"])
        s = fc.load_state()
        self.assertEqual(s["owners"], self.s["owners"])
        fc.save_state({"accounts": {}, "grants": [], "clients": {}, "daykeys": {}})   # a state from before "for"
        self.s = fc.load_state()
        self.assertTrue(self.go(body(G2, **{"for": w}), now=DAY + 1)["ok"])

    def test_release_forgets_the_account_but_not_the_wallet(self):
        # Known limit (2026-11-27): `faucet.py --release <account>` (the operator checked an uncertain
        # transfer) drops the account's record only; a wallet named by "for" stays used.
        w = fc.account_id(bytes.fromhex(G4))
        self.cli.answer = (False, "cd" * 32, None, "timed out waiting for the block")
        self.go(body(G, **{"for": w}))
        acct = fc.account_id(bytes.fromhex(G))
        r = subprocess.run([sys.executable, "-I", os.path.join(HERE, "faucet.py"), "--release", acct],
                           env=dict(os.environ), capture_output=True, text=True, timeout=60)
        self.assertEqual((r.returncode, r.stdout.strip()), (0, "released"), r.stderr)
        s = fc.load_state()
        self.assertNotIn(acct, s["accounts"])
        self.assertIn(w, s["owners"])

    def test_no_key_and_no_client_address_in_state_or_log(self):
        self.go(body(G), ip="203.0.113.77")
        text = json.dumps(self.s) + (open(os.environ["FAUCET_LOG"]).read() if os.path.exists(os.environ["FAUCET_LOG"]) else "")
        self.assertNotIn("203.0.113.77", text)
        self.assertNotIn(G, text)
        self.assertEqual(os.stat(os.environ["FAUCET_STATE"]).st_mode & 0o777, 0o600)


if __name__ == "__main__":
    unittest.main()
