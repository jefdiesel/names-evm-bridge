#!/usr/bin/python3
"""Settlement service for "Bridge to Ethereum": proves, wraps and releases a burned exit, so a wallet
needs no helper and nobody pastes anything. TESTNET: the coins have no value.

It runs beside relayer.py and faucet.py, behind the public endpoint's RPC filter
(public-demo/proxy/lez-rpc-proxy.py --settle), loopback only, stdlib only.

  GET  /settle/info            {"available", "reason", "fee": "0", "vaults": [...]}
  POST /settle                 {"vault", "receipt_hex", "journal_hex"}             a wallet with the proof helper
                               {"vault", "exit": {"secret", "eth_dest", "token", "denom", "max_relay_fee",
                                                  "cfg_id", "leaves": [...], "index"}}   a wallet without it
                               -> {"ok": true, "id": <release nullifier>, "stage"} | {"ok": false, "code", "reason", "retry"}
  POST /settle/status          {"id"} -> {"stage", "text", "seal"?, "tx"?, "block"?, "releasable_at"?, "error"?}
  POST /settle/bundle          {"fetch_id"} -> {"bundle": base64} (the wrap runner only; ciphertext; while wrapping)
                               also the outbound updater's bundles: UPDATER_BUNDLES/<fetch_id>.bin while younger
                               than BUNDLE_TTL (public-demo/updater/updater.py writes them; same WRAP_KEY)

Stages: queued -> proving (exit jobs) -> wrapping -> releasable_at -> releasing -> released | failed.

Each step, resumable from the state file (`SETTLE_DIR/state.json`, mode 600):
- accept: the vault is listed; for an exit, its leaf is in `leaves` at `index`, the leaves give a root the vault recorded
  (`exitRootTime(root) != 0`), `cfg_id` is the vault's; for a receipt, the journal's cfgId and root are the vault's and
  recorded; the release nullifier is unspent ("the bridge's update hasn't recorded this burn yet" otherwise);
- proving: the installed release prover (`release-prover.sh` with COEXIST=1: it may run beside a long update proof
  when at least 30% of memory is free, else the job waits "for memory"); the exit secret goes to it on stdin and is
  kept only in this process's memory until then: never in the state file, never logged (after a restart an exit job
  that was not proved yet fails with "send it again");
- the receipt is verified natively against the release image (`outbound_host verify`);
- wrapping: an encrypted bundle (tar receipt.bin journal.hex | openssl enc -aes-256-cbc -pbkdf2 -iter 200000 -md
  sha256 under WRAP_KEY, as public-demo/prove/wrap-private.sh) is served by `fetch_id` while the run lasts; the
  workflow wrap-private.yml is dispatched with that fetch_id on the public repository (GitHub token: Actions read and
  write); the run's seal artifact is downloaded and checked with a read-only `verify(seal, image, sha256(journal))` on
  RISC Zero's verifier 3.1.0; the bundle is deleted;
- releasable_at: waits for `releasableAt(token, root)`;
- releasing: `release(...)` from the settle key (a keystore and its password file, `cast send --keystore`), relayer
  and fee as the proof names them (0x0 / 0 for the proofs this service makes); refused under SETTLE_FLOOR wei.
Limits: PER_CLIENT_DAY jobs per client (rolling 24 h), DAILY_MAX overall, one job in each stage at a time (one worker).
Logs carry the job id (the release nullifier, published by `release` anyway), stages, run ids, tx hashes: no secret,
receipt, journal or address. DRY_RUN=1: nothing is proved, dispatched or sent.
"""
import base64, hashlib, hmac, http.server, io, ipaddress, json, os, re, secrets, socketserver, subprocess, sys, tarfile, threading, time, zipfile
import urllib.request, urllib.error

E = os.environ.get
P = E("P", "./public-chain")
DIR = E("SETTLE_DIR", P + "/settle")
STATE = DIR + "/state.json"
JOBS = DIR + "/jobs"
LOG = E("SETTLE_LOG", P + "/logs/settle.log")
LISTEN = E("SETTLE_LISTEN", "127.0.0.1:3299")
AUTH_FILE = E("SETTLE_AUTH_FILE", "")
SEPOLIAS = [u.strip() for u in E("SEPOLIA_RPC", "https://sepolia.gateway.tenderly.co,https://ethereum-sepolia-rpc.publicnode.com").split(",") if u.strip()]
VAULTS = [v.strip().lower() for v in E("VAULTS", "0xafbdf192434e9888bb23dae6f2d15eea6246ed10,0xc2c6a059eab928966b24fa5549d441a3b17b54ed").split(",") if v.strip()]
VERIFIER = E("VERIFIER", "0x8c915b5071206C6E328BC18529586c513408aB3b")
IMAGE = E("RELEASE_IMAGE_ID", "0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4")
PROVER = E("RELEASE_PROVER", "<local path>")
HOST = E("OUTBOUND_HOST", "<local path>")
CAST = E("CAST", os.path.expanduser("~/.foundry/bin/cast"))
OPENSSL = E("OPENSSL", "openssl")
KEYSTORE = E("SETTLE_KEYSTORE", DIR + "/keystore")
KEYSTORE_PW = E("SETTLE_KEYSTORE_PW", DIR + "/keystore.pw")
ADDRESS = E("SETTLE_ADDRESS", "").lower()               # the settle key's address (it pays the release gas)
FLOOR = int(E("SETTLE_FLOOR", str(10**16)))             # 0.01 Sepolia ETH
WRAP_KEY_FILE = E("WRAP_KEY_FILE", DIR + "/wrap.key")
TOKEN_FILE = E("GITHUB_TOKEN_FILE", DIR + "/github-token")
REPO = E("WRAP_REPO", "jefdiesel/names-evm-bridge")
WORKFLOW = E("WRAP_WORKFLOW", "wrap-private.yml")
REF = E("WRAP_REF", "main")
PER_CLIENT_DAY = int(E("PER_CLIENT_DAY", "5"))
DAILY_MAX = int(E("DAILY_MAX", "50"))
WRAP_TIMEOUT = int(E("WRAP_TIMEOUT", str(90 * 60)))
BUNDLE_TTL = int(E("BUNDLE_TTL", str(2 * 3600)))
UPDATER_BUNDLES = E("UPDATER_BUNDLES", P + "/updater/bundles")   # the outbound updater's encrypted update receipts
TICK = float(E("TICK", "10"))
DRY = E("DRY_RUN", "0") == "1"
MAX_BODY = 700 * 1024
AUTH_HEADER = "X-Relay-Auth"
AUTH = None
LOCK = threading.RLock()
SECRETS = {}          # job id -> exit secret (hex), memory only, until proved
S = None              # the state, shared by the HTTP threads and the worker; changed and saved under LOCK
STAGES = ("queued", "proving", "wrapping", "releasable_at", "releasing", "released", "failed")
TEXT = {
    "queued": "Waiting its turn on the bridge's machine.",
    "proving": "Making the release proof (about 20 s).",
    "wrapping": "Wrapping the proof for Ethereum (about 4 min).",
    "releasable_at": "Waiting for the vault's delay.",
    "releasing": "Sending the release on Ethereum.",
    "released": "Released on Ethereum.",
    "failed": "Stopped.",
}

EXIT_LEAF_TAG = bytes.fromhex("22018742ad6028fda6cb0726afaae90ea20dff3cef6b1ec5b3c14f856d76c59c")
SEL = {"exitRootTime": "89cc0f3a", "spent": "161bf9b1", "releasableAt": "949ef977", "cfgId": "ed9305e3",
       "tokenConfig": "fe136c4e", "verify": "ab750e75", "releaseImageId": "7b4d5f9a"}
RELEASE_SIG = "release(bytes,bytes32,bytes32,address,uint32,address,address,uint256)"
ZERO20 = "0x" + "00" * 20


# ---------------------------------------------------------------- pure helpers (unit-tested)

def sha(*parts):
    return hashlib.sha256(b"".join(parts)).digest()


def u256(v):
    return int(v).to_bytes(32, "big")


def h2b(h, n=None):
    b = bytes.fromhex(str(h).lower().removeprefix("0x"))
    if n is not None and len(b) != n:
        raise ValueError("expected %d bytes" % n)
    return b


def zero_hashes():
    z = [b"\0" * 32]
    for _ in range(32):
        z.append(sha(z[-1], z[-1]))
    return z


def tree_root(leaves):
    """The vault's exit tree root over `leaves` (SHA-256, depth 32, zero hashes)."""
    zs = zero_hashes()
    layer = list(leaves)
    for h in range(32):
        if len(layer) % 2:
            layer.append(zs[h])
        layer = [sha(layer[i], layer[i + 1]) for i in range(0, len(layer), 2)] if layer else []
    return layer[0] if layer else zs[32]


def exit_commitment(k, dest, token, denom, max_fee, cfg):
    return sha(b"RFP022/exit", k, dest, token, u256(denom), u256(max_fee), cfg)


def exit_leaf(cfg, token, denom, e):
    return sha(EXIT_LEAF_TAG, cfg, token, u256(denom), e)


def nullifier(k):
    return sha(b"RFP022/nfr", k)


def decode_journal(j):
    """The 256-byte release journal: abi.encode(cfgId, root, nullifier, token, denom, recipient, relayer, relayFee)."""
    if len(j) != 256:
        raise ValueError("a release journal is 256 bytes")
    w = [j[i * 32:(i + 1) * 32] for i in range(8)]
    for i in (3, 5, 6):
        if any(w[i][:12]):
            raise ValueError("not an address word")
    return {"cfg": w[0], "root": w[1], "nullifier": w[2], "token": w[3][12:], "denom": int.from_bytes(w[4], "big"),
            "recipient": w[5][12:], "relayer": w[6][12:], "relay_fee": int.from_bytes(w[7], "big")}


def release_calldata(sel, seal, root, nf, token, denom_index, recipient, relayer, fee):
    head = (u256(8 * 32) + root + nf + b"\0" * 12 + token + u256(denom_index) + b"\0" * 12 + recipient + b"\0" * 12 + relayer + u256(fee))
    tail = u256(len(seal)) + seal + b"\0" * ((32 - len(seal) % 32) % 32)
    return sel + head + tail


def verify_calldata(seal, image, digest):
    return h2b(SEL["verify"]) + u256(0x60) + image + digest + u256(len(seal)) + seal + b"\0" * ((32 - len(seal) % 32) % 32)


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


# ---------------------------------------------------------------- the outside world (replaced in tests)

class Sepolia:
    def call(self, to, data_hex):
        last = None
        for url in SEPOLIAS:
            body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "eth_call", "params": [{"to": to, "data": data_hex}, "latest"]}).encode()
            try:
                req = urllib.request.Request(url, data=body, headers={"Content-Type": "application/json", "User-Agent": "lez-demo-settle/1"})
                with urllib.request.urlopen(req, timeout=20) as r:
                    v = json.loads(r.read(4 * 1024 * 1024))
            except Exception as e:  # noqa: BLE001
                last = e
                continue
            if v.get("error"):
                raise RuntimeError("reverted: %s" % str(v["error"].get("message", ""))[:120])
            return h2b(v.get("result") or "0x")
        raise RuntimeError("Sepolia can't be read (%s)" % last)

    def rpc(self, method, params):
        for url in SEPOLIAS:
            body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
            try:
                req = urllib.request.Request(url, data=body, headers={"Content-Type": "application/json", "User-Agent": "lez-demo-settle/1"})
                with urllib.request.urlopen(req, timeout=20) as r:
                    v = json.loads(r.read(1024 * 1024))
                if "result" in v:
                    return v["result"]
            except Exception:  # noqa: BLE001
                continue
        raise RuntimeError("Sepolia can't be read")

    def now(self):
        return int(self.rpc("eth_getBlockByNumber", ["latest", False])["timestamp"], 16)

    def balance(self, addr):
        return int(self.rpc("eth_getBalance", [addr, "latest"]), 16)


class GitHub:
    def __init__(self, token_file=TOKEN_FILE):
        self.token_file = token_file

    def _req(self, method, path, body=None, raw=False):
        with open(self.token_file) as f:
            token = f.read().strip()
        url = path if path.startswith(("https://", "http://")) else "https://api.github.com" + path
        req = urllib.request.Request(url, data=None if body is None else json.dumps(body).encode(), method=method,
                                     headers={"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28",
                                              "User-Agent": "lez-demo-settle/1"})
        # Not sent on a redirect: the artifact download answers 302 to a signed storage URL on
        # another host, which must not get the token (and refuses a request carrying one).
        req.add_unredirected_header("Authorization", "Bearer " + token)
        with urllib.request.urlopen(req, timeout=60) as r:
            data = r.read(64 * 1024 * 1024)
        return data if raw else (json.loads(data) if data else {})

    def dispatch(self, fetch_id, digest):
        self._req("POST", "/repos/%s/actions/workflows/%s/dispatches" % (REPO, WORKFLOW),
                  {"ref": REF, "inputs": {"fetch_id": fetch_id, "journal_digest": digest, "image_id": IMAGE}})

    def find_run(self, fetch_id):
        v = self._req("GET", "/repos/%s/actions/workflows/%s/runs?event=workflow_dispatch&per_page=20" % (REPO, WORKFLOW))
        for run in v.get("workflow_runs", []):
            if run.get("display_title") == "wrap-private " + fetch_id:
                return {"id": run["id"], "status": run.get("status"), "conclusion": run.get("conclusion")}
        return None

    def seal(self, run_id, fetch_id):
        v = self._req("GET", "/repos/%s/actions/runs/%d/artifacts" % (REPO, run_id))
        art = next((a for a in v.get("artifacts", []) if a.get("name") == "wrap-private-" + fetch_id), None)
        if not art:
            return None
        z = zipfile.ZipFile(io.BytesIO(self._req("GET", art["archive_download_url"], raw=True)))
        return z.read("seal.hex").decode().strip()


def run_prover(secret_hex, args, out):
    """The installed release prover, the secret on stdin. (rc, tail of its output)"""
    env = dict(os.environ, COEXIST="1", MIN_FREE=E("PROVER_MIN_FREE", "30"))
    try:
        r = subprocess.run([PROVER] + args, input=secret_hex.encode(), env=env, capture_output=True, timeout=600)
    except subprocess.TimeoutExpired:
        return 1, "timed out"
    return r.returncode, (r.stdout + r.stderr).decode("utf-8", "replace")[-300:]


def verify_receipt(job_dir):
    r = subprocess.run([HOST, "verify", "--receipt", job_dir + "/receipt.bin", "--image-id", IMAGE, "--journal", job_dir + "/journal.hex"],
                       capture_output=True, timeout=300)
    return r.returncode == 0


def cast_release(vault, args):
    """`cast send <vault> release(...)` from the settle keystore. (ok, tx, block)"""
    cmd = [CAST, "send", vault, RELEASE_SIG] + args + ["--keystore", KEYSTORE, "--password-file", KEYSTORE_PW,
                                                       "--rpc-url", SEPOLIAS[0], "--json"]
    r = subprocess.run(cmd, capture_output=True, timeout=300)
    try:
        v = json.loads(r.stdout.decode())
        return v.get("status") in ("0x1", 1, "1"), v.get("transactionHash"), int(str(v.get("blockNumber", "0")), 0)
    except (ValueError, AttributeError):
        return False, None, None


class World:
    def __init__(self):
        self.eth, self.gh = Sepolia(), GitHub()
        self.prover, self.verify, self.cast = run_prover, verify_receipt, cast_release
        self.clock = time.time


W = World()


# ---------------------------------------------------------------- state, log

def load_state():
    try:
        with open(STATE) as f:
            s = json.load(f)
    except FileNotFoundError:
        s = {}
    s.setdefault("jobs", {})
    s.setdefault("accepted", [])
    s.setdefault("clients", {})
    s.setdefault("daykeys", {})
    return s


def save_state(s):
    with LOCK:
        _save_state(s)


def _save_state(s):
    os.makedirs(DIR, exist_ok=True)
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


def client_tags(s, key, now):
    out = []
    for d in (time.strftime("%Y-%m-%d", time.gmtime(now)), time.strftime("%Y-%m-%d", time.gmtime(now - 86400))):
        k = s["daykeys"].get(d)
        if k is None and d == time.strftime("%Y-%m-%d", time.gmtime(now)):
            k = s["daykeys"][d] = secrets.token_hex(32)
        if k:
            out.append(hmac.new(bytes.fromhex(k), key.encode(), hashlib.sha256).hexdigest()[:20])
    return out


def prune(s, now):
    cut = now - 86400
    s["accepted"] = [t for t in s["accepted"] if t > cut]
    for t in list(s["clients"]):
        s["clients"][t] = [x for x in s["clients"][t] if x > cut]
        if not s["clients"][t]:
            del s["clients"][t]
    keep = {time.strftime("%Y-%m-%d", time.gmtime(now)), time.strftime("%Y-%m-%d", time.gmtime(cut))}
    for d in list(s["daykeys"]):
        if d not in keep:
            del s["daykeys"][d]


# ---------------------------------------------------------------- chain reads

def word(b):
    return "0x" + b.hex()


def vault_call(vault, sel, *words):
    return W.eth.call(vault, "0x" + SEL[sel] + "".join(w.hex() for w in words))


def recorded(vault, root):
    return int.from_bytes(vault_call(vault, "exitRootTime", root)[:32], "big") != 0


def spent(vault, nf):
    return int.from_bytes(vault_call(vault, "spent", nf)[:32], "big") != 0


def denom_index(vault, token, denom):
    out = vault_call(vault, "tokenConfig", b"\0" * 12 + token)
    w = [out[i * 32:(i + 1) * 32] for i in range(len(out) // 32)]
    at = int.from_bytes(w[5], "big") // 32
    n = int.from_bytes(w[at], "big")
    denoms = [int.from_bytes(w[at + 1 + i], "big") for i in range(n)]
    return denoms.index(denom)


# ---------------------------------------------------------------- the calls

def info():
    out = {"available": True, "reason": None, "fee": "0", "vaults": VAULTS}
    missing = [n for n, f in (("the wrap key", WRAP_KEY_FILE), ("the GitHub token", TOKEN_FILE), ("the settle key", KEYSTORE)) if not os.path.exists(f)]
    if missing:
        return dict(out, available=False, reason="The bridge's settlement service is not set up yet (%s)." % ", ".join(missing))
    try:
        if ADDRESS and W.eth.balance(ADDRESS) < FLOOR:
            return dict(out, available=False, reason="The bridge's settlement service is out of Sepolia ETH for gas; the operator refills it.")
    except Exception:  # noqa: BLE001
        return dict(out, available=False, reason="The settlement service can't read Sepolia right now; try again shortly.")
    return out


def accept(body, ip, s, now=None):
    """Checks a request against Sepolia and queues it. The secret stays in memory (SECRETS)."""
    now = now or W.clock()
    try:
        req = json.loads(body)
        vault = str(req["vault"]).lower()
    except (ValueError, KeyError, TypeError):
        return refuse("bad_request", "expected JSON {vault, receipt_hex, journal_hex} or {vault, exit}")
    if vault not in VAULTS:
        return refuse("bad_request", "That vault is not one this service settles for.")
    exit_ = req.get("exit")
    try:
        cfg = vault_call(vault, "cfgId")[:32]
        if exit_ is not None:
            k = h2b(exit_["secret"], 32)
            dest, token = h2b(exit_["eth_dest"], 20), h2b(exit_["token"], 20)
            denom, max_fee = int(exit_["denom"]), int(exit_["max_relay_fee"])
            if h2b(exit_["cfg_id"], 32) != cfg:
                return refuse("bad_request", "This exit is for another bridge than this vault's.")
            leaves = [h2b(x, 32) for x in exit_["leaves"]]
            index = int(exit_["index"])
            if not (0 <= index < len(leaves)) or len(leaves) > 1 << 20:
                return refuse("bad_request", "The leaf index is outside the leaves given.")
            if leaves[index] != exit_leaf(cfg, token, denom, exit_commitment(k, dest, token, denom, max_fee, cfg)):
                return refuse("bad_request", "The exit secret and the leaves don't make this exit's leaf.")
            root, nf = tree_root(leaves), nullifier(k)
            job = {"kind": "exit", "root": word(root), "token": word(token), "denom": str(denom), "recipient": word(dest),
                   "relayer": ZERO20, "relay_fee": "0", "max_relay_fee": str(max_fee), "cfg": word(cfg), "index": index,
                   "leaves": [word(x) for x in leaves]}
        else:
            receipt, journal = h2b(req["receipt_hex"]), h2b(req["journal_hex"], 256)
            if not 1000 < len(receipt) < 400 * 1024:
                return refuse("bad_request", "That is not a release receipt.")
            j = decode_journal(journal)
            if j["cfg"] != cfg:
                return refuse("bad_request", "This proof is for another bridge than this vault's.")
            root, nf = j["root"], j["nullifier"]
            job = {"kind": "receipt", "root": word(root), "token": word(j["token"]), "denom": str(j["denom"]),
                   "recipient": word(j["recipient"]), "relayer": word(j["relayer"]), "relay_fee": str(j["relay_fee"])}
    except (KeyError, TypeError, ValueError) as e:
        return refuse("bad_request", "The request is malformed (%s)." % str(e)[:80])
    except RuntimeError:
        return refuse("busy", "Sepolia can't be read right now; try again shortly.", retry=True)
    jid = word(nf)
    with LOCK:
        prune(s, now)
        old = s["jobs"].get(jid)
        if old and old["stage"] != "failed":
            return {"ok": True, "id": jid, "stage": old["stage"]}
        try:
            if spent(vault, nf):
                return refuse("released", "This exit was released already.")
            if not recorded(vault, root):
                return refuse("not_recorded", "The bridge's update hasn't recorded this burn yet; try again after the next update.", retry=True)
        except RuntimeError:
            return refuse("busy", "Sepolia can't be read right now; try again shortly.", retry=True)
        tags = client_tags(s, client_key(ip), now)
        if sum(len([x for x in s["clients"].get(t, []) if x > now - 86400]) for t in tags) >= PER_CLIENT_DAY:
            return refuse("rate_limited", "Too many settlements from this address today; try again tomorrow.")
        if len(s["accepted"]) >= DAILY_MAX:
            return refuse("rate_limited", "Today's settlements are used up; try again tomorrow, or make the proof yourself.")
        d = os.path.join(JOBS, jid[2:18])
        os.makedirs(d, mode=0o700, exist_ok=True)
        if exit_ is None:
            with open(os.path.join(d, "receipt.bin"), "wb") as f:
                f.write(receipt)
            with open(os.path.join(d, "journal.hex"), "w") as f:
                f.write(word(journal))
            if not W.verify(d):
                for n in ("receipt.bin", "journal.hex"):
                    try:
                        os.remove(os.path.join(d, n))
                    except OSError:
                        pass
                return refuse("bad_request", "The receipt does not verify against the release image.")
        else:
            SECRETS[jid] = k.hex()
        job.update({"vault": vault, "stage": "queued", "at": now, "dir": d, "nullifier": jid})
        s["jobs"][jid] = job
        s["accepted"].append(now)
        s["clients"].setdefault(tags[0], []).append(now)
        save_state(s)
    log("accepted", id=jid, kind=job["kind"])
    return {"ok": True, "id": jid, "stage": "queued"}


def status(body, s):
    try:
        jid = str(json.loads(body)["id"]).lower()
    except (ValueError, KeyError, TypeError):
        return refuse("bad_request", "expected JSON {id}")
    j = s["jobs"].get(jid)
    if not j:
        return refuse("unknown", "This service has no settlement with that id.")
    out = {"ok": True, "id": jid, "stage": j["stage"], "text": j.get("text") or TEXT[j["stage"]]}
    for k in ("seal", "tx", "block", "releasable_at", "error", "run"):
        if j.get(k) is not None:
            out[k] = j[k]
    return out


def bundle(body, s, now=None):
    now = now or W.clock()
    try:
        fid = str(json.loads(body)["fetch_id"])
    except (ValueError, KeyError, TypeError):
        return refuse("bad_request", "expected JSON {fetch_id}")
    if not re.fullmatch(r"[0-9a-f]{32}", fid):
        return refuse("bad_request", "not a fetch id")
    with LOCK:
        jobs = list(s["jobs"].values())
    for j in jobs:
        if j.get("fetch_id") == fid and j["stage"] == "wrapping" and now - j.get("dispatched", 0) < BUNDLE_TTL:
            p = os.path.join(j["dir"], "bundle.bin")
            if os.path.exists(p):
                with open(p, "rb") as f:
                    return {"ok": True, "bundle": base64.b64encode(f.read()).decode()}
    p = os.path.join(UPDATER_BUNDLES, fid + ".bin")      # fid is 32 hex characters (checked above): no path tricks
    try:
        if now - os.stat(p).st_mtime < BUNDLE_TTL:
            with open(p, "rb") as f:
                return {"ok": True, "bundle": base64.b64encode(f.read()).decode()}
    except OSError:
        pass
    return refuse("unknown", "no such bundle (or no longer served)")


# ---------------------------------------------------------------- the worker: one job per stage at a time

def set_stage(s, j, stage, text=None, **kw):
    with LOCK:
        j["stage"], j["text"] = stage, text
        j.update(kw)
        _save_state(s)
    log("stage", id=j["nullifier"], stage=stage, **{k: v for k, v in kw.items() if k in ("run", "tx", "block", "error")})


def fail(s, j, why):
    SECRETS.pop(j["nullifier"], None)
    set_stage(s, j, "failed", why, error=why)


def make_bundle(j):
    """The encrypted bundle the wrap runner fetches (as wrap-private.sh): (fetch id, journal digest)."""
    d = j["dir"]
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as t:
        for n in ("receipt.bin", "journal.hex"):
            t.add(os.path.join(d, n), arcname=n)
    with open(WRAP_KEY_FILE) as f:
        key = f.read().strip()
    r = subprocess.run([OPENSSL, "enc", "-aes-256-cbc", "-pbkdf2", "-iter", "200000", "-md", "sha256", "-salt", "-pass", "env:WRAP_KEY"],
                       input=buf.getvalue(), capture_output=True, env=dict(os.environ, WRAP_KEY=key), timeout=60)
    if r.returncode != 0:
        raise RuntimeError("openssl failed")
    fd = os.open(os.path.join(d, "bundle.bin"), os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "wb") as f:
        f.write(r.stdout)
    with open(os.path.join(d, "journal.hex")) as f:
        digest = word(sha(h2b(f.read().strip())))
    return secrets.token_hex(16), digest


def step(s):
    """One pass: each stage moves at most one job. Returns True when something changed."""
    now = W.clock()
    with LOCK:
        jobs = sorted(s["jobs"].values(), key=lambda j: j["at"])
    moved = False
    # proving
    j = next((x for x in jobs if x["stage"] in ("queued",) and x["kind"] == "exit"), None)
    if j and not any(x["stage"] == "proving" for x in jobs):
        k = SECRETS.get(j["nullifier"])
        if k is None:
            fail(s, j, "The service restarted before making the proof; send the settlement again.")
        elif not DRY:
            set_stage(s, j, "proving")
            args = ["release", "--secret-file", "/dev/stdin", "--eth-dest", j["recipient"], "--token", j["token"], "--denom", j["denom"],
                    "--max-relay-fee", j["max_relay_fee"], "--cfg-id", j["cfg"], "--leaves", os.path.join(j["dir"], "leaves.txt"),
                    "--index", str(j["index"]), "--relayer", j["relayer"], "--relay-fee", j["relay_fee"], "--out", j["dir"], "--image-id", IMAGE, "--prove"]
            with open(os.path.join(j["dir"], "leaves.txt"), "w") as f:
                f.write("\n".join(j["leaves"]) + "\n")
            rc, tail = W.prover(k, args, j["dir"])
            if rc == 75:
                set_stage(s, j, "queued", "Waiting for memory on the bridge's machine (another proof is running); it starts by itself.")
            elif rc != 0 or not os.path.exists(os.path.join(j["dir"], "receipt.bin")):
                fail(s, j, "The release proof failed on the bridge's machine.")
                log("prove-failed", id=j["nullifier"], rc=rc)
            elif not W.verify(j["dir"]):
                fail(s, j, "The release proof did not verify.")
            else:
                SECRETS.pop(j["nullifier"], None)
                set_stage(s, j, "queued", "Proof made; waiting for its Ethereum wrapper.", kind="receipt")
            moved = True
    # wrapping
    w = next((x for x in jobs if x["stage"] == "wrapping"), None)
    if w is None:
        j = next((x for x in jobs if x["stage"] == "queued" and x["kind"] == "receipt"), None)
        if j and not DRY:
            try:
                fid, digest = make_bundle(j)
                set_stage(s, j, "wrapping", fetch_id=fid, digest=digest, dispatched=now)
                W.gh.dispatch(fid, digest)
            except Exception as e:  # noqa: BLE001
                set_stage(s, j, "queued", "Waiting for the wrapper service (%s); it is asked again shortly." % type(e).__name__)
            moved = True
    else:
        try:
            run = W.gh.find_run(w["fetch_id"])
        except Exception:  # noqa: BLE001
            run = None
        if run and w.get("run") != run["id"]:
            with LOCK:
                w["run"] = run["id"]
        seal_tries = w.get("seal_tries", 0)
        if run and run.get("status") == "completed" and run.get("conclusion") == "success":
            try:
                seal = W.gh.seal(run["id"], w["fetch_id"])
            except Exception as e:  # noqa: BLE001
                seal = None
                if seal_tries + 1 < 3:
                    with LOCK:
                        w["seal_tries"] = seal_tries + 1
                        _save_state(s)
                    log("seal-fetch", id=w["nullifier"], run=run["id"], error=type(e).__name__)
                    run = None          # asked again next tick
        if run and run.get("status") == "completed":
            seal = seal if run.get("conclusion") == "success" else None
            try:
                os.remove(os.path.join(w["dir"], "bundle.bin"))
            except OSError:
                pass
            ok = False
            if seal and re.fullmatch(r"0x[0-9a-f]{520}", seal):
                try:
                    W.eth.call(VERIFIER, word(verify_calldata(h2b(seal), h2b(IMAGE), h2b(w["digest"]))))
                    ok = True
                except RuntimeError:
                    ok = False
            if ok:
                set_stage(s, w, "releasable_at", seal=seal, fetch_id=None)
            else:
                fail(s, w, "The Ethereum wrapper failed (run %s)." % run["id"])
            moved = True
        elif now - w.get("dispatched", now) > WRAP_TIMEOUT:
            fail(s, w, "The Ethereum wrapper took too long.")
            moved = True
    # releasable_at
    for j in [x for x in jobs if x["stage"] == "releasable_at"]:
        try:
            if spent(j["vault"], h2b(j["nullifier"])):
                set_stage(s, j, "released", "Released on Ethereum (by someone else).")
                continue
            at = int.from_bytes(vault_call(j["vault"], "releasableAt", b"\0" * 12 + h2b(j["token"]), h2b(j["root"]))[:32], "big")
            if j.get("releasable_at") != at:
                with LOCK:
                    j["releasable_at"] = at
                    _save_state(s)
            if at and W.eth.now() >= at and not any(x["stage"] == "releasing" for x in jobs):
                set_stage(s, j, "releasing")
                moved = True
                break
        except RuntimeError:
            continue
    # releasing
    j = next((x for x in jobs if x["stage"] == "releasing"), None)
    if j and not DRY:
        try:
            if spent(j["vault"], h2b(j["nullifier"])):    # sent before a restart, or by someone else
                set_stage(s, j, "released", "Released on Ethereum.")
                return True
            if ADDRESS and W.eth.balance(ADDRESS) < FLOOR:
                set_stage(s, j, "releasable_at", "The service is out of Sepolia ETH for gas; the release waits for a refill.")
                return True
            idx = denom_index(j["vault"], h2b(j["token"]), int(j["denom"]))
        except (RuntimeError, ValueError):
            return moved
        ok, tx, blk = W.cast(j["vault"], [j["seal"], j["root"], j["nullifier"], j["token"], str(idx), j["recipient"], j["relayer"], j["relay_fee"]])
        if ok:
            set_stage(s, j, "released", tx=tx, block=blk)
        elif tx:
            fail(s, j, "The release was sent but refused (tx %s)." % tx)
        else:
            set_stage(s, j, "releasable_at", "The release could not be sent just now; it is tried again.")
        moved = True
    return moved


def worker():
    while True:
        try:
            step(S)
        except Exception as e:  # noqa: BLE001
            log("error", error=type(e).__name__)
        time.sleep(TICK)


# ---------------------------------------------------------------- HTTP

def load_auth():
    if not AUTH_FILE:
        return None
    if os.stat(AUTH_FILE).st_mode & 0o077:
        sys.exit("SETTLE_AUTH_FILE must be mode 600")
    with open(AUTH_FILE) as f:
        return f.read().strip() or None


class Handler(http.server.BaseHTTPRequestHandler):
    server_version = "lez-demo-settle/1"
    sys_version = ""

    def log_message(self, fmt, *args):
        pass

    def client_ip(self):
        given = self.headers.get(AUTH_HEADER, "")
        if AUTH and given and hmac.compare_digest(given.encode(), AUTH.encode()):
            try:
                return str(ipaddress.ip_address(self.headers.get("CF-Connecting-IP", "").strip()))
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
        if self.path.rstrip("/") != "/settle/info":
            return self.reply(refuse("unknown", "not found"))
        self.reply(info())

    def do_POST(self):
        path = self.path.rstrip("/")
        try:
            n = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            n = -1
        if n <= 0 or n > MAX_BODY:
            return self.reply(refuse("bad_request", "request too large or empty"))
        body = self.rfile.read(n).decode("utf-8", "replace")
        try:
            if path == "/settle":
                if not info()["available"]:
                    return self.reply(refuse("unavailable", info()["reason"], retry=True))
                out = accept(body, self.client_ip(), S)
            elif path == "/settle/status":
                out = status(body, S)
            elif path == "/settle/bundle":
                out = bundle(body, S)
            else:
                out = refuse("unknown", "not found")
        except Exception as e:  # noqa: BLE001
            log("error", error=type(e).__name__)
            out = refuse("busy", "settlement error; try again later", retry=True)
        self.reply(out)


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def main():
    global DRY, AUTH, S
    if "--dry-run" in sys.argv:
        DRY = True
    host, port = LISTEN.rsplit(":", 1)
    if host not in ("127.0.0.1", "::1", "localhost"):
        sys.exit("SETTLE_LISTEN must be loopback")
    os.makedirs(JOBS, mode=0o700, exist_ok=True)
    AUTH = load_auth()
    S = s = load_state()
    for j in s["jobs"].values():
        if j["stage"] == "proving":     # stopped mid-proof: its secret is gone with the old process
            j["stage"] = "queued"
    save_state(s)
    log("start", listen=LISTEN, vaults=",".join(VAULTS), repo=REPO, address=ADDRESS or "-", per_client=PER_CLIENT_DAY,
        daily_max=DAILY_MAX, proxy_auth=bool(AUTH), dry_run=DRY)
    threading.Thread(target=worker, daemon=True).start()
    Server((host, int(port)), Handler).serve_forever()


if __name__ == "__main__":
    main()
