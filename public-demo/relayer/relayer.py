#!/usr/bin/python3
"""Claim relayer for the public demo chain: the operator's side of `lez-bridge claim --relay URL`.

A public claim on LEZ needs a fee payer that signs it and holds the fee reserve before it runs. A
claimer's account that was never used holds nothing, and nobody can credit it by its account id
alone (DESIGN.md §3.6). The claimer's own signature admits its account; this service supplies
the fee: it co-signs the claim as the transaction's fee payer, from a DEDICATED payer account
(never the bridge admin/attestor/freezer; the CLI refuses one that holds a bridge role).

  GET  /relay/info   {"payer", "bridge", "max_gas", "max_fee", "burns"}: what the CLI builds against
  POST /relay        {"tx": "<hex of a half-signed PublicTransaction>", "check_only": false}
  POST /relay/burn   the same for a public BURN to this bridge ("Send back to Ethereum"): the
                     token program's BurnWithNotice of one registered denomination, notifying this
                     bridge's config (`relay-cosign --kind burn`: the same strict decode, signer,
                     nonce, fee-cap and simulation checks as a claim, plus the burn's own shape:
                     the root program is the bridge's token program, exactly the burner's token
                     actor, the definition's and the config's, the amount a denomination, burns not
                     frozen); its own caps (BURN_DAILY_MAX overall, IP_DAILY_BURNS per client, once
                     per exit commitment: a burn has no nullifier to tell "it applied", so a burn
                     that was included is never relayed again; one that was dropped may be). What a
                     griefer can cost: a relayed burn that applies destroys one denomination of the
                     sender's wrapped tokens; one that reverts is charged at most MAX_FEE; one that
                     is dropped (never included) costs the payer nothing, and its client slot is
                     NOT given back (the exit key is sender-chosen, so MAX_DROPS bounds nothing),
                     so a sender can't loop drops past IP_DAILY_BURNS. The payer's spend in any
                     24 h stays under (DAILY_MAX + BURN_DAILY_MAX) x MAX_FEE. Records are keyed
                     "exit:<E>" beside the claims' nullifiers.
                     -> {"ok": true, "tx_hash": ...} | {"ok": true, "checked": true}
                      | {"ok": false, "reason": ..., "retry": bool[, "retry_after": seconds]}

Every POST, under one lock (one claim at a time), with at most RELAY_QUEUE more waiting (L2; a
CLI check is a ~107 MB process):
1. size and shape: body <= MAX_BODY, JSON, `tx` hex <= 2 x 8192 characters;
2. the relay's own submissions are reconciled with the chain (below); while its last co-signed
   claim is not in a block yet, a claim (not a check) is answered "busy, retry" with
   `retry_after`, and the CLI rebuilds with the payer's fresh nonce (review M1: two co-signed
   claims built on the same payer nonce, one of them silently dropped);
3. caps before any work, over a ROLLING 24 h window: checks per client per 24 h, relayed claims
   per client per 24 h, relayed claims per 24 h overall. A client is its IPv4 address or its IPv6
   /64 (review M4), taken from CF-Connecting-IP only when the request carries the RPC filter's
   shared secret (X-Relay-Auth, RELAY_AUTH_FILE); any other loopback caller is keyed on its TCP
   peer, so all of them share one budget;
4. `lez-bridge relay-cosign` WITHOUT --submit (no key used): strict decode (a public stand-in
   claim on this bridge, exactly its declared accounts, no admission evidence, the claimer and the
   payer as the only signers, no tip, gas <= MAX_GAS, max_fee <= MAX_FEE, current nonces), the
   claim pre-checks (attested root, unspent nullifier, minting open, denomination, the deposit in
   the root), and a simulation that must apply;
5. once per claim nullifier (MAX_RELAYS, default 1; with 2, a deposit whose relayed claim was
   included but did not apply, its nullifier still unspent RETRY_AFTER s later, may be relayed
   once more); a co-signed claim that was never included does not count (at most MAX_DROPS
   times per deposit); with DESK_STATE set, only deposits made through the deposit desk (their
   commitment is in the fee-grants daemon's FeePaid events), i.e. deposits that paid the fee;
6. the payer keeps MIN_PAYER_BALANCE after this claim's max_fee;
7. the intent is written (fsynced), then `relay-cosign --submit` re-runs every check, signs as
   payer and submits. The record is `sent` until the chain shows the transaction in a block
   (`submitted`, final), or shows it can never be included (`dropped`: the payer's or the
   claimer's nonce moved past the one it carries and it is not in a block, still so DROP_CONFIRM
   s later; counters given back).
   A submission whose answer was lost (CLI failure, transport error after the send) is
   `uncertain` and is reconciled the same way when its transaction hash is known; otherwise the
   operator checks it and runs --release NULLIFIER.

What a griefer can cost: the payer pays only for claims that pass all of the above: real deposits
under an attested root, each once. The chain charges a co-signed claim at most its max_fee
(<= MAX_FEE) even if it reverts; a transaction refused for a stale nonce costs nothing (LEZ: a
nonce mismatch is ExecutionCharge::FREE). Known ways a checked claim can still revert after the
check: the bridge admin or freezer freezing minting (or changing the bridge's config) before it
lands. The claim nullifier binds the claimer, token and denomination (nf = SHA256("RFP022/nfc" |
k | dest | token | u256(denom))) and every bridge instruction must be signed by the claimer, so
only the claimer can spend its nullifier, and any transaction the claimer signs moves its nonce,
which makes a relayed claim queued behind it free. (With the reviewed nullifier, H(secret), a
second deposit with the same secret to another account could revert it: review L1, RAN.) This
list is what we know, not a proof; the bound below holds whatever the cause. So the most a
deposit can cost is MAX_RELAYS x MAX_FEE (defaults 1 x 2,500,000) and the most in any 24 h is
(DAILY_MAX + BURN_DAILY_MAX) x MAX_FEE native LEZ with burns (claims alone: DAILY_MAX x MAX_FEE) (defaults 100 x 2,500,000 = 250,000,000), and with DESK_STATE every
one of those deposits paid the desk's bridge fee on Sepolia (`feeBps`: 3% at deploy, 0.3% since
2026-11-14).

The payer key must be NEW for each chain (each genesis, each network), review M2: LEZ rc3 signs
the hash of "/LEE/v0.3/Message/Public/" + the message (lee/state_machine/src/public_transaction/
message.rs), with no chain id, channel or genesis in it, so a co-signed claim is valid on every
chain where the payer's nonce matches; after a re-genesis that funds the same payer, anyone could
replay every archived relayed claim, past every relay cap. The bridge config stores the chain's
channel id as `lez_chain_id`, but a program cannot read the chain's identity on rc3, so a claim
cannot bind the chain either: a documented residual. The relay therefore binds its payer key to
one chain: `--bind-payer` (once, right after the key is made and funded; refused unless the
payer's nonce on this chain is 0) writes the chain's channel id and genesis block hash into the
payer's wallet directory (relay-chain.json, next to the key, so it travels with it), and the
relay refuses to start when the binding is missing, names another payer, or names another chain.
It cannot prove that a key never signed on another chain; it can only refuse to use a bound key
anywhere else. Not caught: a key copied without its binding file, and a re-genesis that reuses
both the channel id and an identical genesis block.

Privacy (L6): no raw client address is written anywhere. Rate windows and log lines carry an HMAC
of the client key under a key of the UTC day, kept in state.json for two days and then deleted,
after which a tag cannot be linked back to an address; a nullifier record holds its client tag
only until it is final or 24 h old. State, log and temporary files are created mode 600. The
secret, the instruction bytes and the transaction hex are never logged or stored (the hex is
handed to the CLI in a mode-600 temporary file deleted at once); free-text reasons are logged with
long hex runs blanked. Errors returned to clients are fixed texts; details go to the log (L7).

Open relay (branch open-relay, 2026-11-17): nothing here is registered anywhere. Anyone may run
another instance beside this one with its own payer key, wallet, port, state and log
(RELAY_WALLET, PAYER, RELAY_LISTEN, RELAY_STATE, RELAY_LOG; `--bind-payer` once); claimers pick a
relay per claim (`claim --relay URL`, or a list tried in order). On a bridge that pays a relay
refund (`Instruction::Relayed`, `set-relay-refund`) the claimer names this relay's payer for it and
the bridge pays it from its float inside the claim; REQUIRE_REFUND=1 makes this instance co-sign
only such claims (relay-cosign --require-refund also checks that the float covers the refund).

Loopback only. Stdlib only (/usr/bin/python3 3.9). DRY_RUN=1: never runs --submit.
"""
import base64, hashlib, hmac, http.server, ipaddress, json, os, re, secrets, socketserver, subprocess, sys, tempfile, threading, time
import urllib.request

E = os.environ.get
P = E("P", "./public-chain")
CLI = E("CLI", P + "/bin/lez-bridge")
WALLET = E("RELAY_WALLET", P + "/wallets/relay")              # holds the payer's key only
SEQUENCER = E("SEQUENCER", "http://127.0.0.1:3290")
LISTEN = E("RELAY_LISTEN", "127.0.0.1:3297")
BRIDGE = E("BRIDGE", "JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7")
TOKEN_PROGRAM = E("TOKEN_PROGRAM", "")
PAYER = E("PAYER", "")
STATE = E("RELAY_STATE", P + "/relay/state.json")
LOG = E("RELAY_LOG", P + "/logs/relayer.log")
DESK_STATE = E("DESK_STATE", "")                               # fee-grants state.json, read only
AUTH_FILE = E("RELAY_AUTH_FILE", "")                           # the RPC filter's shared secret (mode 600)
MAX_GAS = int(E("MAX_GAS", "250000"))                          # rc3 measures 152,168 (+25% = 190,210)
MAX_FEE = int(E("MAX_FEE", "2500000"))                         # rc3 reserve ~1.54M; charged ~1.23M
MIN_PAYER_BALANCE = int(E("MIN_PAYER_BALANCE", "10000000"))
DAILY_MAX = int(E("DAILY_MAX", "100"))                         # relayed claims per rolling 24 h
IP_DAILY_MAX = int(E("IP_DAILY_MAX", "5"))                     # relayed claims per client per 24 h
IP_DAILY_CHECKS = int(E("IP_DAILY_CHECKS", "50"))              # POSTs per client per 24 h
RETRY_AFTER = int(E("RETRY_AFTER", "90"))                      # 6 blocks at 15 s
MAX_RELAYS = int(E("MAX_RELAYS", "1"))
MAX_DROPS = int(E("MAX_DROPS", "3"))                           # never-included submissions per deposit
BUSY_RETRY = int(E("BUSY_RETRY", "15"))                        # seconds the CLI waits on "busy"
STUCK_AFTER = int(E("STUCK_AFTER", "180"))                     # not included, no nonce moved: uncertain
DROP_CONFIRM = int(E("DROP_CONFIRM", "20"))                    # drop evidence must hold this long (> 1 block)
RECONCILE_EVERY = float(E("RECONCILE_EVERY", "3"))
BURN_DAILY_MAX = int(E("BURN_DAILY_MAX", "50"))                # relayed burns per rolling 24 h
IP_DAILY_BURNS = int(E("IP_DAILY_BURNS", "5"))                 # relayed burns per client per 24 h
RELAY_QUEUE = int(E("RELAY_QUEUE", "2"))                       # requests waiting behind the running one
REQUIRE_REFUND = E("REQUIRE_REFUND", "0") == "1"                # co-sign only claims that name this payer for the bridge's relay refund
WINDOW = 86400
MAX_BODY = 20000
CLI_TIMEOUT = int(E("CLI_TIMEOUT", "180"))
DRY = E("DRY_RUN", "0") == "1"
B58 = re.compile(r"^[1-9A-HJ-NP-Za-km-z]{32,44}$")
LOCK = threading.Lock()
ADMIT = threading.BoundedSemaphore(1 + max(0, RELAY_QUEUE))
AUTH_HEADER = "X-Relay-Auth"
BINDING_NAME = "relay-chain.json"
AUTH = None                                                    # loaded in main(); never logged
GENERIC_ERROR = "relay error; try again later (the operator's log has the details)"


# ---------------------------------------------------------------- text hygiene (M3, L7)

HEX_RUN = re.compile(r"(?:0x)?[0-9a-fA-F]{40,}")
INTERNAL = re.compile(r"https?://|/Users/|/Volumes/|/home/|/private/|/tmp/|/var/|\.rs:\d|127\.0\.0\.1|localhost|os error|"
                      r"connection refused|error sending request|stack backtrace", re.I)


def scrub(text):
    """For the log: long hex runs (a secret, instruction bytes, a transaction) blanked."""
    return HEX_RUN.sub("<hex>", str(text or ""))[:300]


def public_reason(text):
    """For a client: the CLI's designed refusal texts pass; anything that names an internal URL,
    path or transport error becomes a fixed text."""
    text = str(text or "refused")
    return GENERIC_ERROR if INTERNAL.search(text) else text[:300]


# ---------------------------------------------------------------- state and log

def log(event, **kw):
    kw.update(at=time.strftime("%Y-%m-%d %H:%M:%S"), event=event)
    line = json.dumps(kw, sort_keys=True)
    try:
        fd = os.open(LOG, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
        with os.fdopen(fd, "a") as f:
            f.write(line + "\n")
    except OSError:
        pass
    if DRY or "--foreground" in sys.argv:
        print(line, flush=True)


def load_state():
    try:
        with open(STATE) as f:
            s = json.load(f)
    except FileNotFoundError:
        s = {}
    s.setdefault("nullifiers", {})
    s.setdefault("window", {"relayed": [], "ips": {}})
    s["window"].setdefault("burned", [])
    s.setdefault("ipkeys", {})
    s.setdefault("inflight", None)
    s.pop("days", None)        # the old per-UTC-day counters (raw addresses): dropped
    return s


def save_state(s):
    tmp = STATE + ".tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        json.dump(s, f, indent=1, sort_keys=True)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, STATE)


def desk_commitments():
    """Commitments of desk deposits (they paid the bridge fee), from the fee-grants daemon."""
    try:
        with open(DESK_STATE) as f:
            events = json.load(f).get("events", {})
    except (OSError, ValueError):
        return set()
    return {c.lower() for c, ev in events.items() if not ev.get("refused")}


# ---------------------------------------------------------------- clients and windows (M4, L3, L6)

def client_key(ip):
    """IPv4: the address; IPv6: its /64 (one host usually holds a whole /64)."""
    try:
        a = ipaddress.ip_address(ip)
    except ValueError:
        return str(ip)
    if a.version == 6 and a.ipv4_mapped:
        a = a.ipv4_mapped
    if a.version == 6:
        return str(ipaddress.ip_network("%s/64" % a, strict=False))
    return str(a)


def _day(t):
    return time.strftime("%Y-%m-%d", time.gmtime(t))


def tags(state, key, now):
    """[today's tag, yesterday's tag] of a client key (HMAC under the day's key). New events go
    under today's; a rolling 24 h window finds every event under one of the two."""
    out = []
    for d in (_day(now), _day(now - WINDOW)):
        k = state["ipkeys"].get(d)
        if k is None and d == _day(now):
            k = state["ipkeys"][d] = secrets.token_hex(32)
        if k is not None:
            t = hmac.new(bytes.fromhex(k), key.encode(), hashlib.sha256).hexdigest()[:20]
            if t not in out:
                out.append(t)
    return out


def prune(state, now):
    """Drops events older than the window, empty clients, day keys older than yesterday, and the
    client tag of final or old nullifier records."""
    cut = now - WINDOW
    w = state["window"]
    w["relayed"] = [t for t in w["relayed"] if t > cut]
    w["burned"] = [t for t in w.get("burned", []) if t > cut]
    for tag in list(w["ips"]):
        e = w["ips"][tag]
        e["c"] = [t for t in e.get("c", []) if t > cut]
        e["r"] = [t for t in e.get("r", []) if t > cut]
        e["b"] = [t for t in e.get("b", []) if t > cut]
        if not e["b"]:
            e.pop("b")
        if not e["c"] and not e["r"] and not e.get("b"):
            del w["ips"][tag]
    keep = {_day(now), _day(now - WINDOW)}
    for d in list(state["ipkeys"]):
        if d not in keep:
            del state["ipkeys"][d]
    for g in state["nullifiers"].values():
        g.pop("ip", None)      # raw addresses of records written before 2026-11-14
        if "ip_tag" in g and (g.get("status") in ("submitted", "dropped") or g.get("at", 0) <= cut):
            del g["ip_tag"]


def count(state, tg, kind, now):
    cut = now - WINDOW
    return sum(1 for t in tg for x in state["window"]["ips"].get(t, {}).get(kind, []) if x > cut)


def add_event(state, tag, kind, t):
    state["window"]["ips"].setdefault(tag, {"c": [], "r": []}).setdefault(kind, []).append(t)


def give_back(state, g, keep_client=False):
    """Undo the counters of a submission that never went out or was never included. A dropped
    burn gives back only the global slot (`keep_client`): its exit key is the sender's choice, so
    only the client's own daily count bounds how often one sender can make burns that are dropped."""
    t = g.get("counted_at")
    if t is None:
        return
    win, kind = ("burned", "b") if g.get("kind") == "burn" else ("relayed", "r")
    if t in state["window"].get(win, []):
        state["window"][win].remove(t)
    e = state["window"]["ips"].get(g.get("ip_tag") or "", {})
    if not keep_client and t in e.get(kind, []):
        e[kind].remove(t)
    g.pop("counted_at", None)


# ---------------------------------------------------------------- the chain (reads only)

class SeqChain:
    """Plain JSON-RPC reads from the sequencer on loopback (no wallet, no key)."""

    def __init__(self, url):
        self.url = url

    def call(self, method, params):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
        req = urllib.request.Request(self.url, data=body, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=10) as r:
            v = json.loads(r.read(8 * 1024 * 1024))
        if v.get("error") is not None:
            raise RuntimeError("%s refused" % method)
        return v.get("result")

    def nonce(self, account):
        r = self.call("getAccount", [account])
        return 0 if r is None else int(r.get("nonce") or 0)

    def tx_block(self, tx_hash):
        """The block id holding the transaction, or None."""
        r = self.call("getTransaction", [tx_hash])
        return None if r is None else int(r[1])

    def identity(self):
        """(channel id, genesis block hash or None): what tells one chain from another."""
        return str(self.call("getChannelId", [])), genesis_hash(self.call("getBlock", [1]))


def genesis_hash(block):
    """The header hash of block 1. rc3 `getBlock` answers base64 of the borsh Block (`common`
    `borsh_base64`): header = block_id u64, prev_block_hash [32], hash [32], timestamp u64, so the
    hash is bytes 40..72. Never a hash of the whole answer: it carries `bedrock_status`, which
    changes as the block finalizes. None when the answer has another shape."""
    try:
        if isinstance(block, dict):
            h = block.get("header", {}).get("hash")
            return h if isinstance(h, str) else None
        raw = base64.b64decode(block, validate=True)
        return raw[40:72].hex() if len(raw) >= 72 and int.from_bytes(raw[:8], "little") == 1 else None
    except (TypeError, ValueError):
        return None


CHAIN = SeqChain(SEQUENCER)
_last_reconcile = [0.0]


def reconcile(state, now, chain, force=False):
    """Settles the relay's own submissions that are not final, from chain evidence only: in a
    block -> `submitted`; not in a block and the payer's or the claimer's nonce moved past the one
    the transaction carries -> it can never be included: `dropped` (counters given back); neither,
    STUCK_AFTER s after the send -> `uncertain` and no longer blocks other claims (they reuse the
    payer's nonce, so at most one of them can ever apply). A failed read changes nothing (a claim
    stays busy: fail closed). Nonces are read before the transaction, so an inclusion between the
    two reads is seen as included."""
    if not force and now - _last_reconcile[0] < RECONCILE_EVERY:
        return
    _last_reconcile[0] = now
    todo = [(nf, g) for nf, g in state["nullifiers"].items()
            if g.get("status") in ("sent", "uncertain") and g.get("payer_nonce") is not None]
    if not todo:
        if _stale_inflight(state):
            state["inflight"] = None
            save_state(state)
        return
    try:
        pn = chain.nonce(PAYER)
    except Exception as e:  # noqa: BLE001
        log("reconcile", note="chain read failed", error=scrub(e))
        return
    changed = False
    for nf, g in todo:
        try:
            cn = chain.nonce(g["claimer"])
            blk = chain.tx_block(g["tx_hash"]) if g.get("tx_hash") else None
        except Exception as e:  # noqa: BLE001
            log("reconcile", nullifier=nf, note="chain read failed", error=scrub(e))
            continue
        pm, cm = pn > int(g["payer_nonce"]), cn > int(g["claimer_nonce"])
        if blk is not None:
            g.update(status="submitted", block=blk, at=g.get("at", now))
            g.pop("ip_tag", None)
            g.pop("drop_seen", None)
            log("INCLUDED", nullifier=nf, tx=g["tx_hash"], block=blk)
        elif (g.get("tx_hash") and (pm or cm)) or (not g.get("tx_hash") and pm != cm):
            # A nonce it carries is spent by another transaction, so it can never be included,
            # and it was not charged. Without a hash, only one nonce moving says the same (had
            # this transaction landed, both the payer's and the claimer's would have moved).
            # The rc3 sequencer stores a block and moves the head state in one executor step,
            # and both reads go through that executor; the evidence must still hold DROP_CONFIRM
            # s later (one more block) before the slot is given back.
            g.setdefault("drop_seen", now)
            if now - g["drop_seen"] < DROP_CONFIRM:
                changed = True
                continue
            give_back(state, g, keep_client=g.get("kind") == "burn")
            drops = g.get("drops", 0) + 1
            earlier = g.get("txs", [])
            dropped_tx = g.get("tx_hash")
            if earlier:   # an earlier relayed claim of this deposit was included (and reverted)
                g.update(status="submitted", tx_hash=earlier[-1], txs=earlier[:-1], relays=max(1, g.get("relays", 1) - 1),
                         at=now - RETRY_AFTER, drops=drops)
            else:
                g.update(status="dropped", tx_hash=None, relays=max(0, g.get("relays", 1) - 1), drops=drops)
            for k in ("payer_nonce", "claimer_nonce", "reason", "drop_seen"):
                g.pop(k, None)
            log("DROPPED", nullifier=nf, tx=dropped_tx, drops=drops, note="never included; counters given back")
        elif pm and cm:
            # No hash (the CLI died) and both nonces moved: it probably landed; it cannot be told
            # from here whether it applied. The operator checks it.
            g["status"] = "uncertain"
            g.pop("payer_nonce", None)
            log("UNCERTAIN", nullifier=nf, note="both nonces moved; check the chain, then --release if it did not land")
        elif state.get("inflight") == nf and now - g.get("at", now) > STUCK_AFTER:
            g.update(status="uncertain", reason="not in a block after %d s; nonces unchanged" % STUCK_AFTER)
            log("UNCERTAIN", nullifier=nf, tx=g.get("tx_hash"), note="not included, nonces unchanged; no longer blocks other claims")
        else:
            continue
        if state.get("inflight") == nf:
            state["inflight"] = None
        changed = True
    if _stale_inflight(state):
        state["inflight"] = None
        changed = True
    if changed:
        save_state(state)


def _stale_inflight(state):
    """The in-flight mark points at nothing reconcile can still settle (never blocks for good)."""
    nf = state.get("inflight")
    g = state["nullifiers"].get(nf) if nf else None
    return bool(nf) and (g is None or g.get("status") not in ("pending", "sent", "uncertain") or g.get("payer_nonce") is None)


# ---------------------------------------------------------------- the CLI

def run_cli(tx_hex, submit, kind="claim"):
    """`lez-bridge relay-cosign`; returns its RELAY dict, or a retryable error dict."""
    d = os.path.dirname(STATE)
    fd, path = tempfile.mkstemp(prefix="tx-", dir=d)   # mode 600
    try:
        with os.fdopen(fd, "w") as f:
            f.write(tx_hex)
        args = [CLI, "relay-cosign", "--tx-file", path, "--bridge", BRIDGE, "--token-program", TOKEN_PROGRAM,
                "--payer", PAYER, "--max-gas", str(MAX_GAS), "--max-fee", str(MAX_FEE)] + (["--submit"] if submit else []) \
            + (["--kind", "burn"] if kind == "burn" else []) \
            + (["--require-refund"] if REQUIRE_REFUND and kind != "burn" else [])
        env = dict(os.environ, LEE_WALLET_HOME_DIR=WALLET, LEE_WALLET_PASSWORD="", LEZ_BRIDGE_SEQUENCER=SEQUENCER)
        r = subprocess.run(args, env=env, capture_output=True, text=True, timeout=CLI_TIMEOUT)
        for line in r.stdout.splitlines():
            if line.startswith("RELAY "):
                return json.loads(line[6:])
        tail = (r.stderr.strip().splitlines() or [""])[-1]
        return {"ok": False, "reason": "relay error: no answer from the CLI", "detail": tail or "exit %d" % r.returncode,
                "retry": True, "cli_failed": True}
    except subprocess.TimeoutExpired:
        return {"ok": False, "reason": "relay error: timed out", "retry": True, "cli_failed": True, "timeout": True}
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass


# ---------------------------------------------------------------- one request

def busy():
    # "stale nonce" in the text: the Names Wallet's module (modules/lez_bridge/rust-lib/src/relayed.rs,
    # relay_refused) re-signs the same call on it once the payer's nonce moved; the CLI rebuilds.
    return {"ok": False, "reason": "stale nonce: busy, the relay's payer has another transaction in flight; wait for it to land, then build it again and send it", "retry": True,
            "retry_after": BUSY_RETRY}


def handle_burn(body, ip, state, now=None, cli=run_cli, chain=None):
    """Decide one POST /relay/burn (a public burn to this bridge). The same order as a claim:
    shape, reconcile, busy, caps, the CLI's check, once per exit, the payer's floor, the intent on
    disk, the CLI's submit."""
    now = now or time.time()
    chain = chain or CHAIN
    if len(body) > MAX_BODY:
        return {"ok": False, "reason": "request too large", "retry": False}
    try:
        req = json.loads(body)
        tx = req["tx"]
        check_only = bool(req.get("check_only", False))
    except (ValueError, KeyError, TypeError):
        return {"ok": False, "reason": "expected JSON {\"tx\": hex}", "retry": False}
    if not isinstance(tx, str) or not re.fullmatch(r"(0x)?[0-9a-fA-F]{2,16384}", tx):
        return {"ok": False, "reason": "tx is not hex (or too long)", "retry": False}
    prune(state, now)
    reconcile(state, now, chain)
    if not check_only and state.get("inflight"):
        return busy()
    tg = tags(state, client_key(ip), now)
    if count(state, tg, "c", now) >= IP_DAILY_CHECKS:
        log("refused", ip=tg[0], kind="burn", reason="checks per client per 24 h")
        return {"ok": False, "reason": "too many requests from this address in 24 h", "retry": False}
    add_event(state, tg[0], "c", now)
    save_state(state)
    if not check_only:
        if len(state["window"].setdefault("burned", [])) >= BURN_DAILY_MAX:
            log("refused", ip=tg[0], kind="burn", reason="24 h burn cap")
            return {"ok": False, "reason": "the relay's 24 h cap for burns is reached; try later or pay the fee yourself", "retry": False}
        if count(state, tg, "b", now) >= IP_DAILY_BURNS:
            log("refused", ip=tg[0], kind="burn", reason="burns per client per 24 h")
            return {"ok": False, "reason": "too many relayed burns from this address in 24 h", "retry": False}
    checked = cli(tx, False, kind="burn")
    if not checked.get("ok"):
        log("refused", ip=tg[0], kind="burn", stage="check", reason=scrub(checked.get("reason")), detail=scrub(checked.get("detail")))
        return {"ok": False, "reason": public_reason(checked.get("reason")), "retry": bool(checked.get("retry"))}
    if checked.get("kind") != "burn":
        log("refused", ip=tg[0], kind="burn", reason="the CLI's answer is not a burn's")
        return {"ok": False, "reason": GENERIC_ERROR, "retry": False}
    key = "exit:" + checked["exit_commitment"].lower()
    g = state["nullifiers"].get(key)
    if g and g.get("drops", 0) >= MAX_DROPS and g["status"] == "dropped":
        return {"ok": False, "reason": "this burn's relayed transactions were dropped %d times; pay its fee yourself" % g.get("drops"), "retry": False}
    if g:
        st = g["status"]
        if st == "submitted":
            log("refused", ip=tg[0], kind="burn", exit=key, reason="already relayed", tx=g.get("tx_hash"))
            return {"ok": False, "reason": "this burn was already relayed (tx %s); if it did not apply, nothing was burned: start a new send-back (a new exit)" % g.get("tx_hash"), "retry": False}
        if st == "sent":
            return busy()
        if st != "dropped":
            return {"ok": False, "reason": "an earlier relay of this burn is %s; the operator checks it" % st, "retry": False}
    if int(checked["payer_balance"]) - int(checked["max_fee"]) < MIN_PAYER_BALANCE:
        log("STOP", reason="payer balance %s would fall under the floor %d" % (checked["payer_balance"], MIN_PAYER_BALANCE))
        return {"ok": False, "reason": "the relay is out of funds; try later or pay the fee yourself", "retry": False}
    if check_only or DRY:
        log("checked", ip=tg[0], kind="burn", exit=key, burner=checked["burner"], gas=checked["gas_limit"], max_fee=checked["max_fee"], dry_run=DRY)
        return {"ok": True, "checked": True}
    try:
        pn = int(checked["payer_nonce"])
        cn = int(checked["burner_nonce"])
    except (KeyError, ValueError):
        return {"ok": False, "reason": GENERIC_ERROR, "retry": True}
    prev = json.loads(json.dumps(g)) if g else None
    state["nullifiers"][key] = {"kind": "burn", "status": "pending", "at": now, "counted_at": now, "ip_tag": tg[0], "claimer": checked["burner"],
                                "exit": checked["exit_commitment"], "denom": checked.get("denom"), "max_fee": checked["max_fee"], "relays": 1,
                                "txs": [], "drops": (g or {}).get("drops", 0), "payer_nonce": pn, "claimer_nonce": cn}
    state["window"].setdefault("burned", []).append(now)
    add_event(state, tg[0], "b", now)
    state["inflight"] = key
    save_state(state)
    sent = cli(tx, True, kind="burn")
    g = state["nullifiers"][key]
    if sent.get("ok") and sent.get("tx_hash"):
        g.update(status="sent", tx_hash=sent["tx_hash"], at=now)
        save_state(state)
        log("SENT", ip=tg[0], kind="burn", exit=key, burner=checked["burner"], tx=sent["tx_hash"])
        return {"ok": True, "tx_hash": sent["tx_hash"]}
    if sent.get("cli_failed") or sent.get("uncertain"):
        g.update(status="uncertain", reason=scrub(sent.get("reason")), tx_hash=sent.get("tx_hash"), at=now)
        save_state(state)
        log("UNCERTAIN", kind="burn", exit=key, tx=sent.get("tx_hash"), reason=scrub(sent.get("reason")))
        return {"ok": False, "reason": "the relay could not confirm the submission; the operator checks it", "retry": False}
    give_back(state, g)
    if prev:
        state["nullifiers"][key] = prev
    else:
        del state["nullifiers"][key]
    state["inflight"] = None
    save_state(state)
    log("refused", ip=tg[0], kind="burn", stage="submit", exit=key, reason=scrub(sent.get("reason")))
    return {"ok": False, "reason": public_reason(sent.get("reason")), "retry": bool(sent.get("retry"))}


def handle(body, ip, state, now=None, cli=run_cli, desk=desk_commitments, chain=None):
    """Decide one POST. Returns the JSON reply (a dict). Pure apart from `cli`, `desk`, `chain`
    and the state file, so it is unit-tested offline. `ip` is the client's address (already
    authenticated by the caller); it is keyed (IPv6 /64) and tagged here, never stored."""
    now = now or time.time()
    chain = chain or CHAIN
    if len(body) > MAX_BODY:
        return {"ok": False, "reason": "request too large", "retry": False}
    try:
        req = json.loads(body)
        tx = req["tx"]
        check_only = bool(req.get("check_only", False))
    except (ValueError, KeyError, TypeError):
        return {"ok": False, "reason": "expected JSON {\"tx\": hex}", "retry": False}
    if not isinstance(tx, str) or not re.fullmatch(r"(0x)?[0-9a-fA-F]{2,16384}", tx):
        return {"ok": False, "reason": "tx is not hex (or too long)", "retry": False}
    prune(state, now)
    reconcile(state, now, chain)
    if not check_only and state.get("inflight"):
        return busy()          # before any counter: a busy answer costs the client nothing
    tg = tags(state, client_key(ip), now)
    if count(state, tg, "c", now) >= IP_DAILY_CHECKS:
        log("refused", ip=tg[0], reason="checks per client per 24 h")
        return {"ok": False, "reason": "too many requests from this address in 24 h", "retry": False}
    add_event(state, tg[0], "c", now)
    save_state(state)
    if not check_only:
        if len(state["window"]["relayed"]) >= DAILY_MAX:
            log("refused", ip=tg[0], reason="24 h cap")
            return {"ok": False, "reason": "the relay's 24 h cap is reached; try later or pay the fee yourself", "retry": False}
        if count(state, tg, "r", now) >= IP_DAILY_MAX:
            log("refused", ip=tg[0], reason="claims per client per 24 h")
            return {"ok": False, "reason": "too many relayed claims from this address in 24 h", "retry": False}


    checked = cli(tx, False)
    if not checked.get("ok"):
        log("refused", ip=tg[0], stage="check", reason=scrub(checked.get("reason")), detail=scrub(checked.get("detail")))
        return {"ok": False, "reason": public_reason(checked.get("reason")), "retry": bool(checked.get("retry"))}
    nf, c = checked["nullifier"], checked["commitment"].lower()
    g = state["nullifiers"].get(nf)
    if g and g.get("drops", 0) >= MAX_DROPS and g["status"] in ("submitted", "dropped"):
        log("refused", ip=tg[0], nullifier=nf, reason="dropped too often", drops=g.get("drops"))
        return {"ok": False, "reason": "this claim's relayed transactions were dropped %d times; pay its fee yourself" % g.get("drops"), "retry": False}
    if g:
        st = g["status"]
        if st == "submitted":
            # The check above passed, so the nullifier is still unspent: the earlier relayed claim
            # was included and did not apply (reverted). One retry, once it has had time to land.
            if now - g["at"] < RETRY_AFTER or g.get("relays", 1) >= MAX_RELAYS:
                log("refused", ip=tg[0], nullifier=nf, reason="already relayed", tx=g.get("tx_hash"), relays=g.get("relays", 1))
                return {"ok": False, "reason": "this claim was already relayed (tx %s); pay its fee yourself if it did not apply" % g.get("tx_hash"), "retry": False}
            log("retry", nullifier=nf, tx=g.get("tx_hash"), note="earlier relayed claim did not apply (nullifier unspent)")
        elif st == "dropped":
            pass               # never included: relayed again (MAX_DROPS checked above)
        elif st == "sent":
            return busy()
        else:  # pending, uncertain
            log("refused", ip=tg[0], nullifier=nf, reason="earlier relay " + st)
            return {"ok": False, "reason": "an earlier relay of this claim is %s; the operator checks it" % st, "retry": False}
    if DESK_STATE and c not in desk():
        log("refused", ip=tg[0], nullifier=nf, commitment=c, reason="not a desk deposit")
        return {"ok": False, "reason": "the relay pays only for deposits made through the deposit page (they paid the bridge fee)", "retry": False}
    if int(checked["payer_balance"]) - int(checked["max_fee"]) < MIN_PAYER_BALANCE:
        log("STOP", reason="payer balance %s would fall under the floor %d" % (checked["payer_balance"], MIN_PAYER_BALANCE))
        return {"ok": False, "reason": "the relay is out of funds; try later or pay the fee yourself", "retry": False}
    if check_only or DRY:
        log("checked", ip=tg[0], nullifier=nf, commitment=c, claimer=checked["claimer"], gas=checked["gas_limit"],
            max_fee=checked["max_fee"], cycles=checked["cycles"], dry_run=DRY)
        return {"ok": True, "checked": True}

    # The nonces the transaction carries (the check compared them with the chain just now).
    try:
        pn = int(checked["payer_nonce"]) if "payer_nonce" in checked else chain.nonce(PAYER)
        cn = int(checked["claimer_nonce"]) if "claimer_nonce" in checked else chain.nonce(checked["claimer"])
    except Exception as e:  # noqa: BLE001
        log("refused", ip=tg[0], nullifier=nf, reason="nonces unreadable", error=scrub(e))
        return {"ok": False, "reason": GENERIC_ERROR, "retry": True}
    prev = json.loads(json.dumps(g)) if g else None
    relays = (g or {}).get("relays", 0) + 1
    earlier = (g or {}).get("txs", []) + ([g["tx_hash"]] if g and g.get("tx_hash") else [])
    state["nullifiers"][nf] = {"status": "pending", "at": now, "counted_at": now, "ip_tag": tg[0], "claimer": checked["claimer"],
                               "commitment": c, "max_fee": checked["max_fee"], "leaf_index": checked["leaf_index"], "relays": relays,
                               "txs": earlier, "drops": (g or {}).get("drops", 0), "payer_nonce": pn, "claimer_nonce": cn}
    state["window"]["relayed"].append(now)
    add_event(state, tg[0], "r", now)
    state["inflight"] = nf
    save_state(state)  # the intent is on disk before the payer signs
    sent = cli(tx, True)
    g = state["nullifiers"][nf]
    if sent.get("ok") and sent.get("tx_hash"):
        g.update(status="sent", tx_hash=sent["tx_hash"], at=now)
        save_state(state)
        log("SENT", ip=tg[0], nullifier=nf, commitment=c, claimer=checked["claimer"], tx=sent["tx_hash"],
            gas=sent.get("gas_limit"), max_fee=sent.get("max_fee"), cycles=sent.get("cycles"))
        return {"ok": True, "tx_hash": sent["tx_hash"]}
    if sent.get("cli_failed") or sent.get("uncertain"):
        # The answer was lost: the transaction may be in the mempool. It keeps the payer busy
        # until the chain settles it (reconcile), or STUCK_AFTER.
        g.update(status="uncertain", reason=scrub(sent.get("reason")), tx_hash=sent.get("tx_hash"), at=now)
        save_state(state)
        log("UNCERTAIN", nullifier=nf, tx=sent.get("tx_hash"), reason=scrub(sent.get("reason")), detail=scrub(sent.get("detail")),
            note="reconciled from the chain when its hash is known; else check it, then --release")
        return {"ok": False, "reason": "the relay could not confirm the submission; the operator checks it", "retry": False}
    # refused at the second check (e.g. the payer's nonce moved): nothing was sent
    give_back(state, g)
    if prev:
        state["nullifiers"][nf] = prev
    else:
        del state["nullifiers"][nf]
    state["inflight"] = None
    save_state(state)
    log("refused", ip=tg[0], stage="submit", nullifier=nf, reason=scrub(sent.get("reason")), detail=scrub(sent.get("detail")))
    return {"ok": False, "reason": public_reason(sent.get("reason")), "retry": bool(sent.get("retry"))}


# ---------------------------------------------------------------- operator commands

def release(nf, chain):
    """--release NULLIFIER: frees a deposit whose relayed claim never landed. `pending`/`uncertain`
    without a known hash: the operator has checked the chain. With known hashes (`sent`,
    `uncertain`, `submitted`, including records written before 2026-11-14, when `submitted` meant
    "sent"): refused if any of them is in a block."""
    s = load_state()
    reconcile(s, time.time(), chain, force=True)
    g = s["nullifiers"].get(nf)
    if not g:
        return "no relay record for %s" % nf
    hashes = [h for h in [g.get("tx_hash")] + g.get("txs", []) if h]
    if g["status"] in ("submitted", "sent") and not hashes:
        return "%s: no transaction hash recorded; nothing to check" % nf
    for h in hashes:
        try:
            blk = chain.tx_block(h)
        except Exception as e:  # noqa: BLE001
            return "chain read failed (%s); nothing changed" % scrub(e)
        if blk is not None:
            return "%s: tx %s is in block %d; it landed, nothing to release" % (nf, h, blk)
    if g["status"] == "sent" and s.get("inflight") == nf:
        return "%s: still in flight (not in a block, no nonce moved yet); wait, or retry after %d s" % (nf, STUCK_AFTER)
    del s["nullifiers"][nf]
    if s.get("inflight") == nf:
        s["inflight"] = None
    save_state(s)
    log("released", nullifier=nf, status=g["status"], txs=hashes, note="operator released it: it may be relayed again")
    return None


def binding_path():
    return os.path.join(WALLET, BINDING_NAME)


def read_binding():
    try:
        with open(binding_path()) as f:
            return json.load(f)
    except FileNotFoundError:
        return None


def check_binding(chain):
    """None if the payer is bound to this chain, else why the relay must not start (M2)."""
    b = read_binding()
    if b is None:
        return ("the payer has no chain binding (%s): a relay payer must be a NEW key for this chain; make one "
                "(RUNBOOK §13, demo.sh new-relay-payer), fund it, then relayer.py --bind-payer" % binding_path())
    channel, genesis = chain.identity()
    if b.get("payer") != PAYER:
        return "the binding in %s names payer %s, not PAYER %s" % (binding_path(), b.get("payer"), PAYER)
    # The channel id always; the genesis hash when both sides could read it (None: another shape).
    if b.get("channel_id") != channel or (b.get("genesis") and genesis and b.get("genesis") != genesis):
        return ("this payer key is bound to another chain (channel %s, genesis %s; this chain: %s, %s). A co-signed claim "
                "carries no chain id, so a payer used on one chain must never be funded on another: make a NEW payer key"
                % (b.get("channel_id"), b.get("genesis"), channel, genesis))
    return None


def bind_payer(chain):
    if read_binding() is not None:
        return "a binding exists already (%s); a payer is bound once" % binding_path()
    if not B58.fullmatch(PAYER):
        return "PAYER is not set to an account id"
    channel, genesis = chain.identity()
    n = chain.nonce(PAYER)
    if n != 0:
        return "the payer's nonce on this chain is %d: it has signed here already; bind only a new key" % n
    fd = os.open(binding_path(), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as f:
        json.dump({"payer": PAYER, "channel_id": channel, "genesis": genesis,
                   "bound_at": time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime())}, f, indent=1, sort_keys=True)
    log("bound", payer=PAYER, channel=channel, genesis=genesis)
    return None


def load_auth():
    """The RPC filter's shared secret: RELAY_AUTH_FILE (mode 600 or stricter), else RELAY_AUTH."""
    if AUTH_FILE:
        st = os.stat(AUTH_FILE)
        if st.st_mode & 0o077:
            sys.exit("RELAY_AUTH_FILE must be mode 600")
        with open(AUTH_FILE) as f:
            v = f.read().strip()
    else:
        v = E("RELAY_AUTH", "").strip()
    if v and len(v) < 32:
        sys.exit("the relay's shared secret must be at least 32 characters")
    return v or None


# ---------------------------------------------------------------- HTTP

class Handler(http.server.BaseHTTPRequestHandler):
    server_version = "lez-demo-relayer/2"
    sys_version = ""

    def log_message(self, fmt, *args):
        pass

    def client_ip(self):
        """CF-Connecting-IP only from the RPC filter (it proves itself with the shared secret);
        any other caller is its TCP peer (loopback: all of them share one budget)."""
        given = self.headers.get(AUTH_HEADER, "")
        if AUTH and given and hmac.compare_digest(given.encode(), AUTH.encode()):
            ip = self.headers.get("CF-Connecting-IP", "").strip()
            try:
                return str(ipaddress.ip_address(ip))
            except ValueError:
                pass
        return self.client_address[0]

    def reply(self, status, obj):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.rstrip("/") in ("/relay/info", "/info"):
            return self.reply(200, {"payer": PAYER, "bridge": BRIDGE, "max_gas": MAX_GAS, "max_fee": str(MAX_FEE), "burns": True,
                                    "refund_required": REQUIRE_REFUND})
        self.reply(404, {"ok": False, "reason": "not found"})

    def do_POST(self):
        path = self.path.rstrip("/")
        if path not in ("/relay", "", "/relay/burn"):
            return self.reply(404, {"ok": False, "reason": "not found"})
        try:
            n = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            n = -1
        if n <= 0 or n > MAX_BODY:
            return self.reply(413, {"ok": False, "reason": "request too large or empty", "retry": False})
        body = self.rfile.read(n).decode("utf-8", "replace")
        if not ADMIT.acquire(blocking=False):   # one running, RELAY_QUEUE waiting, the rest turned away
            return self.reply(200, dict(busy(), reason="busy: too many requests waiting; retry"))
        try:
            with LOCK:
                state = load_state()
                try:
                    out = (handle_burn if path == "/relay/burn" else handle)(body, self.client_ip(), state)
                except Exception as e:  # never a stack trace to the client
                    log("error", error=scrub(e))
                    out = {"ok": False, "reason": GENERIC_ERROR, "retry": True}
        finally:
            ADMIT.release()
        self.reply(200, out)


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def main():
    global DRY, AUTH
    if "--dry-run" in sys.argv:
        DRY = True
    os.makedirs(os.path.dirname(STATE), exist_ok=True)
    for f in (STATE, LOG):
        try:
            os.chmod(f, 0o600)
        except FileNotFoundError:
            pass
    if "--release" in sys.argv:
        why = release(sys.argv[sys.argv.index("--release") + 1].lower(), CHAIN)
        if why:
            sys.exit(why)
        print("released")
        return
    if "--bind-payer" in sys.argv:
        why = bind_payer(CHAIN)
        if why:
            sys.exit(why)
        print("bound %s to this chain (%s)" % (PAYER, binding_path()))
        return
    for name, v in (("PAYER", PAYER), ("TOKEN_PROGRAM", TOKEN_PROGRAM), ("BRIDGE", BRIDGE)):
        if not B58.fullmatch(v):
            sys.exit("%s is not set to an account id" % name)
    host, port = LISTEN.rsplit(":", 1)
    if host not in ("127.0.0.1", "::1", "localhost"):
        sys.exit("RELAY_LISTEN must be loopback")
    AUTH = load_auth()
    why = check_binding(CHAIN)
    if why:
        sys.exit(why)
    d = os.path.dirname(STATE)
    for name in os.listdir(d):                     # transaction files of a killed run
        if name.startswith("tx-"):
            try:
                os.unlink(os.path.join(d, name))
            except OSError:
                pass
    s = load_state()
    for nf, g in s["nullifiers"].items():
        if g["status"] == "pending":  # a stop between the intent and the answer
            g["status"] = "uncertain"
            log("UNCERTAIN", nullifier=nf, note="pending at start; not resent; reconciled from the chain if possible, else --release")
    prune(s, time.time())
    save_state(s)
    log("start", listen=LISTEN, payer=PAYER, bridge=BRIDGE, token_program=TOKEN_PROGRAM, max_gas=MAX_GAS, max_fee=MAX_FEE,
        daily_max=DAILY_MAX, ip_daily_max=IP_DAILY_MAX, burn_daily_max=BURN_DAILY_MAX, ip_daily_burns=IP_DAILY_BURNS, floor=MIN_PAYER_BALANCE, desk_only=bool(DESK_STATE), dry_run=DRY,
        proxy_auth=bool(AUTH), queue=RELAY_QUEUE, require_refund=REQUIRE_REFUND)
    Server((host, int(port)), Handler).serve_forever()


if __name__ == "__main__":
    main()
