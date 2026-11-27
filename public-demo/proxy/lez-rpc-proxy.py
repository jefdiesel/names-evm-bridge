#!/usr/bin/python3
"""lez-rpc-proxy: a loopback-only JSON-RPC filter in front of a LEZ 0.4 sequencer.

Why it exists: the sequencer's RPC (jsonrpsee 0.26, `lez/sequencer/actors/rpc_server`) has no
caller auth, no CORS, no rate limit, accepts JSON-RPC batches, serves WebSocket on the same port,
takes 10 MiB bodies, and exposes `requeueCrossZoneDeadLetter` (a state-changing operator call,
"anyone who can reach the RPC can requeue"). A tunnel or reverse proxy forwards public traffic here,
never to the sequencer.

What it does, per request:
  - only `POST /` with `Content-Type: application/json` and a `Content-Length` (no chunked bodies,
    no Upgrade, no GET/OPTIONS except the loopback `GET /healthz`);
  - one JSON-RPC 2.0 object per request, or a batch of at most MAX_BATCH of them (--max-batch;
    0 refuses batches): every call checked as a single one would be (one bad call refuses the
    whole batch), no empty batch, no nested arrays, and a batch of N counts as N requests in the
    per-client bucket; method on the allowlist,
    `getBlockRange` span at most MAX_RANGE blocks (the stock wallet asks for 100);
  - body caps: SEND_MAX for `sendTransaction`, READ_MAX for everything else;
  - per-client token buckets keyed on the client-address header your front end sets
    (--client-ip-header, default `CF-Connecting-IP`, which Cloudflare sets; trust it only when
    nothing but that front end can reach this port); a client is an IPv4 address or an IPv6 /64
    (one host usually holds a whole /64, so per-/128 buckets were free to multiply); a tighter
    bucket for `sendTransaction`, a global bucket, a cap on concurrent upstream calls and on open
    connections; over a limit it answers at once (HTTP 429/503 + JSON-RPC error), never queues;
  - the client table holds at most MAX_CLIENTS entries: a new client is charged to the global
    bucket before it gets an entry, and a full table evicts the least recently seen client
    instead of refusing new ones (rotating addresses could otherwise fill it and lock everyone
    out with 503s);
  - forwards a re-serialised request (only jsonrpc/id/method/params) to the upstream sequencer
    with a timeout, and returns the upstream body as is (capped at RESP_MAX).

RFP-022 demo addition (--relay HOST:PORT; everything above is the testnet kit's filter as of
its commit 96a0a502, with this repository's default ports): two methods served by the proxy itself,
not by the sequencer, that reach the claim relay on loopback (public-demo/relayer/relayer.py):
  - `bridge_relayInfo` (no params) -> the relay's payer and caps;
  - `bridge_relayClaim` (params: [hex of a half-signed claim] or [hex, check_only]) -> the relay's
    verdict ({"ok": true, "tx_hash": ...} or {"ok": false, "reason": ..., "retry": ...});
  - `bridge_relayBurn` (the same params, a half-signed public burn: "Send back to Ethereum") -> the
    relay's verdict from its POST /relay/burn.
  Their own per-client bucket (IP_RELAY; `bridge_relayInfo`, a read, only the general one), body cap RELAY_MAX, at most RELAY_INFLIGHT at a time,
  never in a batch; the client address goes to the relay as CF-Connecting-IP for its daily caps,
  with the shared secret of --relay-auth-file (a mode-600 file, or LEZ_RPC_PROXY_RELAY_AUTH) in
  X-Relay-Auth, so the relay believes that header from this filter only and from no other
  loopback client. The secret is never logged. Without --relay both are refused like any unknown
  method.

CORS for browser pages (--cors-origin ORIGIN, repeatable; none by default, and then nothing here
changes): the change of logos-names `filter/cors.patch` (web-register-04), for the bridge page's
LNS name lookups. A preflight (`OPTIONS /` from a listed Origin asking for POST with at most the
Content-Type header, no body) gets 204 with Access-Control-Allow-Origin set to that origin,
Allow-Methods POST, OPTIONS, Allow-Headers content-type and Max-Age CORS_MAX_AGE (browsers cap
it: Chrome at 2 hours). Any other OPTIONS is refused as before (405). Every reply to a listed
Origin, errors and 429s included, carries Access-Control-Allow-Origin and Vary: Origin, so the page
can read why it was refused; other origins get no CORS header, so a browser keeps their pages from
reading anything. Never `*`. Preflights have their own per-client bucket (IP_PREFLIGHT) and do not
touch the shared global bucket or the client's POST budget (only a new client's first request
is charged to the global bucket, as for any request): a preflight flood can only use up its own
sender's preflight tokens, and it costs no body read, no parse and no upstream call.

No third-party modules: any python3 >= 3.7 runs it (the macOS system /usr/bin/python3 included).

  lez-rpc-proxy.py [--listen 127.0.0.1:3295] [--upstream 127.0.0.1:3290] [--log-requests FILE]
                   [--client-ip-header NAME|none] [--allow-method NAME ...] [--set LIMIT=VALUE ...]
                   [--relay 127.0.0.1:3297 [--relay-auth-file FILE]] [--cors-origin https://example.org ...]

It refuses to listen on anything but a loopback address.
"""

import argparse
import collections
import http.client
import ipaddress
import json
import logging
import logging.handlers
import os
import socket
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# Methods a wallet, an app's CLI, a status snapshot or a block scanner uses: every method of
# lez/sequencer/service/rpc/src/lib.rs (v0.4.0-rc1 to rc3) except the two operator calls below.
# Add one with --allow-method NAME.
ALLOWED = {
    "sendTransaction",
    "getFeeState",
    "checkHealth",
    "getBlock",
    "getBlockRange",
    "getLastBlockId",
    "getAccountBalance",
    "getTransaction",
    "getAccountsNonces",
    "getAccountView",
    "getPublications",
    "getMessagePath",
    "getRecoveryBinding",
    "getProofsAndRoot",
    "getAccount",
    "getProgramIds",
    "getChannelId",
}
# Named here so the refusal is deliberate and testable: operator-only (no auth upstream).
DENIED = {"getCrossZoneDeadLetters", "requeueCrossZoneDeadLetter"}
# Served by the proxy itself from the claim relay (--relay), never sent to the sequencer.
RELAY_METHODS = {"bridge_relayInfo", "bridge_relayClaim", "bridge_relayBurn"}
# Served by the test-LEZ faucet (--faucet, public-demo/faucet/faucet.py), never sent to the sequencer.
FAUCET_METHODS = {"bridge_faucetInfo", "bridge_faucet"}
# Served by the settlement service (--settle, public-demo/settle/settle.py), never sent to the sequencer.
SETTLE_METHODS = {"bridge_settleInfo", "bridge_settle", "bridge_settleStatus", "bridge_settleBundle"}
LOOPBACK_METHODS = RELAY_METHODS | FAUCET_METHODS | SETTLE_METHODS

DEFAULTS = dict(
    SEND_MAX=768 * 1024,  # a ~256 KB private transaction is ~342 KB as base64 JSON
    READ_MAX=16 * 1024,
    RESP_MAX=12 * 1024 * 1024,  # the upstream caps responses at 10 MiB itself
    MAX_RANGE=100,
    MAX_BATCH=20,  # calls per JSON-RPC batch; 0 refuses batches
    MAX_CONN=96,  # open client connections (threads)
    MAX_INFLIGHT=12,  # concurrent upstream calls
    UPSTREAM_TIMEOUT=30.0,
    IDLE_TIMEOUT=75.0,  # keep-alive idle; longer than cloudflared's keepAliveTimeout (30 s)
    # token buckets: (rate per second, burst)
    IP_ALL=(5.0, 60),  # matches the edge rule (50 per 10 s); the dry run used 10/s, burst 150
    IP_SEND=(0.2, 4),
    IP_RANGE=(4.0, 60),
    GLOBAL=(60.0, 300),
    MAX_CLIENTS=20000,  # per-IP table size before it is pruned
    # the claim relay (--relay)
    IP_RELAY=(0.05, 5),  # 5 at once, then one per 20 s per client
    IP_SETTLE=(0.05, 3),  # bridge_settle: 3 at once, then one per 20 s per client (the service caps jobs itself)
    SETTLE_MAX=720 * 1024,  # a release receipt (~225 KB) as hex in JSON
    IP_FAUCET=(0.05, 3),  # bridge_faucet: 3 at once, then one per 20 s per client (the faucet caps grants itself)
    RELAY_MAX=20000,  # body cap of a relay call (the relay's own cap)
    RELAY_INFLIGHT=2,
    RELAY_TIMEOUT=120.0,  # a relayed claim is checked, then checked again, signed and sent
    # CORS (--cors-origin)
    CORS_MAX_AGE=7200,  # seconds a browser may cache a preflight (Chrome caps it at 7200)
    IP_PREFLIGHT=(0.2, 10),  # preflights per client: a browser needs one per Max-Age per page
)


class Bucket:
    __slots__ = ("rate", "burst", "tokens", "stamp")

    def __init__(self, rate, burst):
        self.rate, self.burst, self.tokens, self.stamp = rate, burst, float(burst), time.monotonic()

    def take(self, n=1.0):
        now = time.monotonic()
        self.tokens = min(self.burst, self.tokens + (now - self.stamp) * self.rate)
        self.stamp = now
        if self.tokens >= n:
            self.tokens -= n
            return True
        return False


class Limits:
    def __init__(self, cfg):
        self.cfg = cfg
        self.lock = threading.Lock()
        self.clients = collections.OrderedDict()  # least recently seen first
        self.global_bucket = Bucket(*cfg["GLOBAL"])
        self.inflight = threading.BoundedSemaphore(cfg["MAX_INFLIGHT"])
        self.relay_inflight = threading.BoundedSemaphore(int(cfg["RELAY_INFLIGHT"]))
        self.faucet_inflight = threading.BoundedSemaphore(2)
        self.settle_inflight = threading.BoundedSemaphore(4)
        self.counters = {}

    def new_entry(self):
        cfg = self.cfg
        return {
            "all": Bucket(*cfg["IP_ALL"]),
            "send": Bucket(*cfg["IP_SEND"]),
            "range": Bucket(*cfg["IP_RANGE"]),
            "relay": Bucket(*cfg["IP_RELAY"]),
            "faucet": Bucket(*cfg["IP_FAUCET"]),
            "settle": Bucket(*cfg["IP_SETTLE"]),
            "preflight": Bucket(*cfg["IP_PREFLIGHT"]),
        }

    def count(self, key):
        with self.lock:
            self.counters[key] = self.counters.get(key, 0) + 1

    def allow(self, ip, method=None, extra=0):
        """method None: the per-client and global buckets, checked before the body is read.
        A method name: only that method's own bucket, checked after the parse.
        "*preflight": a CORS preflight, its own per-client bucket only (a new client is still
        charged to the global bucket once, like any first request)."""
        cfg = self.cfg
        with self.lock:
            entry = self.clients.get(ip)
            if entry is not None:
                self.clients.move_to_end(ip)
            elif method is not None and method != "*preflight":
                # Evicted since this request's first check: fail closed rather than bypassing
                # the per-method or batch bucket.
                return "client"
            else:
                # A new client: charged to the global bucket first, so requests the global bucket
                # refuses never create entries; a full table evicts the least recently seen client
                # (it comes back with fresh buckets) rather than refusing everyone new.
                if not self.global_bucket.take():
                    return "busy"
                while len(self.clients) >= cfg["MAX_CLIENTS"]:
                    self.clients.popitem(last=False)
                    self.counters["evicted"] = self.counters.get("evicted", 0) + 1
                entry = self.new_entry()
                self.clients[ip] = entry
                if method == "*preflight":
                    return None if entry["preflight"].take() else "client"
                if not entry["all"].take():
                    return "client"
                return None
            if method == "*preflight":
                # CORS preflights: their own bucket only (never the global one or the POST budget)
                return None if entry["preflight"].take() else "client"
            if method is None:
                if not entry["all"].take():
                    return "client"
                if not self.global_bucket.take():
                    return "busy"
            elif method == "*extra":
                # The rest of a batch: n - 1 more requests from the client's own bucket.
                if not entry["all"].take(extra):
                    return "client"
            elif method == "sendTransaction" and not entry["send"].take():
                return "client"
            elif method == "getBlockRange" and not entry["range"].take():
                return "client"
            elif method == "bridge_settle" and not entry["settle"].take():
                return "client"
            elif method == "bridge_faucet" and not entry["faucet"].take():
                return "client"
            elif method in RELAY_METHODS and method != "bridge_relayInfo" and not entry["relay"].take():
                # bridge_relayInfo (a cached read of the relay's payer and caps) costs only the
                # client's general budget: wallets read it before every claim and burn
                return "client"
        return None


def client_key(ip):
    """The rate-limit key of an address: IPv4 as is (also when IPv4-mapped), IPv6 its /64."""
    try:
        a = ipaddress.ip_address(ip)
    except ValueError:
        return ip
    if a.version == 6 and a.ipv4_mapped:
        a = a.ipv4_mapped
    if a.version == 6:
        return str(ipaddress.ip_network("%s/64" % a, strict=False))
    return str(a)


def rpc_error(code, message, rid=None):
    return json.dumps({"jsonrpc": "2.0", "id": rid, "error": {"code": code, "message": message}}).encode()


class Server(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True
    request_queue_size = 64

    def __init__(self, addr, handler, cfg, upstream, log, allowed=None, client_ip_header="CF-Connecting-IP", relay=None, relay_auth=None, cors_origins=(), faucet=None, settle=None):
        super().__init__(addr, handler)
        self.cfg, self.upstream, self.log = cfg, upstream, log
        self.cors_origins = frozenset(cors_origins)  # exact origins only, never "*"
        self.relay = relay
        self.relay_auth = relay_auth  # shared secret for the relay (X-Relay-Auth); never logged
        self.faucet = faucet
        self.settle = settle
        self.allowed = (set(ALLOWED if allowed is None else allowed) | (RELAY_METHODS if relay else set()) | (FAUCET_METHODS if faucet else set())
                        | (SETTLE_METHODS if settle else set()))
        self.client_ip_header = client_ip_header
        self.limits = Limits(cfg)
        self.open_conns = 0
        self.conn_lock = threading.Lock()
        self.health = (0.0, None)

    def process_request(self, request, client_address):
        # Shed before spawning a thread when too many connections are open.
        with self.conn_lock:
            if self.open_conns >= self.cfg["MAX_CONN"]:
                self.limits.count("shed_conn")
                try:
                    request.sendall(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                except OSError:
                    pass
                self.shutdown_request(request)
                return
            self.open_conns += 1
        super().process_request(request, client_address)

    def handle_error(self, request, client_address):
        # Count, never print: a traceback per bad request would fill the disk.
        self.limits.count("handler_error")

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            with self.conn_lock:
                self.open_conns -= 1


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "lez-rpc-proxy"
    sys_version = ""

    def setup(self):
        self.timeout = self.server.cfg["IDLE_TIMEOUT"]
        super().setup()

    def log_message(self, fmt, *args):  # silence the default stderr access log
        pass

    def client_ip(self):
        name = self.server.client_ip_header
        ip = self.headers.get(name, "").strip() if name else ""
        if ip:
            try:
                return str(ipaddress.ip_address(ip))
            except ValueError:
                pass
        return self.client_address[0]

    def client_key(self):
        return client_key(self.client_ip())

    def cors_origin(self):
        """The request's Origin when it is on the --cors-origin list, else None."""
        origin = self.headers.get("Origin")
        return origin if origin in self.server.cors_origins else None

    def reply(self, status, body, method="-", started=None, extra=None):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        origin = self.cors_origin()
        if origin is not None:
            self.send_header("Access-Control-Allow-Origin", origin)
            self.send_header("Vary", "Origin")
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        if status >= 400 and status not in (429,):
            self.send_header("Connection", "close")
            self.close_connection = True
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)
        self.server.limits.count("%s %d" % (method, status))
        if self.server.log is not None:
            ms = (time.monotonic() - started) * 1000 if started else 0
            self.server.log.info("%s %s %d %d %.0f", self.client_ip(), method, status, len(body), ms)

    def refuse_other(self):
        self.reply(405, rpc_error(-32600, "only POST / with a JSON-RPC body"))

    do_PUT = do_DELETE = do_PATCH = do_HEAD = refuse_other

    def do_OPTIONS(self):
        # A CORS preflight from a listed origin, for exactly what a page's fetch() POST needs.
        origin = self.cors_origin()
        wanted = {h.strip().lower() for h in self.headers.get("Access-Control-Request-Headers", "").split(",") if h.strip()}
        if (origin is None or self.path != "/" or self.headers.get("Access-Control-Request-Method") != "POST"
                or not wanted <= {"content-type"} or self.headers.get("Content-Length", "0") not in ("", "0")):
            return self.refuse_other()
        verdict = self.server.limits.allow(self.client_key(), "*preflight")
        if verdict is not None:
            return self.reply(429 if verdict == "client" else 503, rpc_error(-32005, "rate limited; slow down"), "preflight", extra={"Retry-After": "10"})
        self.send_response(204)
        self.send_header("Access-Control-Allow-Origin", origin)
        self.send_header("Access-Control-Allow-Methods", "POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "content-type")
        self.send_header("Access-Control-Max-Age", str(int(self.server.cfg["CORS_MAX_AGE"])))
        self.send_header("Vary", "Origin, Access-Control-Request-Method, Access-Control-Request-Headers")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", "0")
        self.end_headers()
        self.server.limits.count("preflight 204")
        if self.server.log is not None:
            self.server.log.info("%s preflight 204 0 0", self.client_ip())

    def do_GET(self):
        # Loopback health for the local monitor; the public ingress must never route this path,
        # and a request carrying the client-address header (so: from the front end) is refused.
        name = self.server.client_ip_header
        if self.path == "/healthz" and (not name or self.headers.get(name) is None):
            stamp, cached = self.server.health
            if time.monotonic() - stamp > 5 or cached is None:
                ok, height = False, None
                try:
                    height = json.loads(self.upstream_call(b'{"jsonrpc":"2.0","id":1,"method":"getLastBlockId","params":[]}')[1])["result"]
                    ok = True
                except Exception:  # noqa: BLE001
                    pass
                cached = json.dumps({"ok": ok, "height": height, "counters": self.server.limits.counters}).encode()
                self.server.health = (time.monotonic(), cached)
            return self.reply(200, cached, "healthz")
        return self.refuse_other()

    def upstream_call(self, body):
        host, port = self.server.upstream
        conn = http.client.HTTPConnection(host, port, timeout=self.server.cfg["UPSTREAM_TIMEOUT"])
        try:
            conn.request("POST", "/", body=body, headers={"Content-Type": "application/json"})
            resp = conn.getresponse()
            data = resp.read(self.server.cfg["RESP_MAX"] + 1)
            if len(data) > self.server.cfg["RESP_MAX"]:
                raise ValueError("upstream response too large")
            return resp.status, data
        finally:
            conn.close()

    def do_POST(self):
        started = time.monotonic()
        cfg = self.server.cfg
        if self.path != "/":
            return self.reply(404, rpc_error(-32600, "not found"), started=started)
        if self.headers.get("Upgrade") or self.headers.get("Transfer-Encoding"):
            return self.reply(400, rpc_error(-32600, "no upgrades or chunked bodies"), started=started)
        if not self.headers.get("Content-Type", "").lower().startswith("application/json"):
            return self.reply(415, rpc_error(-32600, "Content-Type must be application/json"), started=started)
        try:
            length = int(self.headers.get("Content-Length", ""))
        except ValueError:
            return self.reply(411, rpc_error(-32600, "Content-Length required"), started=started)
        if length < 0 or length > max(cfg["SEND_MAX"], cfg["READ_MAX"], cfg["RELAY_MAX"]):
            return self.reply(413, rpc_error(-32600, "request too large"), started=started)
        # Per-client and global limits before the body is read, so a refused flood costs no read
        # or parse.
        verdict = self.server.limits.allow(self.client_key())
        if verdict is not None:
            self.close_connection = True  # the unread body must not be parsed as the next request
            if verdict == "client":
                return self.reply(429, rpc_error(-32005, "rate limited; slow down"), started=started, extra={"Retry-After": "10", "Connection": "close"})
            return self.reply(503, rpc_error(-32005, "server busy; retry later"), started=started, extra={"Retry-After": "5"})
        raw = self.rfile.read(length)
        try:
            req = json.loads(raw)
        except Exception:  # noqa: BLE001  ValueError, RecursionError (deep nesting), MemoryError
            return self.reply(400, rpc_error(-32700, "parse error"), started=started)
        if isinstance(req, list):
            return self.batch(req, length, started)
        if not isinstance(req, dict):
            return self.reply(400, rpc_error(-32600, "send one JSON-RPC request object or a batch"), started=started)
        refused = self.check_call(req, length)
        if refused is not None:
            return self.reply(*refused, started=started)
        rid, method, params = req.get("id"), req["method"], req.get("params", [])
        if self.server.limits.allow(self.client_key(), method) == "client":
            return self.reply(429, rpc_error(-32005, "rate limited; slow down", rid), method, started, {"Retry-After": "10"})
        if method in LOOPBACK_METHODS:
            return self.relay_call(rid, method, params, started)
        body = json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}, separators=(",", ":")).encode()
        return self.forward(body, rid, method, started)

    def check_call(self, req, length):
        """None if this one call may go upstream, else (status, body, label) for the refusal."""
        cfg = self.server.cfg
        if not isinstance(req, dict):
            return 400, rpc_error(-32600, "a batch holds request objects only (no nested arrays)"), "-"
        rid, method, params = req.get("id"), req.get("method"), req.get("params", [])
        if not isinstance(rid, (int, str)) or isinstance(rid, bool) or (isinstance(rid, str) and len(rid) > 64):
            return 400, rpc_error(-32600, "id must be a number or a short string"), "-"
        if not isinstance(method, str) or method not in self.server.allowed:
            label = method if isinstance(method, str) and method in DENIED else "other"
            return 403, rpc_error(-32601, "method not served here", rid), label
        cap = (cfg["SEND_MAX"] if method == "sendTransaction" else cfg["SETTLE_MAX"] if method == "bridge_settle"
               else cfg["RELAY_MAX"] if method in LOOPBACK_METHODS else cfg["READ_MAX"])
        if length > cap:
            return 413, rpc_error(-32600, "request too large for " + method, rid), method
        if not isinstance(params, (list, dict)):
            return 400, rpc_error(-32602, "params must be a list or object", rid), method
        if method == "getBlockRange":
            try:
                a, b = (params[0], params[1]) if isinstance(params, list) else (params["start_block_id"], params["end_block_id"])
                if not (isinstance(a, int) and isinstance(b, int) and 0 <= a <= b and b - a + 1 <= cfg["MAX_RANGE"]):
                    raise ValueError
            except (ValueError, KeyError, IndexError, TypeError):
                return 400, rpc_error(-32602, "getBlockRange spans at most %d blocks" % cfg["MAX_RANGE"], rid), method
        return None

    def batch(self, reqs, length, started):
        cfg = self.server.cfg
        if cfg["MAX_BATCH"] <= 0:
            return self.reply(400, rpc_error(-32600, "batches are not served; send one request"), "batch", started)
        if not reqs:
            return self.reply(400, rpc_error(-32600, "empty batch"), "batch", started)
        if len(reqs) > cfg["MAX_BATCH"]:
            return self.reply(400, rpc_error(-32600, "a batch holds at most %d calls" % cfg["MAX_BATCH"]), "batch", started)
        # The whole batch is one body: it must fit the cap of its largest kind of call.
        for req in reqs:
            if isinstance(req, dict) and req.get("method") in LOOPBACK_METHODS:
                return self.reply(400, rpc_error(-32600, "relay methods are not served in a batch"), "batch relay", started)
            refused = self.check_call(req, length)
            if refused is not None:
                status, body, label = refused
                return self.reply(status, body, "batch " + label, started)
        ip = self.client_key()
        # One request was counted before the body was read; a batch of N counts N.
        if len(reqs) > 1 and self.server.limits.allow(ip, "*extra", len(reqs) - 1) == "client":
            return self.reply(429, rpc_error(-32005, "rate limited; slow down"), "batch", started, {"Retry-After": "10"})
        for req in reqs:
            if self.server.limits.allow(ip, req["method"]) == "client":
                return self.reply(429, rpc_error(-32005, "rate limited; slow down"), "batch", started, {"Retry-After": "10"})
        body = json.dumps([{"jsonrpc": "2.0", "id": r.get("id"), "method": r["method"], "params": r.get("params", [])} for r in reqs],
                          separators=(",", ":")).encode()
        return self.forward(body, None, "batch", started)

    def forward(self, body, rid, label, started):
        if not self.server.limits.inflight.acquire(blocking=False):
            return self.reply(503, rpc_error(-32005, "server busy; retry later", rid), label, started, {"Retry-After": "5"})
        try:
            status, data = self.upstream_call(body)
        except (OSError, ValueError, http.client.HTTPException):
            return self.reply(502, rpc_error(-32603, "sequencer unavailable", rid), label, started)
        finally:
            self.server.limits.inflight.release()
        return self.reply(200 if status == 200 else 502, data, label, started)


    def relay_call(self, rid, method, params, started):
        """A relay or faucet method: checked here, answered by the claim relay or the faucet on loopback."""
        target = self.server.faucet if method in FAUCET_METHODS else self.server.settle if method in SETTLE_METHODS else self.server.relay
        one = params[0] if isinstance(params, list) and len(params) == 1 else params if isinstance(params, dict) else None
        if method == "bridge_settleInfo":
            path, payload = "/settle/info", None
        elif method == "bridge_settle":
            ok = isinstance(one, dict) and isinstance(one.get("vault"), str) and len(one["vault"]) <= 42 and (
                (isinstance(one.get("receipt_hex"), str) and isinstance(one.get("journal_hex"), str) and "exit" not in one)
                or (isinstance(one.get("exit"), dict) and "receipt_hex" not in one))
            if not ok:
                return self.reply(400, rpc_error(-32602, "params: [{vault, receipt_hex, journal_hex}] or [{vault, exit}]", rid), method, started)
            path, payload = "/settle", json.dumps(one).encode()
        elif method in ("bridge_settleStatus", "bridge_settleBundle"):
            key = "id" if method == "bridge_settleStatus" else "fetch_id"
            v = one if isinstance(one, str) else one.get(key) if isinstance(one, dict) else None
            if not (isinstance(v, str) and 2 <= len(v) <= 66 and all(c in "0123456789abcdefABCDEFx" for c in v)):
                return self.reply(400, rpc_error(-32602, "params: [%s]" % key, rid), method, started)
            path = "/settle/status" if method == "bridge_settleStatus" else "/settle/bundle"
            payload = json.dumps({key: v}).encode()
        elif method == "bridge_faucetInfo":
            path, payload = "/faucet/info", None
        elif method == "bridge_faucet":
            pk = params[0] if isinstance(params, list) and params else params.get("public_key") if isinstance(params, dict) else None
            acct = (params[1] if isinstance(params, list) and len(params) > 1 else
                    params.get("account") if isinstance(params, dict) else None)
            n = len(params) if isinstance(params, (list, dict)) else 0
            if not (isinstance(pk, str) and len(pk) <= 66 and all(c in "0123456789abcdefABCDEFx" for c in pk)
                    and (acct is None or (isinstance(acct, str) and len(acct) <= 60)) and n <= 2):
                return self.reply(400, rpc_error(-32602, "params: [public key hex] or [public key hex, account]", rid), method, started)
            path = "/faucet"
            payload = json.dumps({"public_key": pk} if acct is None else {"public_key": pk, "account": acct}).encode()
        elif method == "bridge_relayInfo":
            path, payload = "/relay/info", None
        else:
            tx = params[0] if isinstance(params, list) and params else params.get("tx") if isinstance(params, dict) else None
            check = (params[1] if isinstance(params, list) and len(params) > 1 else
                     params.get("check_only", False) if isinstance(params, dict) else False)
            n = len(params) if isinstance(params, (list, dict)) else 0
            if not (isinstance(tx, str) and 2 <= len(tx) <= 2 * 8192 + 2 and all(c in "0123456789abcdefABCDEFx" for c in tx)
                    and isinstance(check, bool) and n <= 2):
                return self.reply(400, rpc_error(-32602, "params: [hex of a half-signed claim or burn] or [hex, check_only]", rid), method, started)
            path = "/relay/burn" if method == "bridge_relayBurn" else "/relay"
            payload = json.dumps({"tx": tx, "check_only": check}).encode()
        sem = (self.server.limits.faucet_inflight if method in FAUCET_METHODS else self.server.limits.settle_inflight if method in SETTLE_METHODS
               else self.server.limits.relay_inflight)
        what = "faucet" if method in FAUCET_METHODS else "settlement" if method in SETTLE_METHODS else "relay"
        cap = 1024 * 1024 if method == "bridge_settleBundle" else 64 * 1024
        if not sem.acquire(blocking=False):
            return self.reply(503, rpc_error(-32005, what + " busy; retry later", rid), method, started, {"Retry-After": "10"})
        try:
            host, port = target
            conn = http.client.HTTPConnection(host, port, timeout=self.server.cfg["RELAY_TIMEOUT"])
            try:
                headers = {"Content-Type": "application/json", "CF-Connecting-IP": self.client_ip()}
                if self.server.relay_auth:
                    headers["X-Relay-Auth"] = self.server.relay_auth
                conn.request("GET" if payload is None else "POST", path, body=payload, headers=headers)
                resp = conn.getresponse()
                data = resp.read(cap + 1)
            finally:
                conn.close()
            result = json.loads(data) if len(data) <= cap else None
            if not isinstance(result, dict):
                raise ValueError("bad relay answer")
        except (OSError, ValueError, http.client.HTTPException):
            return self.reply(502, rpc_error(-32603, what + " unavailable", rid), method, started)
        finally:
            sem.release()
        return self.reply(200, json.dumps({"jsonrpc": "2.0", "id": rid, "result": result}).encode(), method, started)


def loopback_addr(text):
    host, _, port = text.rpartition(":")
    host = host.strip("[]")
    if not ipaddress.ip_address(host).is_loopback:
        raise argparse.ArgumentTypeError(text + " is not a loopback address")
    return host, int(port)


def relay_secret(path):
    """The relay's shared secret: from a file only this user can read (mode 600), else the
    environment (LEZ_RPC_PROXY_RELAY_AUTH); None without either."""
    if path:
        if os.stat(path).st_mode & 0o077:
            raise SystemExit("--relay-auth-file must be mode 600")
        with open(path) as f:
            value = f.read().strip()
    else:
        value = os.environ.get("LEZ_RPC_PROXY_RELAY_AUTH", "").strip()
    if value and len(value) < 32:
        raise SystemExit("the relay's shared secret must be at least 32 characters")
    return value or None


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--listen", type=loopback_addr, default=("127.0.0.1", 3295))
    p.add_argument("--upstream", type=loopback_addr, default=("127.0.0.1", 3290))
    p.add_argument("--relay", type=loopback_addr, default=None,
                   help="the claim relay (public-demo/relayer/relayer.py): serve bridge_relayInfo and bridge_relayClaim from it")
    p.add_argument("--settle", type=loopback_addr, default=None,
                   help="the settlement service (public-demo/settle/settle.py): serve bridge_settleInfo, bridge_settle, bridge_settleStatus, bridge_settleBundle")
    p.add_argument("--faucet", type=loopback_addr, default=None,
                   help="the test-LEZ faucet (public-demo/faucet/faucet.py): serve bridge_faucetInfo and bridge_faucet from it")
    p.add_argument("--relay-auth-file", default=None, metavar="FILE",
                   help="mode-600 file with the secret shared with the relay (sent as X-Relay-Auth), so the relay trusts "
                        "the client address this filter passes on; default: $LEZ_RPC_PROXY_RELAY_AUTH")
    p.add_argument("--client-ip-header", default="CF-Connecting-IP",
                   help="header holding the real client address, set by your tunnel or reverse proxy "
                        "(X-Real-IP for nginx/Caddy setups); 'none' keys limits on the TCP peer")
    p.add_argument("--allow-method", action="append", default=[], metavar="NAME",
                   help="serve this JSON-RPC method too (repeatable)")
    p.add_argument("--max-batch", type=int, default=None, metavar="N",
                   help="calls allowed in one JSON-RPC batch (default %d; 0 refuses batches)" % DEFAULTS["MAX_BATCH"])
    p.add_argument("--log-requests", help="append one line per request (ip method status bytes ms) to this file")
    p.add_argument("--cors-origin", action="append", default=[], metavar="ORIGIN",
                   help="let browser pages from this exact origin (scheme://host[:port], no path) call "
                        "the filter (repeatable; none by default)")
    p.add_argument("--set", action="append", default=[], metavar="NAME=VALUE",
                   help="override a limit, e.g. --set IP_ALL=10,150 --set SEND_MAX=786432")
    args = p.parse_args(argv)
    cfg = dict(DEFAULTS)
    for item in args.set:
        name, _, value = item.partition("=")
        if name not in cfg:
            p.error("unknown limit " + name)
        cfg[name] = tuple(float(x) for x in value.split(",")) if isinstance(cfg[name], tuple) else type(cfg[name])(value)
    if args.max_batch is not None:
        if args.max_batch < 0:
            p.error("--max-batch must be 0 or more")
        cfg["MAX_BATCH"] = args.max_batch
    log = None
    if args.log_requests:
        log = logging.getLogger("requests")
        h = logging.handlers.RotatingFileHandler(args.log_requests, maxBytes=20 * 1024 * 1024, backupCount=3)
        h.setFormatter(logging.Formatter("%(asctime)s %(message)s"))
        log.addHandler(h)
        log.setLevel(logging.INFO)
    socket.setdefaulttimeout(None)
    allowed = set(ALLOWED) | set(args.allow_method)
    for name in sorted(set(args.allow_method) & DENIED):
        print("WARNING: serving operator-only method %s to the public" % name, file=sys.stderr, flush=True)
    header = None if args.client_ip_header.lower() == "none" else args.client_ip_header
    for origin in args.cors_origin:
        if origin != origin.rstrip("/") or origin.count("/") != 2 or not origin.startswith(("https://", "http://127.0.0.1", "http://localhost")):
            p.error("--cors-origin takes an exact origin such as https://nameswallet.app, got " + origin)
    srv = Server(args.listen, Handler, cfg, args.upstream, log, allowed, header, args.relay,
                 relay_secret(args.relay_auth_file) if (args.relay or args.faucet or args.settle) else None, args.cors_origin, args.faucet, args.settle)
    print("lez-rpc-proxy on %s:%d -> %s:%d" % (args.listen + args.upstream), flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
