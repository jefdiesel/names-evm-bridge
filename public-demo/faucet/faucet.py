#!/usr/bin/python3
"""Test-LEZ faucet for the bridge test network (LEZ 0.4): the Names Wallet's "Get test LEZ" button.

TESTNET: the coins have no value. A public account gets FAUCET_AMOUNT native LEZ (default
10,000,000) from the faucet's own account, sent with the CLI's `wallet auth-transfer send`. The
wallet calls it through the public endpoint's RPC filter (public-demo/proxy/lez-rpc-proxy.py
--faucet), which forwards the client's address with the shared secret, as for the claim relay.

  GET  /faucet/info  {"amount", "per_account_hours", "available", "reason"}
  POST /faucet       {"public_key": "<64 hex: the account's public key>", "account": "<optional: its id>"}
                     -> {"ok": true, "tx", "block", "amount", "account"}
                      | {"ok": false, "code": already_today|faucet_empty|busy|rate_limited|bad_key, "reason", "retry"}

Why the public key and not only the id: on rc3 a transfer to a public account that was never
used must name its key (`--to-pk`); `--to <id>` of such an account is included and reverted, and
only the fee is charged (RUNBOOK §10). The id is SHA-256("/LEE/v0.3/AccountId/Public/" | 5 zero
bytes | key), so the key also fixes the account; an `account` given beside it must match.

Limits, over a ROLLING 24 h: one grant per account (PER_ACCOUNT_HOURS), PER_CLIENT_DAY per client
(IPv4 address or IPv6 /64, from CF-Connecting-IP only with the filter's shared secret), DAILY_MAX
overall; the faucet keeps MIN_BALANCE after a grant; one transfer at a time. The intent is written
(fsynced) before the CLI sends; a send whose answer was lost is `uncertain`, still counted, and
the operator looks at it (never sent twice automatically). One log line per grant. No key, no
address and no client address is printed or logged (clients are HMAC tags under a daily key).

Loopback only. Stdlib only. DRY_RUN=1: checks everything, sends nothing.
"""
import hashlib, hmac, http.server, ipaddress, json, os, re, secrets, socketserver, subprocess, sys, threading, time
import urllib.request

E = os.environ.get
P = E("P", "./public-chain")
CLI = E("CLI", P + "/bin/lez-bridge")
WALLET = E("FAUCET_WALLET", P + "/wallets/faucet-v4")
FROM = E("FAUCET_ACCOUNT", "")                         # the faucet's own public account id
SEQUENCER = E("SEQUENCER", "http://127.0.0.1:3290")
LISTEN = E("FAUCET_LISTEN", "127.0.0.1:3298")
STATE = E("FAUCET_STATE", P + "/faucet/state.json")
LOG = E("FAUCET_LOG", P + "/logs/faucet.log")
AUTH_FILE = E("FAUCET_AUTH_FILE", "")                  # the filter's shared secret (mode 600)
AMOUNT = int(E("FAUCET_AMOUNT", "10000000"))
PER_ACCOUNT_HOURS = int(E("PER_ACCOUNT_HOURS", "24"))
PER_CLIENT_DAY = int(E("PER_CLIENT_DAY", "5"))
DAILY_MAX = int(E("DAILY_MAX", "200"))
MIN_BALANCE = int(E("MIN_BALANCE", "100000000"))       # kept after a grant (fees of later sends)
CLI_TIMEOUT = int(E("CLI_TIMEOUT", "180"))
DRY = E("DRY_RUN", "0") == "1"
WINDOW = 86400
MAX_BODY = 2000
AUTH_HEADER = "X-Relay-Auth"
AUTH = None
LOCK = threading.Lock()

NATIVE_PROGRAM = "11111111111111111111111111111111"
PUBLIC_PREFIX = b"/LEE/v0.3/AccountId/Public/" + b"\0" * 5
SECP_P = 2**256 - 2**32 - 977
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


# ---------------------------------------------------------------- pure helpers

def b58encode(b):
    n = int.from_bytes(b, "big")
    s = ""
    while n:
        n, r = divmod(n, 58)
        s = B58[r] + s
    return "1" * (len(b) - len(b.lstrip(b"\0"))) + s


def valid_public_key(pk):
    """A LEZ public key is a BIP-340 x-only secp256k1 key: x < p and x^3 + 7 a square mod p."""
    if len(pk) != 32 or pk == b"\0" * 32:
        return False
    x = int.from_bytes(pk, "big")
    if x >= SECP_P:
        return False
    return pow((pow(x, 3, SECP_P) + 7) % SECP_P, (SECP_P - 1) // 2, SECP_P) == 1


def account_id(pk):
    return b58encode(hashlib.sha256(PUBLIC_PREFIX + pk).digest())


def native_of(account_json):
    if account_json is None:
        return 0
    raw = ((account_json.get("data") or {}).get("actor_states") or {}).get(NATIVE_PROGRAM) or []
    if isinstance(raw, str):
        import base64
        raw = list(base64.b64decode(raw))
    if len(raw) not in (0, 16):
        raise ValueError("native balance is not 16 bytes")
    return int.from_bytes(bytes(raw), "little")


def client_key(ip):
    try:
        a = ipaddress.ip_address(ip)
    except ValueError:
        return str(ip)
    if a.version == 6 and a.ipv4_mapped:
        a = a.ipv4_mapped
    return str(ipaddress.ip_network("%s/64" % a, strict=False)) if a.version == 6 else str(a)


def refuse(code, reason, retry=False):
    return {"ok": False, "code": code, "reason": reason, "retry": retry}


# ---------------------------------------------------------------- state and log

def load_state():
    try:
        with open(STATE) as f:
            s = json.load(f)
    except FileNotFoundError:
        s = {}
    s.setdefault("accounts", {})      # account id -> {at, status, tx, block}
    s.setdefault("grants", [])         # times of grants in the rolling window (global cap)
    s.setdefault("clients", {})        # client tag -> [times]
    s.setdefault("daykeys", {})
    return s


def save_state(s):
    os.makedirs(os.path.dirname(STATE), exist_ok=True)
    tmp = STATE + ".tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        json.dump(s, f, indent=1, sort_keys=True)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, STATE)


def log(event, **kw):
    line = "%s %s %s" % (time.strftime("%Y-%m-%d %H:%M:%S"), event, " ".join("%s=%s" % (k, v) for k, v in kw.items()))
    try:
        os.makedirs(os.path.dirname(LOG), exist_ok=True)
        fd = os.open(LOG, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
        with os.fdopen(fd, "a") as f:
            f.write(line + "\n")
    except OSError:
        pass
    if DRY or "--foreground" in sys.argv:
        print(line, flush=True)


def tag(state, key, now):
    """The client's tag under today's key (a raw address is never stored)."""
    day = time.strftime("%Y-%m-%d", time.gmtime(now))
    k = state["daykeys"].get(day)
    if k is None:
        k = state["daykeys"][day] = secrets.token_hex(32)
    return hmac.new(bytes.fromhex(k), key.encode(), hashlib.sha256).hexdigest()[:20], day


def prune(state, now):
    cut = now - WINDOW
    state["grants"] = [t for t in state["grants"] if t > cut]
    for t in list(state["clients"]):
        state["clients"][t] = [x for x in state["clients"][t] if x > cut]
        if not state["clients"][t]:
            del state["clients"][t]
    acut = now - PER_ACCOUNT_HOURS * 3600
    for a in list(state["accounts"]):
        g = state["accounts"][a]
        if g.get("at", 0) <= acut and g.get("status") != "uncertain":
            del state["accounts"][a]
    keep = {time.strftime("%Y-%m-%d", time.gmtime(now)), time.strftime("%Y-%m-%d", time.gmtime(now - WINDOW))}
    for d in list(state["daykeys"]):
        if d not in keep:
            del state["daykeys"][d]


def client_count(state, key, now):
    """Grants of this client in the window: today's tag and yesterday's (the key rotates daily)."""
    n = 0
    for d in {time.strftime("%Y-%m-%d", time.gmtime(now)), time.strftime("%Y-%m-%d", time.gmtime(now - WINDOW))}:
        k = state["daykeys"].get(d)
        if k:
            t = hmac.new(bytes.fromhex(k), key.encode(), hashlib.sha256).hexdigest()[:20]
            n += len([x for x in state["clients"].get(t, []) if x > now - WINDOW])
    return n


# ---------------------------------------------------------------- the chain and the CLI

class SeqChain:
    def __init__(self, url):
        self.url = url

    def call(self, method, params):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
        req = urllib.request.Request(self.url, data=body, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=10) as r:
            v = json.loads(r.read(4 * 1024 * 1024))
        if v.get("error") is not None:
            raise RuntimeError("%s refused" % method)
        return v.get("result")

    def balance(self, account):
        return native_of(self.call("getAccount", [account]))


CHAIN = SeqChain(SEQUENCER)


def run_cli(pk_hex):
    """`wallet auth-transfer send --to-pk`; returns (ok, tx, block, detail)."""
    env = dict(os.environ, LEE_WALLET_HOME_DIR=WALLET, LEE_WALLET_PASSWORD="", LEZ_BRIDGE_SEQUENCER=SEQUENCER)
    try:
        r = subprocess.run([CLI, "wallet", "auth-transfer", "send", "--from", "Public/" + FROM, "--to-pk", pk_hex, "--amount", str(AMOUNT)],
                           env=env, capture_output=True, text=True, timeout=CLI_TIMEOUT)
    except subprocess.TimeoutExpired:
        return False, None, None, "timed out"
    out = r.stdout + r.stderr
    tx = re.search(r"Transaction hash is ([0-9a-f]{64})", out)
    blk = re.search(r"included in block (\d+)", out)
    return r.returncode == 0 and bool(tx), (tx.group(1) if tx else None), (int(blk.group(1)) if blk else None), out[-300:]


# ---------------------------------------------------------------- the two calls

def info(state, now=None, chain=None):
    now = now or time.time()
    chain = chain or CHAIN
    prune(state, now)
    out = {"amount": str(AMOUNT), "per_account_hours": PER_ACCOUNT_HOURS, "available": True, "reason": None}
    try:
        bal = chain.balance(FROM)
    except Exception:  # noqa: BLE001
        return dict(out, available=False, reason="The faucet can't read the chain right now; try again shortly.")
    if len(state["grants"]) >= DAILY_MAX:
        return dict(out, available=False, reason="Today's test LEZ is given out; try again tomorrow.")
    if bal - AMOUNT < MIN_BALANCE:
        return dict(out, available=False, reason="The faucet is empty; the bridge operator refills it.")
    return out


def handle(body, ip, state, now=None, cli=run_cli, chain=None):
    now = now or time.time()
    chain = chain or CHAIN
    if len(body) > MAX_BODY:
        return refuse("bad_key", "request too large")
    try:
        req = json.loads(body)
        pk_hex = str(req["public_key"]).lower().removeprefix("0x")
        asked = req.get("account")
    except (ValueError, KeyError, TypeError, AttributeError):
        return refuse("bad_key", "expected JSON {\"public_key\": \"<64 hex>\"}")
    if not re.fullmatch(r"[0-9a-f]{64}", pk_hex) or not valid_public_key(bytes.fromhex(pk_hex)):
        return refuse("bad_key", "That is not a LEZ public key (64 hex digits, an x-only secp256k1 key).")
    acct = account_id(bytes.fromhex(pk_hex))
    if asked is not None and str(asked).removeprefix("Public/") != acct:
        return refuse("bad_key", "That public key belongs to another account than the one given.")
    if acct == FROM:
        return refuse("bad_key", "That is the faucet's own account.")
    prune(state, now)
    g = state["accounts"].get(acct)
    if g:
        if g.get("status") == "uncertain":
            return refuse("busy", "An earlier transfer to this account is being checked by the operator.", retry=False)
        left = int((g["at"] + PER_ACCOUNT_HOURS * 3600 - now) // 3600) + 1
        return refuse("already_today", "This account got test LEZ in the last %d hours; try again in about %d hours." % (PER_ACCOUNT_HOURS, left))
    key = client_key(ip)
    if client_count(state, key, now) >= PER_CLIENT_DAY:
        return refuse("rate_limited", "Too many test LEZ grants from this address today; try again tomorrow.")
    if len(state["grants"]) >= DAILY_MAX:
        return refuse("faucet_empty", "Today's test LEZ is given out; try again tomorrow.")
    if state.get("inflight"):
        return refuse("busy", "The faucet is sending to someone else; try again in a few seconds.", retry=True)
    try:
        bal = chain.balance(FROM)
    except Exception:  # noqa: BLE001
        return refuse("busy", "The faucet can't read the chain right now; try again shortly.", retry=True)
    if bal - AMOUNT < MIN_BALANCE:
        log("EMPTY", balance=bal)
        return refuse("faucet_empty", "The faucet is empty; the bridge operator refills it.")
    if DRY:
        log("checked", account=acct)
        return {"ok": True, "checked": True, "amount": str(AMOUNT), "account": acct}
    t, _ = tag(state, key, now)
    state["accounts"][acct] = {"at": now, "status": "pending"}
    state["grants"].append(now)
    state["clients"].setdefault(t, []).append(now)
    state["inflight"] = acct
    save_state(state)  # the intent is on disk before the faucet signs
    ok, tx, blk, detail = cli(pk_hex)
    if ok:
        state["accounts"][acct] = {"at": now, "status": "sent", "tx": tx, "block": blk}
        state["inflight"] = None
        save_state(state)
        log("GRANT", account=acct, tx=tx, block=blk, amount=AMOUNT)
        return {"ok": True, "tx": tx, "block": blk, "amount": str(AMOUNT), "account": acct}
    if tx:  # sent, answer incomplete: counted, the operator checks it
        state["accounts"][acct] = {"at": now, "status": "uncertain", "tx": tx}
        state["inflight"] = None
        save_state(state)
        log("UNCERTAIN", account=acct, tx=tx)
        return refuse("busy", "The transfer was sent but not confirmed yet (tx %s); look at your balance in a minute." % tx)
    # nothing sent: everything given back
    del state["accounts"][acct]
    state["grants"].remove(now)
    state["clients"][t].remove(now)
    state["inflight"] = None
    save_state(state)
    log("failed", account=acct, detail=re.sub(r"[0-9a-fA-F]{40,}", "…", detail or "").replace("\n", " ")[:200])
    return refuse("busy", "The faucet couldn't send just now; try again in a minute.", retry=True)


# ---------------------------------------------------------------- HTTP

def load_auth():
    if not AUTH_FILE:
        return None
    st = os.stat(AUTH_FILE)
    if st.st_mode & 0o077:
        sys.exit("FAUCET_AUTH_FILE must be mode 600")
    with open(AUTH_FILE) as f:
        v = f.read().strip()
    return v or None


class Handler(http.server.BaseHTTPRequestHandler):
    server_version = "lez-demo-faucet/1"
    sys_version = ""

    def log_message(self, fmt, *args):
        pass

    def client_ip(self):
        given = self.headers.get(AUTH_HEADER, "")
        if AUTH and given and hmac.compare_digest(given.encode(), AUTH.encode()):
            ip = self.headers.get("CF-Connecting-IP", "").strip()
            try:
                return str(ipaddress.ip_address(ip))
            except ValueError:
                pass
        return self.client_address[0]

    def reply(self, obj):
        body = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.rstrip("/") != "/faucet/info":
            return self.reply({"ok": False, "code": "bad_key", "reason": "not found"})
        with LOCK:
            self.reply(info(load_state()))

    def do_POST(self):
        if self.path.rstrip("/") != "/faucet":
            return self.reply({"ok": False, "code": "bad_key", "reason": "not found"})
        try:
            n = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            n = -1
        if n <= 0 or n > MAX_BODY:
            return self.reply(refuse("bad_key", "request too large or empty"))
        body = self.rfile.read(n).decode("utf-8", "replace")
        if not LOCK.acquire(blocking=False):   # one transfer at a time; the others are told at once
            return self.reply(refuse("busy", "The faucet is sending to someone else; try again in a few seconds.", retry=True))
        try:
            try:
                out = handle(body, self.client_ip(), load_state())
            except Exception as e:  # never a stack trace to the client
                log("error", error=str(e)[:200])
                out = refuse("busy", "faucet error; try again later", retry=True)
        finally:
            LOCK.release()
        self.reply(out)


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def main():
    global DRY, AUTH
    if "--dry-run" in sys.argv:
        DRY = True
    if "--release" in sys.argv:   # the operator checked an uncertain transfer: forget it
        acct = sys.argv[sys.argv.index("--release") + 1]
        s = load_state()
        if s["accounts"].pop(acct, None) is None:
            sys.exit("no record for %s" % acct)
        save_state(s)
        log("released", account=acct)
        print("released")
        return
    if not re.fullmatch(r"[1-9A-HJ-NP-Za-km-z]{32,44}", FROM):
        sys.exit("FAUCET_ACCOUNT is not set to the faucet's public account id")
    host, port = LISTEN.rsplit(":", 1)
    if host not in ("127.0.0.1", "::1", "localhost"):
        sys.exit("FAUCET_LISTEN must be loopback")
    AUTH = load_auth()
    s = load_state()
    for a, g in s["accounts"].items():
        if g.get("status") == "pending":   # a stop between the intent and the answer
            g["status"] = "uncertain"
            log("UNCERTAIN", account=a, note="pending at start; not resent")
    s["inflight"] = None
    save_state(s)
    log("start", listen=LISTEN, account=FROM, amount=AMOUNT, daily_max=DAILY_MAX, per_client=PER_CLIENT_DAY,
        floor=MIN_BALANCE, proxy_auth=bool(AUTH), dry_run=DRY)
    Server((host, int(port)), Handler).serve_forever()


if __name__ == "__main__":
    main()
