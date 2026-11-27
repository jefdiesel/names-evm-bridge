#!/usr/bin/env python3
"""Offline tests for lez-rpc-proxy.py: a fake sequencer on loopback, the proxy in front of it,
and the cases test-loopback.sh runs against a real chain. Stdlib only.

  python3 -I public-demo/proxy/test_proxy.py      # from the repository root
(The testnet kit's test_proxy.py, plus RelayTest for this repository's --relay methods, and
ClientTableTest / ClientKeyTest for the 2026-11-14 review's M4 and M5.)
"""
import http.client
import importlib.util
import json
import os
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("lez_rpc_proxy", os.path.join(HERE, "lez-rpc-proxy.py"))
proxy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proxy)


class FakeSequencer(BaseHTTPRequestHandler):
    """Answers every JSON-RPC call with {"result": <method name>} and records what it received."""
    seen = []

    def log_message(self, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        FakeSequencer.seen.append(body)
        answer = lambda b: {"jsonrpc": "2.0", "id": b.get("id"), "result": b.get("method")}  # noqa: E731
        out = json.dumps([answer(b) for b in body] if isinstance(body, list) else answer(body)).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)


def serve(server):
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    return server


class ProxyTest(unittest.TestCase):
    def setUp(self, **overrides):
        self.upstream = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeSequencer))
        cfg = dict(proxy.DEFAULTS)
        cfg.update(overrides)
        self.srv = serve(proxy.Server(("127.0.0.1", 0), proxy.Handler, cfg,
                                      ("127.0.0.1", self.upstream.server_address[1]), None,
                                      getattr(self, "allowed", None)))
        self.port = self.srv.server_address[1]
        FakeSequencer.seen = []

    def tearDown(self):
        self.srv.shutdown()
        self.srv.server_close()
        self.upstream.shutdown()
        self.upstream.server_close()

    def post(self, body, headers=None, method="POST", path="/"):
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        h = {"Content-Type": "application/json"}
        h.update(headers or {})
        data = body if isinstance(body, bytes) else json.dumps(body).encode()
        try:
            conn.request(method, path, data if method == "POST" else None, h)
            r = conn.getresponse()
        except (BrokenPipeError, ConnectionResetError):
            # The proxy refused before reading the whole body (a size cap) and closed the socket.
            conn.close()
            return None, b""
        out = r.read()
        conn.close()
        return r.status, out

    def call(self, method, params=None, rid=1, **kw):
        return self.post({"jsonrpc": "2.0", "id": rid, "method": method, "params": params or []}, **kw)

    def test_allowed_method_is_forwarded_and_reserialised(self):
        status, out = self.post({"jsonrpc": "2.0", "id": 7, "method": "getLastBlockId", "params": [], "extra": "x"})
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(out)["result"], "getLastBlockId")
        self.assertEqual(FakeSequencer.seen[-1], {"jsonrpc": "2.0", "id": 7, "method": "getLastBlockId", "params": []})

    def test_every_allowed_method_passes(self):
        for m in sorted(proxy.ALLOWED - {"getBlockRange"}):
            self.assertEqual(self.call(m)[0], 200, m)

    def test_operator_and_unknown_methods_refused(self):
        for m in ("requeueCrossZoneDeadLetter", "getCrossZoneDeadLetters", "rpc.discover", "admin_x"):
            self.assertEqual(self.call(m)[0], 403, m)
        self.assertEqual(FakeSequencer.seen, [])

    @staticmethod
    def calls(n, method="getAccount"):
        return [{"jsonrpc": "2.0", "id": i, "method": method, "params": ["A"]} for i in range(n)]

    def test_batch_forwarded(self):
        status, out = self.post(self.calls(2) + [{"jsonrpc": "2.0", "id": "x", "method": "getLastBlockId", "params": [], "extra": 1}])
        self.assertEqual(status, 200)
        self.assertEqual([r["result"] for r in json.loads(out)], ["getAccount", "getAccount", "getLastBlockId"])
        self.assertEqual(FakeSequencer.seen[-1][2], {"jsonrpc": "2.0", "id": "x", "method": "getLastBlockId", "params": []})

    def test_batch_refusals(self):
        self.assertEqual(self.post([])[0], 400)                                    # empty
        self.assertEqual(self.post(self.calls(proxy.DEFAULTS["MAX_BATCH"]))[0], 200)
        self.assertEqual(self.post(self.calls(proxy.DEFAULTS["MAX_BATCH"] + 1))[0], 400)  # too many
        self.assertEqual(self.post([self.calls(1)])[0], 400)                       # nested array
        self.assertEqual(self.post(self.calls(2) + [7])[0], 400)                   # not an object
        bad = self.calls(2) + [{"jsonrpc": "2.0", "id": 9, "method": "requeueCrossZoneDeadLetter", "params": []}]
        self.assertEqual(self.post(bad)[0], 403)                                   # one bad call refuses all
        wide = self.calls(1) + [{"jsonrpc": "2.0", "id": 9, "method": "getBlockRange", "params": [0, 100]}]
        self.assertEqual(self.post(wide)[0], 400)
        n = len(FakeSequencer.seen)
        self.assertEqual(self.post([{"jsonrpc": "2.0", "id": 1, "method": "getAccount", "params": ["A" * 9000]}] * 2)[0], 413)  # whole body over the read cap
        self.assertEqual(len(FakeSequencer.seen), n)                               # nothing refused went upstream

    def test_batch_counts_as_n_requests(self):
        ip = {"CF-Connecting-IP": "198.51.100.20"}
        statuses = [self.post(self.calls(20), headers=ip)[0] for _ in range(4)]
        self.assertEqual(statuses[:3], [200, 200, 200])   # 3 x 20 = the burst of 60
        self.assertEqual(statuses[3], 429)


class NoBatchTest(ProxyTest):
    def setUp(self):
        super().setUp(MAX_BATCH=0)

    def test_batch_forwarded(self):
        self.assertEqual(self.post(self.calls(2))[0], 400)

    def test_batch_refusals(self):
        self.assertEqual(self.post(self.calls(1))[0], 400)

    def test_batch_counts_as_n_requests(self):
        pass

    def test_block_range_span(self):
        self.assertEqual(self.call("getBlockRange", [0, 99])[0], 200)
        self.assertEqual(self.call("getBlockRange", [0, 100])[0], 400)
        self.assertEqual(self.call("getBlockRange", [5, 4])[0], 400)
        self.assertEqual(self.call("getBlockRange", {"start_block_id": 1, "end_block_id": 2})[0], 200)

    def test_methods_paths_and_types(self):
        self.assertEqual(self.post(b"", method="GET")[0], 405)
        self.assertEqual(self.post({"jsonrpc": "2.0", "id": 1, "method": "getLastBlockId"}, path="/x")[0], 404)
        self.assertEqual(self.call("getLastBlockId", headers={"Content-Type": "text/plain"})[0], 415)
        self.assertEqual(self.call("getLastBlockId", headers={"Upgrade": "websocket"})[0], 400)
        self.assertEqual(self.post(b"{not json")[0], 400)
        self.assertEqual(self.post(b"[" * 100000 + b"]" * 100000)[0], 400)   # deep nesting
        self.assertEqual(self.call("getLastBlockId", rid=True)[0], 400)

    def test_body_caps(self):
        big_read = {"jsonrpc": "2.0", "id": 1, "method": "getAccount", "params": ["A" * 20000]}
        self.assertIn(self.post(big_read)[0], (413, None))
        big_send = {"jsonrpc": "2.0", "id": 1, "method": "sendTransaction", "params": ["A" * 600000]}
        self.assertEqual(self.post(big_send)[0], 200)
        huge = {"jsonrpc": "2.0", "id": 1, "method": "sendTransaction", "params": ["A" * 800000]}
        self.assertIn(self.post(huge)[0], (413, None))

    def test_healthz_only_without_client_header(self):
        self.assertEqual(self.post(b"", method="GET", path="/healthz")[0], 200)
        self.assertEqual(self.post(b"", method="GET", path="/healthz", headers={"CF-Connecting-IP": "203.0.113.9"})[0], 405)

    def test_per_client_rate_limit(self):
        statuses = [self.call("getLastBlockId", headers={"CF-Connecting-IP": "198.51.100.7"})[0] for _ in range(80)]
        self.assertIn(429, statuses)
        self.assertEqual(statuses[0], 200)
        # another client is unaffected
        self.assertEqual(self.call("getLastBlockId", headers={"CF-Connecting-IP": "198.51.100.8"})[0], 200)

    def test_send_bucket(self):
        statuses = [self.call("sendTransaction", ["AAAA"], headers={"CF-Connecting-IP": "198.51.100.9"})[0] for _ in range(8)]
        self.assertEqual(statuses[:4], [200] * 4)
        self.assertIn(429, statuses[4:])


class AllowMethodTest(ProxyTest):
    allowed = proxy.ALLOWED | {"getCrossZoneDeadLetters"}

    def test_operator_and_unknown_methods_refused(self):
        self.assertEqual(self.call("getCrossZoneDeadLetters")[0], 200)
        self.assertEqual(self.call("requeueCrossZoneDeadLetter")[0], 403)


class FakeRelay(BaseHTTPRequestHandler):
    """The claim relay's HTTP side: GET /relay/info, POST /relay; records what it received."""
    seen = []
    auth = []

    def log_message(self, *args):
        pass

    def answer(self, obj):
        out = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)

    def do_GET(self):
        FakeRelay.seen.append(("GET", self.path, self.headers.get("CF-Connecting-IP"), None))
        self.answer({"payer": "RP", "max_gas": 250000, "max_fee": "2500000"})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        FakeRelay.seen.append(("POST", self.path, self.headers.get("CF-Connecting-IP"), body))
        FakeRelay.auth.append(self.headers.get("X-Relay-Auth"))
        self.answer({"ok": True, "tx_hash": "ee" * 32} if not body["check_only"] else {"ok": True, "checked": True})


class FakeRelayV6(FakeRelay):
    """A second relay, of one bridge (--relay-bridge): records apart, names its bridge."""
    seen = []

    def do_GET(self):
        FakeRelayV6.seen.append(("GET", self.path))
        self.answer({"payer": "RP6", "bridge": V6, "max_gas": 500000, "max_fee": "5000000"})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        FakeRelayV6.seen.append(("POST", self.path, len(body["tx"])))
        self.answer({"ok": True, "tx_hash": "66" * 32})


V6 = "4PxaLAfPZDvpfZaR8SsC2Y7pYMPNwPPGVGMpjEVPQWed"


class RoutedRelayTest(unittest.TestCase):
    """--relay-bridge: a call naming a routed bridge reaches its own relay (with the larger cap);
    every call naming none, or another bridge, reaches --relay exactly as before."""

    def setUp(self):
        self.upstream = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeSequencer))
        self.relay = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeRelay))
        self.relay6 = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeRelayV6))
        self.srv = serve(proxy.Server(("127.0.0.1", 0), proxy.Handler, dict(proxy.DEFAULTS),
                                      ("127.0.0.1", self.upstream.server_address[1]), None, None, "CF-Connecting-IP",
                                      ("127.0.0.1", self.relay.server_address[1]), None, (), None, None,
                                      {V6: ("127.0.0.1", self.relay6.server_address[1])}))
        self.port = self.srv.server_address[1]
        FakeSequencer.seen, FakeRelay.seen, FakeRelay.auth, FakeRelayV6.seen = [], [], [], []

    def tearDown(self):
        for x in (self.srv, self.upstream, self.relay, self.relay6):
            x.shutdown()
            x.server_close()

    post = ProxyTest.post
    call = ProxyTest.call

    def test_named_calls_reach_the_bridge_s_relay_and_the_rest_the_default(self):
        ip = lambda n: {"CF-Connecting-IP": "198.51.100.%d" % n}
        res = lambda r: json.loads(r[1])["result"]
        self.assertEqual(res(self.call("bridge_relayInfo", [V6], headers=ip(60)))["bridge"], V6)
        self.assertEqual(res(self.call("bridge_relayInfo", {"bridge": V6}, headers=ip(60)))["bridge"], V6)
        self.assertNotIn("bridge", res(self.call("bridge_relayInfo", [], headers=ip(60))))   # the default relay
        other = "CuV1ctZ2qJG4wQcXP4TBudVGKzNmdoYT3DpP1veHnDMA"
        self.assertEqual(res(self.call("bridge_relayInfo", [other], headers=ip(60)))["payer"], "RP")   # no route: default
        self.assertEqual(self.call("bridge_relayInfo", ["not a bridge!"], headers=ip(60))[0], 400)
        # An attested claim (about 7.3 KB, 14,600 hex characters) and one near the routed cap.
        r = self.call("bridge_relayClaim", ["ab" * 7300, False, V6], headers=ip(61))
        self.assertEqual((r[0], res(r)["tx_hash"]), (200, "66" * 32))
        status, out = self.call("bridge_relayClaim", {"tx": "ab" * 12000, "check_only": False, "bridge": V6}, headers=ip(62))
        self.assertEqual(status, 200)
        self.assertEqual(FakeRelayV6.seen[-2:], [("POST", "/relay", 14600), ("POST", "/relay", 24000)])
        # Past the routed cap, and the default relay keeps its 8,192-byte cap.
        self.assertEqual(self.call("bridge_relayClaim", ["ab" * 12290, False, V6], headers=ip(63))[0], 400)
        self.assertEqual(self.call("bridge_relayClaim", ["ab" * 8194], headers=ip(64))[0], 400)
        self.assertEqual(self.call("bridge_relayClaim", ["ab" * 8000], headers=ip(65))[0], 200)
        self.assertEqual([x[1] for x in FakeRelay.seen if x[0] == "POST"], ["/relay"])
        self.assertEqual(self.call("bridge_relayClaim", ["ab", False, V6, "extra"], headers=ip(66))[0], 400)
        self.assertEqual(FakeSequencer.seen, [], "nothing reached the sequencer")

    def test_relay_route_takes_a_base58_bridge_and_a_loopback_relay(self):
        self.assertEqual(proxy.relay_route(V6 + "=127.0.0.1:3296"), (V6, ("127.0.0.1", 3296)))
        for bad in ("127.0.0.1:3296", "0OIl" * 10 + "=127.0.0.1:3296", V6 + "=", V6 + "=192.0.2.1:3296"):
            with self.assertRaises(Exception, msg=bad):
                proxy.relay_route(bad)

    def test_a_malformed_bridge_name_reaches_no_relay(self):
        ip = lambda n: {"CF-Connecting-IP": "198.51.100.%d" % n}
        for i, params in enumerate((["ab", False, "not a bridge!"], ["ab", False, 5],
                                    {"tx": "ab", "check_only": False, "bridge": "0" * 44})):
            self.assertEqual(self.call("bridge_relayClaim", params, headers=ip(80 + i))[0], 400, str(params))
        self.assertEqual(self.call("bridge_relayInfo", {"bridge": 5}, headers=ip(84))[0], 400)
        # Known defect (2026-11-27): a list or object as the bridge is looked up in the route table
        # before its type is checked, so the handler raises (TypeError, counted as handler_error)
        # and the client gets no answer instead of a 400. Nothing reaches a relay and the proxy serves on.
        for i, params in enumerate((["ab", False, ["x"]], {"tx": "ab", "check_only": False, "bridge": {"a": 1}})):
            self.assertIn(self.call("bridge_relayClaim", params, headers=ip(85 + i))[0], (400, None), str(params))
        self.assertGreaterEqual(self.srv.limits.counters.get("handler_error", 0), 1)
        self.assertEqual(self.call("bridge_relayInfo", [V6], headers=ip(87))[0], 200)
        self.assertEqual([x for x in FakeRelay.seen + FakeRelayV6.seen if x[0] == "POST"], [])


class FakeRelayV7(FakeRelay):
    """A third relay, of the v7 bridge (demo.sh's relayer-v7, on 127.0.0.1:3300)."""
    seen = []

    def do_GET(self):
        FakeRelayV7.seen.append(("GET", self.path))
        self.answer({"payer": "RP7", "bridge": V7, "max_gas": 500000, "max_fee": "5000000", "burns": True})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        FakeRelayV7.seen.append(("POST", self.path, len(body["tx"])))
        self.answer({"ok": True, "tx_hash": "77" * 32})


V7 = "7" * 44   # a stand-in program id (base58)


class TwoRoutedRelaysTest(unittest.TestCase):
    """Two --relay-bridge routes (v6 and v7, as demo.sh starts them): claims AND burns that name a
    bridge reach that bridge's relay (a burn on /relay/burn); unnamed ones the default relay."""

    def setUp(self):
        self.upstream = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeSequencer))
        self.relay = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeRelay))
        self.relay6 = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeRelayV6))
        self.relay7 = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeRelayV7))
        self.srv = serve(proxy.Server(("127.0.0.1", 0), proxy.Handler, dict(proxy.DEFAULTS),
                                      ("127.0.0.1", self.upstream.server_address[1]), None, None, "CF-Connecting-IP",
                                      ("127.0.0.1", self.relay.server_address[1]), None, (), None, None,
                                      {V6: ("127.0.0.1", self.relay6.server_address[1]), V7: ("127.0.0.1", self.relay7.server_address[1])}))
        self.port = self.srv.server_address[1]
        FakeSequencer.seen, FakeRelay.seen, FakeRelay.auth, FakeRelayV6.seen, FakeRelayV7.seen = [], [], [], [], []

    def tearDown(self):
        for x in (self.srv, self.upstream, self.relay, self.relay6, self.relay7):
            x.shutdown()
            x.server_close()

    post = ProxyTest.post
    call = ProxyTest.call

    def test_burns_and_claims_naming_v7_reach_the_v7_relay(self):
        ip = lambda n: {"CF-Connecting-IP": "198.51.100.%d" % n}
        res = lambda r: json.loads(r[1])["result"]
        self.assertEqual(res(self.call("bridge_relayInfo", [V7], headers=ip(70)))["payer"], "RP7")
        self.assertEqual(res(self.call("bridge_relayInfo", [V6], headers=ip(70)))["payer"], "RP6")
        self.assertEqual(res(self.call("bridge_relayInfo", [], headers=ip(70)))["payer"], "RP")
        r = self.call("bridge_relayBurn", ["ab" * 400, False, V7], headers=ip(71))
        self.assertEqual((r[0], res(r)["tx_hash"]), (200, "77" * 32))
        r = self.call("bridge_relayClaim", ["ab" * 7300, False, V7], headers=ip(72))
        self.assertEqual((r[0], res(r)["tx_hash"]), (200, "77" * 32))
        self.assertEqual(FakeRelayV7.seen[-2:], [("POST", "/relay/burn", 800), ("POST", "/relay", 14600)])
        # An unnamed burn (today's installed module) still reaches the default relay, never v7's.
        self.assertEqual(self.call("bridge_relayBurn", ["ab" * 400, False], headers=ip(73))[0], 200)
        self.assertEqual([x[1] for x in FakeRelay.seen if x[0] == "POST"], ["/relay/burn"])
        self.assertEqual([x for x in FakeRelayV6.seen if x[0] == "POST"], [])
        self.assertEqual(FakeSequencer.seen, [], "nothing reached the sequencer")


class RelayTest(unittest.TestCase):
    relay_auth = None

    def setUp(self):
        self.upstream = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeSequencer))
        self.relay = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeRelay))
        self.srv = serve(proxy.Server(("127.0.0.1", 0), proxy.Handler, dict(proxy.DEFAULTS),
                                      ("127.0.0.1", self.upstream.server_address[1]), None, None, "CF-Connecting-IP",
                                      ("127.0.0.1", self.relay.server_address[1]), self.relay_auth))
        self.port = self.srv.server_address[1]
        FakeSequencer.seen, FakeRelay.seen, FakeRelay.auth = [], [], []

    def tearDown(self):
        for x in (self.srv, self.upstream, self.relay):
            x.shutdown()
            x.server_close()

    post = ProxyTest.post
    call = ProxyTest.call

    def test_info_and_claim_reach_the_relay_never_the_sequencer(self):
        ip = {"CF-Connecting-IP": "198.51.100.30"}
        status, out = self.call("bridge_relayInfo", headers=ip)
        self.assertEqual((status, json.loads(out)["result"]["payer"]), (200, "RP"))
        status, out = self.call("bridge_relayClaim", ["ab" * 1000], rid=5, headers=ip)
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(out), {"jsonrpc": "2.0", "id": 5, "result": {"ok": True, "tx_hash": "ee" * 32}})
        status, out = self.call("bridge_relayClaim", {"tx": "abcd", "check_only": True}, headers=ip)
        self.assertEqual(json.loads(out)["result"], {"ok": True, "checked": True})
        self.assertEqual([x[:3] for x in FakeRelay.seen], [("GET", "/relay/info", "198.51.100.30"),
                                                           ("POST", "/relay", "198.51.100.30"), ("POST", "/relay", "198.51.100.30")])
        self.assertEqual(FakeRelay.seen[1][3], {"tx": "ab" * 1000, "check_only": False})
        self.assertEqual(FakeSequencer.seen, [])

    def test_a_burn_reaches_the_relays_burn_path(self):
        ip = {"CF-Connecting-IP": "198.51.100.35"}
        status, out = self.call("bridge_relayBurn", ["cd" * 400], rid=6, headers=ip)
        self.assertEqual((status, json.loads(out)["result"]), (200, {"ok": True, "tx_hash": "ee" * 32}))
        self.assertEqual(FakeRelay.seen[-1][:2], ("POST", "/relay/burn"))
        self.assertEqual(FakeRelay.seen[-1][3], {"tx": "cd" * 400, "check_only": False})
        self.assertIn(self.call("bridge_relayBurn", ["zz"], headers={"CF-Connecting-IP": "198.51.100.36"})[0], (400, None))
        self.assertEqual(FakeSequencer.seen, [])

    def test_malformed_relay_calls_never_reach_the_relay(self):
        for i, params in enumerate(([], [5], ["zz"], ["ab", "yes"], ["ab", True, 1], ["ab" * 9000], {"tx": 1})):
            status = self.call("bridge_relayClaim", params, headers={"CF-Connecting-IP": "198.51.100.%d" % (40 + i)})[0]
            self.assertIn(status, (400, 413, None), str(params)[:40])
        self.assertEqual(self.post([{"jsonrpc": "2.0", "id": 1, "method": "bridge_relayInfo", "params": []}])[0], 400)  # no batches
        self.assertEqual(FakeRelay.seen, [])

    def test_relay_bucket(self):
        ip = {"CF-Connecting-IP": "198.51.100.31"}
        statuses = [self.call("bridge_relayClaim", ["abcd"], headers=ip)[0] for _ in range(7)]
        self.assertEqual(statuses[:5], [200] * 5)
        self.assertEqual(statuses[5:], [429, 429])
        self.assertEqual([self.call("bridge_relayInfo", headers=ip)[0] for _ in range(3)], [200] * 3)   # a read: the general budget only
        self.assertEqual(self.call("getLastBlockId", headers=ip)[0], 200)   # the sequencer methods are unaffected

    def test_without_relay_the_methods_are_unknown(self):
        self.srv.relay = None
        self.srv.allowed -= proxy.RELAY_METHODS
        self.assertEqual(self.call("bridge_relayInfo")[0], 403)

    def test_no_shared_secret_is_sent_without_one(self):
        self.call("bridge_relayClaim", ["abcd"], headers={"CF-Connecting-IP": "198.51.100.32"})
        self.assertEqual(FakeRelay.auth, [None])


class FakeFaucet(BaseHTTPRequestHandler):
    """The faucet's HTTP side: GET /faucet/info, POST /faucet; records what it received."""
    seen = []

    def log_message(self, *args):
        pass

    def answer(self, obj):
        out = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)

    def do_GET(self):
        FakeFaucet.seen.append(("GET", self.path, self.headers.get("CF-Connecting-IP"), None, self.headers.get("X-Relay-Auth")))
        self.answer({"amount": "10000000", "per_account_hours": 24, "available": True, "reason": None})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        FakeFaucet.seen.append(("POST", self.path, self.headers.get("CF-Connecting-IP"), body, self.headers.get("X-Relay-Auth")))
        self.answer({"ok": True, "tx": "ab" * 32, "block": 7, "amount": "10000000"})


class FaucetTest(unittest.TestCase):
    PK = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"

    def setUp(self):
        self.upstream = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeSequencer))
        self.faucet = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeFaucet))
        self.srv = serve(proxy.Server(("127.0.0.1", 0), proxy.Handler, dict(proxy.DEFAULTS),
                                      ("127.0.0.1", self.upstream.server_address[1]), None, None, "CF-Connecting-IP",
                                      None, "s" * 40, (), ("127.0.0.1", self.faucet.server_address[1])))
        self.port = self.srv.server_address[1]
        FakeSequencer.seen, FakeFaucet.seen = [], []

    def tearDown(self):
        for x in (self.srv, self.upstream, self.faucet):
            x.shutdown()
            x.server_close()

    post = ProxyTest.post
    call = ProxyTest.call

    def test_info_and_a_grant_reach_the_faucet_with_the_client_address(self):
        ip = {"CF-Connecting-IP": "198.51.100.50"}
        status, out = self.call("bridge_faucetInfo", headers=ip)
        self.assertEqual((status, json.loads(out)["result"]["available"]), (200, True))
        status, out = self.call("bridge_faucet", [self.PK], rid=3, headers=ip)
        self.assertEqual(json.loads(out), {"jsonrpc": "2.0", "id": 3, "result": {"ok": True, "tx": "ab" * 32, "block": 7, "amount": "10000000"}})
        self.call("bridge_faucet", {"public_key": self.PK, "account": "Public/Abc"}, headers=ip)
        self.assertEqual([x[:3] for x in FakeFaucet.seen], [("GET", "/faucet/info", "198.51.100.50"), ("POST", "/faucet", "198.51.100.50"), ("POST", "/faucet", "198.51.100.50")])
        self.assertEqual(FakeFaucet.seen[1][3], {"public_key": self.PK})
        self.assertEqual(FakeFaucet.seen[2][3], {"public_key": self.PK, "account": "Public/Abc"})
        self.assertEqual(FakeFaucet.seen[1][4], "s" * 40)   # the shared secret, so the faucet believes the address
        self.assertEqual(FakeSequencer.seen, [])

    def test_the_faucet_bucket_and_malformed_calls(self):
        ip = {"CF-Connecting-IP": "198.51.100.51"}
        statuses = [self.call("bridge_faucet", [self.PK], headers=ip)[0] for _ in range(5)]
        self.assertEqual(statuses, [200, 200, 200, 429, 429])
        self.assertEqual(self.call("bridge_faucetInfo", headers=ip)[0], 200)   # info: the general budget only
        # Three params are valid since 2026-11-19 ([key, account or null, the wallet it is for]); four are not.
        malformed = ([], [5], ["zz"], ["ab" * 40], [self.PK, 5], [self.PK, "a", "b", "c"], [self.PK, None, 5],
                     [self.PK, None, "W" * 61], {"public_key": self.PK, "for": ["W"]})
        for i, params in enumerate(malformed):
            self.assertIn(self.call("bridge_faucet", params, headers={"CF-Connecting-IP": "198.51.100.%d" % (60 + i)})[0], (400, None), str(params))
        self.assertEqual([x for x in FakeFaucet.seen if x[0] == "POST"][3:], [], "no malformed call reached the faucet")
        self.assertEqual(self.call("bridge_faucet", [self.PK, "a", "b"], headers={"CF-Connecting-IP": "198.51.100.70"})[0], 200)
        self.assertEqual(self.post([{"jsonrpc": "2.0", "id": 1, "method": "bridge_faucet", "params": [self.PK]}])[0], 400)   # no batches

    def test_the_wallet_it_is_for_is_forwarded(self):
        # The "for" param (the faucet grants once per wallet, ever): list form with or without an account, dict form.
        ip = lambda n: {"CF-Connecting-IP": "198.51.100.%d" % n}
        owner = "Public/" + "W" * 43
        for i, params in enumerate(([self.PK, None, owner], [self.PK, "Public/Abc", owner],
                                    {"public_key": self.PK, "for": owner}, [self.PK, None, None])):
            self.assertEqual(self.call("bridge_faucet", params, headers=ip(90 + i))[0], 200, str(params))
        self.assertEqual([x[3] for x in FakeFaucet.seen], [
            {"public_key": self.PK, "for": owner},
            {"public_key": self.PK, "account": "Public/Abc", "for": owner},
            {"public_key": self.PK, "for": owner},
            {"public_key": self.PK}])
        self.assertEqual(FakeSequencer.seen, [])

    def test_without_the_faucet_the_methods_are_unknown(self):
        self.srv.allowed -= proxy.FAUCET_METHODS
        self.assertNotEqual(self.call("bridge_faucetInfo", headers={"CF-Connecting-IP": "198.51.100.52"})[0], 200)


class FakeSettle(BaseHTTPRequestHandler):
    """The settlement service's HTTP side; records what it received."""
    seen = []

    def log_message(self, *args):
        pass

    def answer(self, obj):
        out = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)

    def do_GET(self):
        FakeSettle.seen.append(("GET", self.path, None))
        self.answer({"available": True, "reason": None, "fee": "0", "vaults": []})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        FakeSettle.seen.append(("POST", self.path, body))
        if self.path == "/settle/bundle":
            return self.answer({"ok": True, "bundle": "QUJD" * 100000})   # 400 KB of base64
        self.answer({"ok": True, "id": "0x" + "ab" * 32, "stage": "queued"})


class SettleTest(unittest.TestCase):
    def setUp(self):
        self.upstream = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeSequencer))
        self.settle = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeSettle))
        self.srv = serve(proxy.Server(("127.0.0.1", 0), proxy.Handler, dict(proxy.DEFAULTS),
                                      ("127.0.0.1", self.upstream.server_address[1]), None, None, "CF-Connecting-IP",
                                      None, "s" * 40, (), None, ("127.0.0.1", self.settle.server_address[1])))
        self.port = self.srv.server_address[1]
        FakeSequencer.seen, FakeSettle.seen = [], []

    def tearDown(self):
        for x in (self.srv, self.upstream, self.settle):
            x.shutdown()
            x.server_close()

    post = ProxyTest.post
    call = ProxyTest.call

    def test_the_four_methods_reach_the_service(self):
        ip = {"CF-Connecting-IP": "198.51.100.70"}
        self.assertEqual(json.loads(self.call("bridge_settleInfo", headers=ip)[1])["result"]["available"], True)
        receipt = {"vault": "0x" + "af" * 20, "receipt_hex": "ab" * 225000, "journal_hex": "cd" * 256}
        status, out = self.call("bridge_settle", [receipt], headers=ip)
        self.assertEqual((status, json.loads(out)["result"]["stage"]), (200, "queued"))
        self.call("bridge_settle", [{"vault": "0x" + "af" * 20, "exit": {"secret": "0x01"}}], headers=ip)
        self.call("bridge_settleStatus", ["0x" + "ab" * 32], headers=ip)
        status, out = self.call("bridge_settleBundle", ["0" * 32], headers=ip)
        self.assertEqual((status, len(json.loads(out)["result"]["bundle"])), (200, 400000), "a bundle larger than the relay's answer cap")
        self.assertEqual([(m, p) for m, p, _ in FakeSettle.seen], [("GET", "/settle/info"), ("POST", "/settle"), ("POST", "/settle"),
                                                                ("POST", "/settle/status"), ("POST", "/settle/bundle")])
        self.assertEqual(FakeSettle.seen[1][2]["receipt_hex"], "ab" * 225000)
        self.assertEqual(FakeSettle.seen[3][2], {"id": "0x" + "ab" * 32})
        self.assertEqual(FakeSequencer.seen, [])

    def test_the_settle_bucket_and_malformed_calls(self):
        ip = {"CF-Connecting-IP": "198.51.100.71"}
        ok = [{"vault": "0x" + "af" * 20, "receipt_hex": "ab", "journal_hex": "cd"}]
        self.assertEqual([self.call("bridge_settle", ok, headers=ip)[0] for _ in range(5)], [200, 200, 200, 429, 429])
        self.assertEqual(self.call("bridge_settleStatus", ["0xab"], headers=ip)[0], 200)   # status: the general budget only
        for i, params in enumerate(([], [5], [{"vault": "x"}], [{"vault": "x", "receipt_hex": "a", "journal_hex": "b", "exit": {}}], ["zz"])):
            self.assertIn(self.call("bridge_settle", params, headers={"CF-Connecting-IP": "198.51.100.%d" % (80 + i)})[0], (400, None), str(params))
        self.assertIn(self.call("bridge_settleStatus", ["not hex"], headers={"CF-Connecting-IP": "198.51.100.90"})[0], (400, None))


class RelayAuthTest(RelayTest):
    """With --relay-auth-file the relay gets the shared secret, so it may believe CF-Connecting-IP."""
    relay_auth = "k" * 48

    def test_no_shared_secret_is_sent_without_one(self):
        # A client's own X-Relay-Auth is never passed on: the filter builds the relay's headers.
        self.call("bridge_relayClaim", ["abcd"], headers={"CF-Connecting-IP": "198.51.100.33", "X-Relay-Auth": "forged" * 8})
        self.assertEqual(FakeRelay.auth, ["k" * 48])

    def test_the_secret_comes_from_a_600_file(self):
        d = tempfile.mkdtemp(prefix="proxy-auth-")
        p = os.path.join(d, "auth")
        with open(p, "w") as f:
            f.write("q" * 40 + "\n")
        os.chmod(p, 0o644)
        with self.assertRaises(SystemExit):
            proxy.relay_secret(p)
        os.chmod(p, 0o600)
        self.assertEqual(proxy.relay_secret(p), "q" * 40)
        with open(p, "w") as f:
            f.write("short")
        with self.assertRaises(SystemExit):
            proxy.relay_secret(p)
        os.unlink(p)
        os.rmdir(d)


class ClientKeyTest(unittest.TestCase):
    """M4: one IPv6 host holds a /64; per-/128 buckets were free to multiply."""

    def test_keys(self):
        self.assertEqual(proxy.client_key("2001:db8:1:2:aaaa:bbbb:cccc:dddd"), "2001:db8:1:2::/64")
        self.assertEqual(proxy.client_key("::ffff:198.51.100.7"), "198.51.100.7")
        self.assertEqual(proxy.client_key("198.51.100.7"), "198.51.100.7")
        self.assertEqual(proxy.client_key("not an address"), "not an address")

    def test_addresses_of_one_64_share_the_send_bucket(self):
        t = ProxyTest("test_allowed_method_is_forwarded_and_reserialised")
        t.setUp()
        try:
            statuses = [t.call("sendTransaction", ["AAAA"], headers={"CF-Connecting-IP": "2001:db8:7::%x" % (i + 1)})[0] for i in range(8)]
            self.assertEqual(statuses[:4], [200] * 4)          # IP_SEND burst 4, shared by the /64
            self.assertIn(429, statuses[4:])
            self.assertEqual(t.call("sendTransaction", ["AAAA"], headers={"CF-Connecting-IP": "2001:db8:8::1"})[0], 200)
        finally:
            t.tearDown()


class ClientTableTest(unittest.TestCase):
    """M5: rotating addresses filled the client table and every new client got 503."""

    def limits(self, **over):
        cfg = dict(proxy.DEFAULTS)
        cfg.update(over)
        return proxy.Limits(cfg)

    def test_the_reviews_260_addresses_do_not_lock_out_a_new_client(self):
        lim = self.limits(MAX_CLIENTS=200)
        verdicts = [lim.allow("198.51.%d.%d" % (i // 250, i % 250)) for i in range(260)]
        self.assertEqual(verdicts, [None] * 260)               # was: 200 entries, then "busy" (503)
        self.assertLessEqual(len(lim.clients), 200)
        self.assertIsNone(lim.allow("203.0.113.77"))           # an honest newcomer
        self.assertEqual(lim.counters["evicted"], 61)

    def test_a_refused_new_client_takes_no_entry(self):
        lim = self.limits(MAX_CLIENTS=200, GLOBAL=(0.0, 10))
        verdicts = [lim.allow("198.51.100.%d" % i) for i in range(30)]
        self.assertEqual(verdicts.count(None), 10)
        self.assertEqual(len(lim.clients), 10)                 # the 20 refused ones left no entry

    def test_one_flooding_client_does_not_drain_the_global_bucket(self):
        lim = self.limits(GLOBAL=(0.0, 20), IP_ALL=(0.0, 5))
        verdicts = [lim.allow("198.51.100.1") for _ in range(100)]
        self.assertEqual(verdicts.count(None), 5)
        self.assertEqual(set(verdicts[5:]), {"client"})        # its own bucket refuses it, not the global one
        self.assertIsNone(lim.allow("198.51.100.2"))           # so the others are still served

    def test_over_http_with_a_small_table(self):
        t = ProxyTest("test_allowed_method_is_forwarded_and_reserialised")
        t.setUp(MAX_CLIENTS=200)
        try:
            statuses = [t.call("getLastBlockId", headers={"CF-Connecting-IP": "198.51.%d.%d" % (i // 250, i % 250)})[0]
                        for i in range(260)]
            self.assertNotIn(503, statuses)
            self.assertEqual(t.call("getLastBlockId", headers={"CF-Connecting-IP": "203.0.113.78"})[0], 200)
            self.assertLessEqual(len(t.srv.limits.clients), 200)
        finally:
            t.tearDown()

    def test_evicted_client_fails_closed_on_method_and_batch_checks(self):
        lim = self.limits(MAX_CLIENTS=2, GLOBAL=(0.0, 100))
        self.assertIsNone(lim.allow("198.51.100.1"))
        self.assertIsNone(lim.allow("198.51.100.2"))
        self.assertIsNone(lim.allow("198.51.100.3"))
        self.assertNotIn("198.51.100.1", lim.clients)
        # Evicted between the first check and the method/batch check: refuse instead of bypassing.
        for method in ("sendTransaction", "getBlockRange", "bridge_relayClaim", "bridge_settle", "bridge_faucet"):
            self.assertEqual(lim.allow("198.51.100.1", method), "client", method)
        self.assertEqual(lim.allow("198.51.100.1", "*extra", 5), "client")
        self.assertNotIn("198.51.100.1", lim.clients)
        # A fresh preflight still creates an entry and succeeds.
        self.assertIsNone(lim.allow("198.51.100.1", "*preflight"))
        self.assertIn("198.51.100.1", lim.clients)

    def test_evicted_client_gets_429_over_http(self):
        t = ProxyTest("test_allowed_method_is_forwarded_and_reserialised")
        t.setUp(MAX_CLIENTS=2)
        try:
            orig_allow = t.srv.limits.allow

            def evict_before_method_check(ip, method=None, extra=0):
                if ip == "198.51.100.1" and method is not None and method != "*preflight":
                    orig_allow("198.51.100.2")
                    orig_allow("198.51.100.3")
                return orig_allow(ip, method, extra)

            t.srv.limits.allow = evict_before_method_check
            status, out = t.call("sendTransaction", ["AAAA"], headers={"CF-Connecting-IP": "198.51.100.1"})
            self.assertEqual(status, 429)
            self.assertEqual(json.loads(out)["error"]["code"], -32005)
            self.assertEqual(FakeSequencer.seen, [])
        finally:
            t.tearDown()


class CorsTest(unittest.TestCase):
    """--cors-origin (2026-11-15): browser pages from the listed origin only; the relay unchanged."""
    ORIGIN = "https://nameswallet.app"

    def setUp(self, **over):
        cfg = dict(proxy.DEFAULTS)
        cfg.update(over)
        self.upstream = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeSequencer))
        self.relay = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeRelay))
        self.srv = serve(proxy.Server(("127.0.0.1", 0), proxy.Handler, cfg,
                                      ("127.0.0.1", self.upstream.server_address[1]), None, None, "CF-Connecting-IP",
                                      ("127.0.0.1", self.relay.server_address[1]), None, (self.ORIGIN,)))
        self.port = self.srv.server_address[1]
        FakeSequencer.seen, FakeRelay.seen, FakeRelay.auth = [], [], []

    def tearDown(self):
        for x in (self.srv, self.upstream, self.relay):
            x.shutdown()
            x.server_close()

    def ask(self, method, headers, body=None):
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        conn.request(method, headers.pop("_path", "/"), body, headers)
        r = conn.getresponse()
        out = r.read()
        conn.close()
        return r.status, {k.lower(): v for k, v in r.getheaders()}, out

    def preflight(self, ip="198.51.100.40", **extra):
        h = {"Origin": self.ORIGIN, "Access-Control-Request-Method": "POST", "Access-Control-Request-Headers": "content-type", "CF-Connecting-IP": ip}
        h.update(extra)
        return self.ask("OPTIONS", h)

    def rpc(self, method, origin=None, ip="198.51.100.41", params=None):
        h = {"Content-Type": "application/json", "CF-Connecting-IP": ip}
        if origin:
            h["Origin"] = origin
        return self.ask("POST", h, json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or []}).encode())

    def test_preflight_from_the_listed_origin(self):
        status, h, body = self.preflight()
        self.assertEqual(status, 204)
        self.assertEqual(body, b"")
        self.assertEqual(h["access-control-allow-origin"], self.ORIGIN)       # never "*"
        self.assertEqual(h["access-control-allow-methods"], "POST, OPTIONS")
        self.assertEqual(h["access-control-allow-headers"], "content-type")
        self.assertEqual(h["access-control-max-age"], "7200")
        self.assertIn("Origin", h["vary"])
        self.assertEqual(FakeSequencer.seen, [])                              # never reaches the sequencer

    def test_other_preflights_are_refused_as_before(self):
        cases = [
            {"Origin": "https://example.com"},
            {"Origin": "https://nameswallet.app.evil.com"},
            {"Origin": "null"},
            {"Access-Control-Request-Method": "PUT"},
            {"Access-Control-Request-Headers": "content-type, authorization"},
            {"_path": "/healthz"},
        ]
        for extra in cases:
            status, h, _ = self.preflight(**extra)
            self.assertEqual(status, 405, extra)
            if extra.get("Origin", self.ORIGIN) != self.ORIGIN:
                self.assertNotIn("access-control-allow-origin", h, extra)
        status, h, _ = self.ask("OPTIONS", {"CF-Connecting-IP": "198.51.100.42"})   # no Origin at all
        self.assertEqual(status, 405)
        self.assertNotIn("access-control-allow-origin", h)

    def test_post_answers_carry_the_origin_only_for_the_listed_one(self):
        status, h, _ = self.rpc("getLastBlockId", origin=self.ORIGIN)
        self.assertEqual(status, 200)
        self.assertEqual(h["access-control-allow-origin"], self.ORIGIN)
        self.assertEqual(h["vary"], "Origin")
        status, h, _ = self.rpc("getLastBlockId", origin="https://example.com")
        self.assertEqual(status, 200)
        self.assertNotIn("access-control-allow-origin", h)
        status, h, _ = self.rpc("getLastBlockId")                                # a non-browser client
        self.assertEqual(status, 200)
        self.assertNotIn("access-control-allow-origin", h)
        status, h, _ = self.rpc("requeueCrossZoneDeadLetter", origin=self.ORIGIN)  # refusals are readable too
        self.assertEqual(status, 403)
        self.assertEqual(h["access-control-allow-origin"], self.ORIGIN)

    def test_relay_methods_from_the_page_and_from_tools(self):
        for origin in (self.ORIGIN, None):
            status, h, out = self.rpc("bridge_relayInfo", origin=origin, ip="198.51.100.43")
            self.assertEqual(status, 200, origin)
            self.assertIn("payer", json.loads(out)["result"])
            self.assertEqual(h.get("access-control-allow-origin"), origin)
        self.assertEqual(len(FakeRelay.seen), 2)

    def test_a_preflight_flood_uses_only_its_own_bucket(self):
        self.tearDown()
        self.setUp(IP_PREFLIGHT=(0.0, 10), GLOBAL=(0.0, 3), IP_ALL=(0.0, 60))
        statuses = [self.preflight(ip="198.51.100.44")[0] for _ in range(30)]
        self.assertEqual(statuses[:10], [204] * 10)
        self.assertEqual(set(statuses[10:]), {429})
        # its POST budget and the shared global bucket are untouched: the global bucket held 3 and
        # the flooder's first request (a new client) took one
        self.assertEqual(self.rpc("getLastBlockId", ip="198.51.100.44")[0], 200)
        self.assertEqual(self.rpc("getLastBlockId", ip="198.51.100.45")[0], 200)

    def test_without_the_flag_nothing_changes(self):
        self.tearDown()
        self.upstream = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeSequencer))
        self.relay = serve(ThreadingHTTPServer(("127.0.0.1", 0), FakeRelay))
        self.srv = serve(proxy.Server(("127.0.0.1", 0), proxy.Handler, dict(proxy.DEFAULTS),
                                      ("127.0.0.1", self.upstream.server_address[1]), None))
        self.port = self.srv.server_address[1]
        self.assertEqual(self.preflight()[0], 405)
        status, h, _ = self.rpc("getLastBlockId", origin=self.ORIGIN)
        self.assertEqual(status, 200)
        self.assertNotIn("access-control-allow-origin", h)

    def test_the_flag_takes_exact_origins_only(self):
        for bad in ("*", "https://nameswallet.app/", "https://nameswallet.app/bridge", "nameswallet.app", "http://nameswallet.app"):
            with self.assertRaises(SystemExit, msg=bad):
                proxy.main(["--cors-origin", bad, "--listen", "127.0.0.1:1"])


class MainArgsTest(unittest.TestCase):
    def test_max_batch_flag(self):
        with self.assertRaises(SystemExit):
            proxy.main(["--max-batch", "-1"])


class ListenTest(unittest.TestCase):
    def test_refuses_non_loopback(self):
        with self.assertRaises(Exception):
            proxy.loopback_addr("0.0.0.0:3045")
        self.assertEqual(proxy.loopback_addr("127.0.0.1:3045"), ("127.0.0.1", 3045))


if __name__ == "__main__":
    unittest.main(verbosity=2)
