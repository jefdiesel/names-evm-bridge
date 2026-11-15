#!/usr/bin/python3
"""Fee grants for the public demo chain: the LEZ side of the DepositDesk bridge fee.

SINCE THE RELAYED CLAIM (branch grant-by-id): the deposit page names no account any more (the desk's
`lezPublicKey` is zero on every deposit), so this daemon grants nothing to new deposits; a public
claim's fee is paid by the claim relayer (public-demo/relayer/relayer.py) co-signing it. The daemon
stays for ACCOUNTING: desk fees collected on Sepolia against LEZ paid out (grants, the float's
drawdown for private claims, and with RELAY_PAYER set the relay payer's drawdown for public
claims), and for any event that does carry a key (none expected). Its feed of desk commitments
(state.json `events`) is also what the relayer reads with DESK_STATE to relay desk deposits only.

A deposit made through the Sepolia DepositDesk (vault/src/desk/DepositDesk.sol) pays a bridge fee
(`feeBps` of the denomination, in the deposited asset: 3% at deploy, 0.3% since 2026-11-14) and names, optionally, the 32-byte PUBLIC KEY of
a LEZ public account. This daemon sends that account native LEZ for its claim's fee reserve, from
the operator wallet, once per deposit, so a claimer with an empty account can make a public claim
without anyone acting by hand. A zero key (private and demo accounts) gets no grant: private
claims are sponsored by the bridge's float, which the fee also pays for (see the report line).

Every POLL seconds (60, never under 30):
1. LEZ reads (plain JSON-RPC, no wallet): channel id, the operator's native balance, the bridge
   float (the Config PDA's native balance).
2. Sepolia reads at block tag `finalized` (SEPOLIA_RPC: one RPC or a comma-separated list, each
   request tried in order until one answers; default Tenderly's public gateway, then PublicNode):
   the desk's FeePaid logs since the last scanned block. Each is checked against its transaction receipt: the log comes from the
   desk, and the same transaction holds the vault's Deposit log with the same commitment and leaf
   index (the vault is the source of truth for leaves).
3. A grant is due when: the key is non-zero and a valid BIP-340 x-only key; the deposit's leaf is
   under the root the root attester has recorded on LEZ (leaf index < the attested deposit count,
   and the deposit's block <= the attested Sepolia block; read from the attester's state.json,
   read only); no grant was ever made for that commitment (or, after a chain reset and only with
   REGRANT_ON_RESET=1, none on the current channel); today's caps and the operator's floor allow it.
4. The intent is written (fsynced) before the send; the send is
   `lez-bridge wallet auth-transfer send --from Public/OP --to-pk PK --amount GRANT` (a never-used
   account can only be credited by its public key); the result is checked by the recipient's
   balance. A pending intent found at start is never resent: it is reconciled from the balance or
   marked `uncertain` for the operator (--release-uncertain C makes it retryable).

Never twice: the state file is keyed by commitment (a vault commitment is used once), and the
send happens only after the intent is on disk. After a chain reset (new channel id) nothing is
re-granted unless REGRANT_ON_RESET=1; a re-genesis forgets every claim nullifier, so at the moment
of a reset every earlier leaf is unclaimed on the new chain, and each gets at most one grant there.

Old history: the daemon scans forward from its saved `scanned` block, so it reads only recent
blocks while it runs. A fresh state (or a stop of more than about a day) makes it read back to
DESK_FROM_BLOCK, which PublicNode's Sepolia RPC no longer serves (2026-11-14: "pruned history
unavailable", and `null` for old receipts). Such an answer moves to the next RPC; if none answers,
the poll fails and `scanned` does not move. A provider that silently answered an old range with
fewer logs could not be detected here (the desk has no counter): known limit, and the effect
fails closed (the relayer would refuse that deposit as not a desk deposit).

DRY_RUN=1 (or --dry-run) never runs the CLI and never opens the operator wallet: it does every read
and logs what it would send. --once runs one poll; --report prints the accounting line and exits.

Stdlib only (/usr/bin/python3 3.9). Logs every grant with the Sepolia tx, the LEZ tx and amounts.
"""
import datetime, hashlib, json, os, re, subprocess, sys, time, urllib.request

E = os.environ.get
P = E("P", "./public-chain")
CLI = E("CLI", P + "/bin/lez-bridge")
WALLET = E("OPERATOR_WALLET", P + "/wallets/operator")
SEQUENCER = E("SEQUENCER", "http://127.0.0.1:3290")          # where grants are sent
LEZ_RPC = E("LEZ_RPC", SEQUENCER)                              # where reads go (public URL for a dry run)
SEPOLIAS = [u.strip() for u in E("SEPOLIA_RPC", "https://sepolia.gateway.tenderly.co,https://ethereum-sepolia-rpc.publicnode.com").split(",") if u.strip()]
BRIDGE = E("BRIDGE", "JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7")
OPERATOR = E("OPERATOR", "BVDcg8SbSdCTKzx4mJbSpVgEuaoPEMhxmmRwvVCGyBnD")
VAULT = E("VAULT", "0x8C8b2664De28e5287158db4DB65D448638871b74").lower()
DESK = E("DESK", "").lower()                                    # empty until the owner deploys it
DESK_FROM_BLOCK = int(E("DESK_FROM_BLOCK", "0"))                # the desk's creation block
ATTESTER_STATE = E("ATTESTER_STATE", P + "/attester/state.json")
STATE = E("GRANTS_STATE", P + "/grants/state.json")
LOG = E("GRANTS_LOG", P + "/logs/fee-grants.log")
POLL = max(30, int(E("POLL", "60")))
# Grant size: the fee reserve of one public claim on the chain in use, plus 25% (RUNBOOK §12).
#   rc1 (live since 11-13): the wallet declares the default 2,000,000 gas: reserve
#       2,000,000 x 8 + 1,893 bytes x 8 = 16,015,144; + 25% = 20,018,930 -> 20,100,000.
#   rc3: the wallet declares measured gas + 25% = 190,210: reserve 1,536,824; + 25% = 1,921,030
#       -> 2,000,000 (set GRANT_AMOUNT=2000000 after the rc3 re-genesis).
GRANT = int(E("GRANT_AMOUNT", "20100000"))
DAILY_MAX_GRANTS = int(E("DAILY_MAX_GRANTS", "50"))
DAILY_MAX_AMOUNT = int(E("DAILY_MAX_AMOUNT", str(50 * 20100000)))
MIN_OPERATOR_BALANCE = int(E("MIN_OPERATOR_BALANCE", str(10**12)))   # the attester pays from it too
REGRANT_ON_RESET = E("REGRANT_ON_RESET", "0") == "1"
RELAY_PAYER = E("RELAY_PAYER", "")                              # the claim relayer's payer (accounting)
MAX_ATTEMPTS = 3
LOG_CHUNK = int(E("LOG_CHUNK", "10000"))   # the deposit page scans in 10,000-block chunks too
MAX_FINALITY_AGE = 30 * 60
UA = {"Content-Type": "application/json", "User-Agent": "lez-demo-fee-grants/1"}

FEE_PAID_TOPIC = "0x2fe535c5fd3b340521910740117c9de5d7485f5881b3b63c47bf695b1d8da057"  # FeePaid(address,bytes32,bytes32,uint32,uint64,uint256)
# keccak256("Deposit(address,uint32,bytes32,uint64,bytes32)"); the same value as DEPOSIT_TOPIC in
# web/deposit/src/lib/vault.js (test_fee_grants.py checks the two agree).
DEPOSIT_TOPIC = "0x7c6270dc673eb588f0d459a71fcc44183d7b70b659eb552028fa91fadbe8ea69"
NATIVE_PROGRAM = "11111111111111111111111111111111"
PUBLIC_PREFIX = b"/LEE/v0.3/AccountId/Public/" + b"\0" * 5        # 32 bytes, rc1 and rc3
PDA_PREFIX = b"/LEE/v0.2/AccountId/PDA/" + b"\0" * 8              # 32 bytes
CONFIG_SEED = b"/RFP022/bridge/config/v1/000000/"
SECP_P = 2**256 - 2**32 - 977
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
TOKEN_NAMES = {"0x0000000000000000000000000000000000000000": ("Sepolia ETH", 18),
               "0xac207b9bfebf3ef7c26297ea6abadeeccc4aa71e": ("dUSDC", 6)}
DRY = E("DRY_RUN", "0") == "1"


# ---------------------------------------------------------------- pure helpers (unit-tested)

def b58encode(b):
    n = int.from_bytes(b, "big")
    s = ""
    while n:
        n, r = divmod(n, 58)
        s = B58[r] + s
    return "1" * (len(b) - len(b.lstrip(b"\0"))) + s


def b58decode(s):
    n = 0
    for ch in s:
        i = B58.find(ch)
        if i < 0:
            raise ValueError("not base58")
        n = n * 58 + i
    pad = len(s) - len(s.lstrip("1"))
    body = n.to_bytes((n.bit_length() + 7) // 8, "big") if n else b""
    return b"\0" * pad + body


def valid_public_key(pk):
    """A LEZ public key is a BIP-340 x-only secp256k1 key: x < p and x^3 + 7 a square mod p."""
    if len(pk) != 32 or pk == b"\0" * 32:
        return False
    x = int.from_bytes(pk, "big")
    if x >= SECP_P:
        return False
    rhs = (pow(x, 3, SECP_P) + 7) % SECP_P
    return pow(rhs, (SECP_P - 1) // 2, SECP_P) == 1


def account_id(pk):
    return b58encode(hashlib.sha256(PUBLIC_PREFIX + pk).digest())


def config_pda(bridge):
    return b58encode(hashlib.sha256(PDA_PREFIX + b58decode(bridge) + CONFIG_SEED).digest())


def native_of(account_json):
    """Native balance from a getAccount result (u128 LE under the native program); None = 0."""
    if account_json is None:
        return 0
    st = (account_json.get("data") or {}).get("actor_states") or {}
    raw = st.get(NATIVE_PROGRAM) or []
    if len(raw) not in (0, 16):
        raise ValueError("native balance is not 16 bytes")
    return int.from_bytes(bytes(raw), "little")


def decode_fee_paid(lg):
    """FeePaid(address indexed token, bytes32 indexed lezPublicKey, bytes32 indexed commitment,
    uint32 denomIndex, uint64 leafIndex, uint256 fee). Returns a dict or raises ValueError."""
    t = lg.get("topics") or []
    if len(t) != 4 or t[0].lower() != FEE_PAID_TOPIC:
        raise ValueError("not a FeePaid log")
    data = bytes.fromhex(lg["data"][2:])
    if len(data) != 96:
        raise ValueError("FeePaid data is not 3 words")
    token_word = bytes.fromhex(t[1][2:])
    if len(token_word) != 32 or token_word[:12] != b"\0" * 12:
        raise ValueError("bad token topic")
    return {
        "token": "0x" + token_word[12:].hex(),
        "pk": t[2][2:].lower().rjust(64, "0"),
        "commitment": "0x" + t[3][2:].lower().rjust(64, "0"),
        "denom_index": int.from_bytes(data[0:32], "big"),
        "leaf_index": int.from_bytes(data[32:64], "big"),
        "fee": int.from_bytes(data[64:96], "big"),
        "tx": lg["transactionHash"].lower(),
        "block": int(lg["blockNumber"], 16),
        "log_index": int(lg["logIndex"], 16),
    }


def deposit_in_receipt(receipt, desk, vault, ev):
    """The FeePaid log is the desk's and the vault's Deposit log in the same tx has the same
    commitment and leaf index. Returns None if fine, else the reason."""
    if int(receipt.get("status", "0x0"), 16) != 1:
        return "transaction failed"
    fee_ok = dep_ok = False
    for lg in receipt.get("logs", []):
        addr, topics = lg["address"].lower(), [x.lower() for x in lg.get("topics", [])]
        if addr == desk and topics and topics[0] == FEE_PAID_TOPIC and int(lg["logIndex"], 16) == ev["log_index"]:
            fee_ok = True
        if addr == vault and topics and topics[0] == DEPOSIT_TOPIC:
            data = bytes.fromhex(lg["data"][2:])
            if len(data) == 128 and "0x" + data[32:64].hex() == ev["commitment"] \
                    and int.from_bytes(data[64:96], "big") == ev["leaf_index"]:
                dep_ok = True
    if not fee_ok:
        return "FeePaid log is not the desk's"
    if not dep_ok:
        return "no vault Deposit with this commitment and leaf in the same transaction"
    return None


def grant_due(ev, att, state, channel, today, regrant):
    """Why a verified event gets no grant now (a string), or None if a grant is due."""
    if ev.get("refused"):
        return "refused: " + ev["refused"]
    if int(ev["pk"], 16) == 0:
        return "no account named (private or demo account; the float pays)"
    if not att or "deposit_count" not in att or "sepolia_block" not in att:
        return "waiting: no attested root yet"
    if ev["leaf_index"] >= att["deposit_count"] or ev["block"] > att["sepolia_block"]:
        return "waiting: leaf not under the attested root yet"
    g = state["grants"].get(ev["commitment"])
    if g:
        if g["status"] in ("pending", "uncertain"):
            return "held: earlier grant is " + g["status"]
        if any(s.get("channel") == channel for s in g.get("sent", [])):
            return "granted"
        if g.get("sent") and not regrant:
            return "granted on an earlier chain (REGRANT_ON_RESET off)"
        if g["status"] == "failed" and g.get("attempts", 0) >= MAX_ATTEMPTS:
            return "failed %d times" % g["attempts"]
    day = state["days"].get(today, {"count": 0, "amount": 0})
    if day["count"] + 1 > DAILY_MAX_GRANTS or day["amount"] + GRANT > DAILY_MAX_AMOUNT:
        return "held: daily cap reached"
    return None


def report_line(state, op_balance, float_now, relay_now=None):
    fees = {}
    n = with_key = 0
    for ev in state["events"].values():
        if ev.get("refused"):
            continue
        n += 1
        with_key += int(ev["pk"], 16) != 0
        fees[ev["token"]] = fees.get(ev["token"], 0) + ev["fee"]
    parts = []
    for tok, amt in sorted(fees.items()):
        name, dec = TOKEN_NAMES.get(tok, (tok, 0))
        parts.append("%s %s (%d base units)" % (fmt_units(amt, dec), name, amt))
    grants = [s for g in state["grants"].values() for s in g.get("sent", [])]
    paid = sum(s["amount"] for s in grants)
    fl = state["float"]
    rl = state.get("relay") or {}
    relay = ("" if relay_now is None else
             " | relay payer: drawdown %d since %s (relayed public claims' fees; balance %s, top-ups seen %d)"
             % (rl.get("drawdown", 0), rl.get("since", "-"), relay_now, rl.get("topups", 0)))
    return ("report: desk deposits %d (%d named an account); fees collected on Sepolia: %s | "
            "LEZ paid: grants %d native LEZ in %d grants, float drawdown %d since %s (float now %s, "
            "top-ups seen %d), total %d | operator balance %s"
            % (n, with_key, ", ".join(parts) or "none", paid, len(grants), fl.get("drawdown", 0),
               fl.get("since", "-"), float_now, fl.get("topups", 0), paid + fl.get("drawdown", 0) + rl.get("drawdown", 0), op_balance)
            + relay)


def fmt_units(amount, decimals):
    if decimals == 0:
        return str(amount)
    s = str(amount).rjust(decimals + 1, "0")
    whole, frac = s[:-decimals], s[-decimals:].rstrip("0")
    return whole + ("." + frac if frac else "")


def track_float(state, float_now, key="float"):
    fl = state.setdefault(key, {})
    if "last" not in fl:
        fl.update(last=float_now, drawdown=0, topups=0, since=time.strftime("%Y-%m-%d %H:%M"))
        return
    if float_now < fl["last"]:
        fl["drawdown"] += fl["last"] - float_now
    elif float_now > fl["last"]:
        fl["topups"] += float_now - fl["last"]
    fl["last"] = float_now


# ---------------------------------------------------------------- I/O

def log(msg):
    line = "%s %s" % (time.strftime("%Y-%m-%d %H:%M:%S"), msg)
    try:
        with open(LOG, "a") as f:
            f.write(line + "\n")
    except OSError:
        pass
    if DRY or "--once" in sys.argv or "--report" in sys.argv:
        print(line)


def post(url, method, params, timeout=20):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    with urllib.request.urlopen(urllib.request.Request(url, body, UA), timeout=timeout) as r:
        out = json.load(r)
    if "error" in out:
        raise RuntimeError("%s: %s" % (method, out["error"]))
    return out["result"]


def eth(method, params):
    """A Sepolia read through the first RPC of SEPOLIAS that answers. An error (pruned history,
    rate limit, transport) or a `null` result (PublicNode's answer for a pruned receipt) moves to
    the next; when none answers, the error names each."""
    fails = []
    for url in SEPOLIAS:
        try:
            out = post(url, method, params)
        except Exception as e:
            fails.append("%s: %s" % (url, str(e)[:200]))
            continue
        if out is None:
            fails.append("%s: %s answered null (pruned history?)" % (url, method))
            continue
        return out
    raise RuntimeError("no Sepolia RPC answered %s: %s" % (method, "; ".join(fails) or "SEPOLIA_RPC is empty"))


def lez(method, params):
    return post(LEZ_RPC, method, params)


def lez_balance(acct):
    return native_of(lez("getAccount", [acct]))


def load_state():
    try:
        with open(STATE) as f:
            s = json.load(f)
    except FileNotFoundError:
        s = {}
    s.setdefault("events", {})
    s.setdefault("grants", {})
    s.setdefault("days", {})
    s.setdefault("float", {})
    s.setdefault("scanned", DESK_FROM_BLOCK - 1)
    return s


def save_state(s):
    tmp = STATE + ".tmp"
    with open(tmp, "w") as f:
        json.dump(s, f, indent=1, sort_keys=True)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, STATE)


def read_attester():
    try:
        with open(ATTESTER_STATE) as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def send_grant(pk_hex):
    env = dict(os.environ, LEE_WALLET_HOME_DIR=WALLET, LEE_WALLET_PASSWORD="", LEZ_BRIDGE_SEQUENCER=SEQUENCER)
    r = subprocess.run([CLI, "wallet", "auth-transfer", "send", "--from", "Public/" + OPERATOR,
                        "--to-pk", pk_hex, "--amount", str(GRANT)],
                       env=env, capture_output=True, text=True, timeout=300)
    out = r.stdout + r.stderr
    tx = re.search(r"Transaction hash is ([0-9a-f]{64})", out)
    blk = re.search(r"included in block (\d+)", out)
    return r.returncode, (tx.group(1) if tx else None), (int(blk.group(1)) if blk else None), out


def wait_balance(acct, want, seconds=90):
    end = time.time() + seconds
    while True:
        try:
            bal = lez_balance(acct)
            if bal >= want or time.time() > end:
                return bal
        except Exception:
            if time.time() > end:
                return None
        time.sleep(10)


# ---------------------------------------------------------------- one poll

def scan(state, fin_no):
    if not DESK:
        return 0
    found = 0
    start = state["scanned"] + 1
    while start <= fin_no:
        end = min(fin_no, start + LOG_CHUNK - 1)
        logs = eth("eth_getLogs", [{"address": DESK, "topics": [FEE_PAID_TOPIC],
                                    "fromBlock": hex(start), "toBlock": hex(end)}])
        for lg in logs:
            if lg.get("removed"):
                continue
            try:
                ev = decode_fee_paid(lg)
            except ValueError as e:
                log("REFUSED malformed log in tx %s: %s" % (lg.get("transactionHash"), e))
                continue
            if ev["commitment"] in state["events"]:
                continue  # seen before (a commitment is used once by the vault)
            receipt = eth("eth_getTransactionReceipt", [ev["tx"]])
            if not receipt:   # never `refused` for want of a receipt; the poll fails, `scanned` stays
                raise RuntimeError("no receipt for Sepolia tx %s (pruned history?): set SEPOLIA_RPC to an RPC with full history" % ev["tx"])
            why = deposit_in_receipt(receipt, DESK, VAULT, ev)
            if why is None and int(ev["pk"], 16) != 0 and not valid_public_key(bytes.fromhex(ev["pk"])):
                why = "the named key is not a valid LEZ public key"
            if why:
                ev["refused"] = why
                log("REFUSED commitment %s (Sepolia tx %s): %s" % (ev["commitment"], ev["tx"], why))
            else:
                ev["account"] = account_id(bytes.fromhex(ev["pk"])) if int(ev["pk"], 16) else None
                log("seen deposit leaf %d, fee %d of %s, account %s (Sepolia tx %s)"
                    % (ev["leaf_index"], ev["fee"], ev["token"], ev["account"] or "none", ev["tx"]))
            state["events"][ev["commitment"]] = ev
            found += 1
        state["scanned"] = end
        save_state(state)
        start = end + 1
    return found


def reconcile_pending(state):
    """At start: a pending intent is never resent. Settle it from the recipient's balance."""
    for c, g in state["grants"].items():
        if g["status"] != "pending":
            continue
        try:
            bal = lez_balance(g["account"])
        except Exception as e:
            log("pending grant %s: balance unreadable (%s); left pending" % (c, e))
            continue
        if bal >= g["balance_before"] + g["amount"]:
            g["sent"].append({"channel": g["channel"], "amount": g["amount"], "lez_tx": None,
                              "lez_block": None, "at": int(time.time()), "reconciled": True})
            g["status"] = "sent"
            log("pending grant %s reconciled as SENT (balance %d)" % (c, bal))
        else:
            g["status"] = "uncertain"
            log("pending grant %s marked UNCERTAIN (balance %d): not resent; check and run --release-uncertain" % (c, bal))
    save_state(state)


def grant_all(state, att, channel, op_balance):
    today = datetime.datetime.utcnow().strftime("%Y-%m-%d")
    for c, ev in sorted(state["events"].items(), key=lambda kv: kv[1]["leaf_index"]):
        why = grant_due(ev, att, state, channel, today, REGRANT_ON_RESET)
        if why:
            continue
        if op_balance - GRANT < MIN_OPERATOR_BALANCE:
            if not state.get("low_logged"):
                log("STOP granting: operator balance %d would fall under the floor %d" % (op_balance, MIN_OPERATOR_BALANCE))
                state["low_logged"] = True
            return op_balance
        acct = ev["account"]
        before = lez_balance(acct)
        if DRY:
            log("DRY RUN: would grant %d native LEZ to %s (key %s) for leaf %d, Sepolia tx %s"
                % (GRANT, acct, ev["pk"], ev["leaf_index"], ev["tx"]))
            continue
        g = state["grants"].setdefault(c, {"sent": [], "attempts": 0})
        g.update(status="pending", channel=channel, account=acct, pk=ev["pk"], amount=GRANT,
                 balance_before=before, attempts=g.get("attempts", 0) + 1, at=int(time.time()))
        day = state["days"].setdefault(today, {"count": 0, "amount": 0})
        day["count"] += 1
        day["amount"] += GRANT
        save_state(state)  # intent on disk before the send
        try:
            rc, tx, blk, out = send_grant(ev["pk"])
        except (FileNotFoundError, PermissionError) as e:   # the CLI never ran: nothing was sent
            rc, tx, blk, out = 127, None, None, "CLI did not start: %s" % e
        except Exception as e:                              # timeout or worse: the balance decides
            rc, tx, blk, out = 1, "unknown", None, "CLI call failed: %s" % e
        bal = wait_balance(acct, before + GRANT)
        if bal is not None and bal >= before + GRANT:
            g["sent"].append({"channel": channel, "amount": GRANT, "lez_tx": tx, "lez_block": blk, "at": int(time.time())})
            g["status"] = "sent"
            op_balance -= GRANT
            log("GRANT %d native LEZ -> %s (key %s) for leaf %d commitment %s | Sepolia tx %s fee %d %s | LEZ tx %s block %s"
                % (GRANT, acct, ev["pk"], ev["leaf_index"], c, ev["tx"], ev["fee"], ev["token"], tx, blk))
        elif rc != 0 and tx is None and bal == before:
            g["status"] = "failed"
            day["count"] -= 1
            day["amount"] -= GRANT
            last = out.strip().splitlines()[-1][:200] if out.strip() else ""
            log("grant to %s for %s FAILED before sending (attempt %d of %d): %s" % (acct, c, g["attempts"], MAX_ATTEMPTS, last))
        else:
            g["status"] = "uncertain"
            log("grant to %s for %s UNCERTAIN (rc %d, tx %s, balance %s -> %s): not resent; check and run --release-uncertain"
                % (acct, c, rc, tx, before, bal))
        save_state(state)
    return op_balance


def poll(state, last):
    channel = lez("getChannelId", [])
    if state.get("channel") and state["channel"] != channel:
        log("RESET: LEZ channel %s -> %s; earlier grants %s" % (
            state["channel"], channel, "will be re-issued once here (REGRANT_ON_RESET=1)" if REGRANT_ON_RESET else "are NOT re-issued"))
        state["float"] = {}
        state["relay"] = {}
    state["channel"] = channel
    op_balance = lez_balance(OPERATOR)
    float_now = lez_balance(config_pda(BRIDGE))
    track_float(state, float_now)
    relay_now = lez_balance(RELAY_PAYER) if RELAY_PAYER else None
    if relay_now is not None:
        track_float(state, relay_now, "relay")
    if op_balance - GRANT >= MIN_OPERATOR_BALANCE:
        state["low_logged"] = False
    fin = eth("eth_getBlockByNumber", ["finalized", False])
    fin_no, age = int(fin["number"], 16), time.time() - int(fin["timestamp"], 16)
    if age > MAX_FINALITY_AGE:
        log("Sepolia finalized block %d is %d s old: not scanning" % (fin_no, age))
    else:
        scan(state, fin_no)
    att = read_attester()
    op_balance = grant_all(state, att, channel, op_balance)
    save_state(state)
    if time.time() - last.get("report", 0) > 3600 or "--once" in sys.argv:
        log(report_line(state, op_balance, float_now, relay_now))
        last["report"] = time.time()
    return op_balance


def main():
    global DRY
    if "--dry-run" in sys.argv:
        DRY = True
    os.makedirs(os.path.dirname(STATE), exist_ok=True)
    state = load_state()
    if "--release-uncertain" in sys.argv:
        c = sys.argv[sys.argv.index("--release-uncertain") + 1].lower()
        g = state["grants"].get(c)
        if not g or g["status"] != "uncertain":
            sys.exit("no uncertain grant for %s" % c)
        g["status"] = "failed"
        save_state(state)
        log("operator released uncertain grant %s: it will be retried" % c)
        return
    if "--report" in sys.argv:
        print(report_line(state, lez_balance(OPERATOR), lez_balance(config_pda(BRIDGE)),
                          lez_balance(RELAY_PAYER) if RELAY_PAYER else None))
        return
    if not DESK and not DRY:
        sys.exit("DESK is not set: nothing to watch (deploy the desk, then set DESK and DESK_FROM_BLOCK)")
    if DESK and not re.fullmatch(r"0x[0-9a-f]{40}", DESK):
        sys.exit("DESK is not an address")
    if DESK and DESK_FROM_BLOCK <= 0:
        sys.exit("DESK_FROM_BLOCK is not set: give the desk's creation block (a scan from 0 is refused)")
    log("start%s: desk %s from block %d, vault %s, operator %s, grant %d, caps %d grants / %d a day, floor %d, regrant on reset %s, Sepolia RPCs %s"
        % (" (DRY RUN: no sends, no wallet)" if DRY else "", DESK or "(none)", DESK_FROM_BLOCK, VAULT, OPERATOR,
           GRANT, DAILY_MAX_GRANTS, DAILY_MAX_AMOUNT, MIN_OPERATOR_BALANCE, REGRANT_ON_RESET, ", ".join(SEPOLIAS)))
    if not DRY:
        reconcile_pending(state)
    last, failures = {}, 0
    while True:
        try:
            poll(state, last)
            failures = 0
        except Exception as e:  # network, RPC or CLI trouble: log (rate-limited) and retry later
            failures += 1
            if failures <= 3 or failures % 30 == 0:
                log("error (%d in a row): %s" % (failures, str(e)[:300]))
            if "--once" in sys.argv:
                sys.exit(1)
        if "--once" in sys.argv:
            return
        time.sleep(POLL if failures < 5 else min(600, POLL * 5))


if __name__ == "__main__":
    main()
