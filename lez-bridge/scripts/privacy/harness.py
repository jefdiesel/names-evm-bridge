#!/usr/bin/python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Privacy and recovery measurements for the RFP-022 bridge on THROWAWAY local chains.

Build host only. A loopback LEZ v0.4.0-rc3 sequencer and a local anvil, fresh keys only, nothing
proved: the sequencer and every wallet run with RISC0_DEV_MODE=1, so a private transaction's
receipt is a development receipt (risc0 FakeReceipt) while everything the block publishes (the
privacy circuit's journal: boundary messages, nullifiers, commitments, one-time addresses, fees)
is the real executor output. Outbound updates and releases are executed, not proved, and accepted
by the vault's test MockVerifier (as in e2e-roundtrip.sh). See README.md beside this file.

  harness.py --run DIR [--n 20] [--seed S] <phase> ...

Phases (each resumable; state in DIR/state.json, ground truth in DIR/truth/, adversary input in
DIR/capture/):
  chain     set up and start the LEZ chain (RISC0_DEV_MODE=1) and anvil
  deploy    programs, vault (MockVerifier), wrapped tokens, init, register, sponsor, float
  users     the cohorts' wallets (seed phrases kept in DIR/secret, mode 700) and Ethereum keys
  deposits  every user's deposit, in random order with random gaps
  attest    the stand-in attestor records the vault root
  claims    every claim, random order and gaps
  burns     every burn, random order and gaps
  update    one G2-lite update over every block so far (executor), submitted to the vault
  releases  every release, random order and gaps (sent by the relayer, or the control's sender)
  capture   everything public on both chains into DIR/capture/
  stop      stop the chain and anvil
  scrub     after stop: delete secret/, wallets/, chain/, g2/, out/; keep the capture, truth and results
  all       chain .. capture
"""
import argparse, json, os, random, re, secrets, shutil, signal, subprocess, sys, time, urllib.request

# PT_HOME: the work directory holding bin/ and src/ (default: four levels above this file, i.e. this
# file at <PT_HOME>/src/lez-bridge/scripts/privacy/harness.py)
W = os.environ.get("PT_HOME") or os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "../../../.."))
BIN = os.path.join(W, "bin")
CLI = os.path.join(BIN, "lez-bridge")
HOST = os.path.join(BIN, "outbound_host")
SEQ_BIN = os.path.join(BIN, "sequencer_service")
NETCFG = os.path.join(BIN, "local.json")
GUARD = os.path.join(W, "src/lez-bridge/scripts/guard.sh")
# the rc3 chain setup script (setup/start/stop of a loopback sequencer); PT_CHAIN_SH overrides it
CHAIN_SH = os.environ.get("PT_CHAIN_SH") or os.path.join(W, "src", "public-demo", "rc3", "chain-rc3.sh")
VAULT_DIR = os.path.join(W, "src/vault")
FOUNDRY = os.path.expanduser("~/.foundry/bin")
RPC_HOSTPORT = os.environ.get("PT_RPC", "127.0.0.1:3480")
METRICS = os.environ.get("PT_METRICS", "127.0.0.1:3481")
ANVIL_PORT = int(os.environ.get("PT_ANVIL_PORT", "8580"))
SEQ = "http://" + RPC_HOSTPORT
ETH_RPC = "http://127.0.0.1:%d" % ANVIL_PORT
for p in (RPC_HOSTPORT, METRICS):
    assert int(p.rsplit(":", 1)[1]) in range(3480, 3486), "ports 3480-3485 only"
assert ANVIL_PORT == 8580 or os.environ.get("PT_ALLOW_PORT") == "1"
ANVIL_KEY0 = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"  # anvil's public test key 0
ETH = "0x0000000000000000000000000000000000000000"
ETH_D = [10 ** 16, 10 ** 17]
ETH_DF = [10 ** 14, 10 ** 15]
USDC_D = [100_000_000, 1_000_000_000]
SPONSOR = 50_000_000
LABEL = "dev receipts (RISC0_DEV_MODE=1): public outputs real, receipts not proofs; updates and releases executed, mock-verified"

# Cohorts: name -> (eth token key, denom index, behaviour). Each cohort has its own (token, denom),
# so the anonymity sets are separate and a control never shrinks the main set.
COHORTS = {
    "main": ("ETH", 0, "private claim (one-time address, sponsored), private burn, release by the relayer"),
    "c1_public": ("USDC", 0, "CONTROL: claim and burn from a PUBLIC LEZ account"),
    "c2_addrsecret": ("USDC", 1, "CONTROL: deposit secret = the depositor's Ethereum address (padded); claim names its account (--reveal-recipient)"),
    "c3_fee": ("ETH", 1, "CONTROL: a per-user fee value on both chains (Ethereum priority fee of the deposit and of the relayed release; the same number as the LEZ claim's and burn's --gas-limit)"),
    "c4_self": ("ETH", 1, "CONTROL: the release is sent by the user's own depositing Ethereum account"),
}


def log(*a):
    print(time.strftime("%H:%M:%S"), *a, flush=True)


class Run:
    def __init__(self, d, n, seed, controls):
        self.d = os.path.abspath(d)
        os.makedirs(self.d, exist_ok=True)
        for sub in ("wallets", "secret", "truth", "capture", "out", "logs"):
            os.makedirs(os.path.join(self.d, sub), exist_ok=True)
        for sub in ("wallets", "secret"):
            os.chmod(os.path.join(self.d, sub), 0o700)
        self.sp = os.path.join(self.d, "state.json")
        self.s = json.load(open(self.sp)) if os.path.exists(self.sp) else {"steps": [], "n": n, "seed": seed, "controls": controls}
        self.rng = random.Random(self.s["seed"] * 7919 + len(self.s["steps"]))

    def save(self):
        tmp = self.sp + ".tmp"
        json.dump(self.s, open(tmp, "w"), indent=1)
        os.replace(tmp, self.sp)

    def done(self, step):
        return step in self.s["steps"]

    def mark(self, step):
        self.s["steps"].append(step)
        self.save()
        log("STEP DONE", step)

    def truth(self, name, rec):
        with open(os.path.join(self.d, "truth", name + ".jsonl"), "a") as f:
            f.write(json.dumps(rec) + "\n")

    # ------------------------------------------------------------------ process helpers
    def env(self, home=None, dev=True):
        e = dict(os.environ)
        e["PATH"] = FOUNDRY + ":" + e.get("PATH", "")
        e["LEZ_BRIDGE_SEQUENCER"] = SEQ
        e["LEZ_BRIDGE_ETH_RPC"] = ETH_RPC
        e["LEE_WALLET_PASSWORD"] = ""
        e["TMPDIR"] = os.path.join(W, "tmp")
        for k in ("RISC0_PROVER", "RISC0_SERVER_PATH", "BONSAI_API_URL", "BONSAI_API_KEY"):
            e.pop(k, None)
        if dev:
            e["RISC0_DEV_MODE"] = "1"
        if home:
            e["LEE_WALLET_HOME_DIR"] = home
        return e

    def run(self, args, home=None, name=None, check=True, guard=False, timeout=900, env_extra=None, dev=True):
        """Run a command; output to out/<name>.out. guard=True: under guard.sh with a 3 GB footprint
        cap (a real proof needs ~9.5 GB, so a proof that started by mistake is killed) and a kill
        after `timeout` seconds."""
        env = self.env(home, dev)
        if env_extra:
            env.update(env_extra)
        cmd = [str(x) for x in args]
        if guard:
            env["MAX_FOOTPRINT_GB"] = "3"
            env["GUARD_LOG"] = os.path.join(self.d, "logs", "guard.log")
            cmd = [GUARD] + cmd
        out = os.path.join(self.d, "out", (name or os.path.basename(cmd[0])) + ".out")
        t0 = time.time()
        with open(out, "w") as f:
            p = subprocess.Popen(cmd, stdout=f, stderr=subprocess.STDOUT, env=env, start_new_session=True)
            try:
                rc = p.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                os.killpg(p.pid, signal.SIGKILL)
                p.wait()
                rc = -9
        text = open(out).read()
        dt = time.time() - t0
        if check and rc != 0:
            tail = "\n".join(l for l in text.splitlines() if not l.startswith("Transaction data"))[-1500:]
            raise SystemExit("FAILED (rc %s, %.0fs): %s\n%s" % (rc, dt, name or cmd[:3], tail))
        return text, rc, dt

    def cli(self, home, *a, name=None, check=True, guard=False, timeout=900, env_extra=None):
        return self.run([CLI] + [str(x) for x in a], home=home, name=name, check=check, guard=guard, timeout=timeout, env_extra=env_extra)[0]

    def cast(self, *a, check=True):
        rpc = [] if a and a[0] in ("wallet", "keccak", "abi-encode", "sig") else ["--rpc-url", ETH_RPC]
        p = subprocess.run([os.path.join(FOUNDRY, "cast")] + [str(x) for x in a] + rpc,
                           capture_output=True, text=True, env=self.env(dev=False))
        if check and p.returncode != 0:
            raise SystemExit("cast %s failed: %s %s" % (a[:2], p.stderr[-800:], p.stdout[-800:]))
        return p.stdout.strip()

    def esend(self, key, *a, extra=()):
        out = self.cast("send", *a, "--private-key", key, "--json", *extra)
        return json.loads(out)

    def lez(self, method, params=None):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or []}).encode()
        req = urllib.request.Request(SEQ, body, {"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.load(r).get("result")

    def eth(self, method, params=None):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or []}).encode()
        req = urllib.request.Request(ETH_RPC, body, {"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=60) as r:
            j = json.load(r)
        if "error" in j:
            raise RuntimeError("%s: %s" % (method, j["error"]))
        return j["result"]

    def pause(self, lo, hi):
        t = self.rng.uniform(lo, hi)
        time.sleep(t)

    def wallet(self, name):
        h = os.path.join(self.d, "wallets", name)
        os.makedirs(h, exist_ok=True)
        os.chmod(h, 0o700)
        return h


def grab(pat, text, what):
    m = re.search(pat, text, re.M)
    if not m:
        raise SystemExit("no %s in output:\n%s" % (what, text[-1500:]))
    return m.group(1)


def block_of(text):
    m = re.findall(r"(?:applied|included) in block (\d+)", text)
    return int(m[-1]) if m else None


def acct(kind, text):
    return grab(r"%s/([1-9A-HJ-NP-Za-km-z]{32,44})" % kind, text, kind + " account")


# ---------------------------------------------------------------------- phases
def p_chain(r):
    if r.done("chain"):
        return start_chain(r)
    env = r.env(dev=True)
    env.update({"R": os.path.join(r.d, "chain"), "SEQ_BIN": SEQ_BIN, "CLI": CLI, "NETCFG": NETCFG, "RPC": RPC_HOSTPORT, "METRICS": METRICS})
    subprocess.run([CHAIN_SH, "setup"], env=env, check=True)
    r.mark("chain")
    start_chain(r)


def start_chain(r):
    env = r.env(dev=True)
    env.update({"R": os.path.join(r.d, "chain"), "SEQ_BIN": SEQ_BIN, "CLI": CLI, "NETCFG": NETCFG, "RPC": RPC_HOSTPORT, "METRICS": METRICS,
                "RUST_LOG": "warn"})
    subprocess.run([CHAIN_SH, "start"], env=env, check=True)
    # the sequencer must verify in dev mode: check its environment
    pid = open(os.path.join(r.d, "chain", "sequencer.pid")).read().strip()
    e = subprocess.run(["ps", "eww", "-p", pid], capture_output=True, text=True).stdout
    assert "RISC0_DEV_MODE=1" in e, "sequencer not in dev mode"
    log("sequencer pid", pid, "runs with RISC0_DEV_MODE=1")
    if subprocess.run([os.path.join(FOUNDRY, "cast"), "chain-id", "--rpc-url", ETH_RPC], capture_output=True).returncode != 0:
        lf = open(os.path.join(r.d, "logs", "anvil.log"), "a")
        p = subprocess.Popen([os.path.join(FOUNDRY, "anvil"), "--port", str(ANVIL_PORT), "--host", "127.0.0.1", "--silent",
                              "--accounts", "10", "--balance", "1000000"], stdout=lf, stderr=lf, start_new_session=True)
        open(os.path.join(r.d, "anvil.pid"), "w").write(str(p.pid))
        for _ in range(30):
            time.sleep(1)
            if subprocess.run([os.path.join(FOUNDRY, "cast"), "chain-id", "--rpc-url", ETH_RPC], capture_output=True).returncode == 0:
                break
    log("anvil on", ETH_RPC, "chain id", r.cast("chain-id"))


def p_stop(r):
    env = r.env()
    env.update({"R": os.path.join(r.d, "chain"), "SEQ_BIN": SEQ_BIN, "CLI": CLI, "NETCFG": NETCFG, "RPC": RPC_HOSTPORT, "METRICS": METRICS})
    subprocess.run([CHAIN_SH, "stop"], env=env)
    pf = os.path.join(r.d, "anvil.pid")
    if os.path.exists(pf):
        try:
            os.kill(int(open(pf).read()), signal.SIGTERM)
        except ProcessLookupError:
            pass
        os.remove(pf)
    log("stopped")


def p_deploy(r):
    s = r.s
    if r.done("deploy"):
        return

    def once(key, fn):  # resumable: every value is saved as soon as it exists
        if key not in s:
            s[key] = fn()
            r.save()
        return s[key]

    opw = os.path.join(r.d, "chain", "wallets", "operator")
    op = once("OP", lambda: open(os.path.join(r.d, "chain", "operator.env")).read().split("=", 1)[1].strip())
    dh = r.wallet("deployer")

    def mkdeployer():
        r.cli(dh, "wallet", "check-health", name="deployer-health", check=False)
        t = r.cli(dh, "wallet", "account", "new", "public", name="deployer-new")
        pk = grab(r"^With pk ([0-9a-f]{64})", t, "pk")
        r.cli(opw, "wallet", "auth-transfer", "send", "--from", "Public/" + op, "--to-pk", pk, "--amount", 10 ** 13, name="fund-deployer")
        return acct("Public", t)
    once("D", mkdeployer)
    once("START", lambda: int(r.run([HOST, "last", "--rpc", SEQ], name="host-last", dev=False)[0].strip()))
    once("TP", lambda: grab(r"^program (\S+)", r.cli(dh, "deploy", "--which", "token", "--payer", s["D"], name="deploy-token"), "token program"))
    once("BP", lambda: grab(r"^program (\S+)", r.cli(dh, "deploy", "--which", "bridge", "--payer", s["D"], name="deploy-bridge"), "bridge program"))
    once("PROGRAM_ID", lambda: grab(r"^PROGRAM_ID=(\S+)", r.cli(dh, "program-id", "--bridge", s["BP"], name="program-id"), "program id"))
    once("LEZ_CHAIN_ID", lambda: grab(r"^LEZ_CHAIN_ID=(\S+)", r.cli(dh, "chain-id", name="chain-id"), "chain id"))
    once("ZONE_ID", lambda: grab(r"^ZONE_ID=(\S+)", r.cli(dh, "source-id", name="source-id"), "zone id"))
    os.makedirs(os.path.join(r.d, "g2", "blocks"), exist_ok=True)
    once("G2_STATE0", lambda: grab(r"^G2_STATE=(\S+)", r.run([HOST, "genesis", "--rpc", SEQ, "--dir", os.path.join(r.d, "g2", "blocks"), "--block", s["START"],
                                                               "--bridge", s["BP"], "--out", os.path.join(r.d, "g2", "state0.bin")], name="g2-genesis", dev=False)[0], "g2 state"))
    t = r.run([HOST, "ids"], name="host-ids", dev=False)[0]
    once("UPDATE_IMAGE_ID", lambda: grab(r"^UPDATE_IMAGE_ID_TRUSTED_TIP=(\S+)", t, "update image"))
    once("RELEASE_IMAGE_ID", lambda: grab(r"^RELEASE_IMAGE_ID=(\S+)", t, "release image"))
    if "VAULT" not in s:
        env = r.env(dev=False)
        env.update({"PROGRAM_ID": s["PROGRAM_ID"], "LEZ_CHAIN_ID": s["LEZ_CHAIN_ID"], "ZONE_ID": s["ZONE_ID"], "UPDATE_IMAGE_ID": s["UPDATE_IMAGE_ID"],
                    "RELEASE_IMAGE_ID": s["RELEASE_IMAGE_ID"], "G2_STATE": s["G2_STATE0"], "RELEASE_DELAY": "5"})
        p = subprocess.run([os.path.join(FOUNDRY, "forge"), "script", "script/RoundTripDeploy.s.sol", "--rpc-url", ETH_RPC,
                            "--private-key", ANVIL_KEY0, "--broadcast"], cwd=VAULT_DIR, env=env, capture_output=True, text=True)
        open(os.path.join(r.d, "out", "vault-deploy.out"), "w").write(p.stdout + p.stderr)
        if p.returncode != 0:
            raise SystemExit("vault deploy failed:\n" + (p.stdout + p.stderr)[-2000:])
        for k in ("USDC", "CFG_ID", "VAULT"):
            s[k] = grab(r"%s=(0x[0-9a-fA-F]+)" % k, p.stdout, k)
        r.save()
    s["EVM"] = int(r.cast("chain-id"))
    d = grab(r"^CFG_ID=(\S+)", r.cli(dh, "cfg-id", "--vault", s["VAULT"], "--chain-id", s["EVM"], "--lez-chain-id", s["LEZ_CHAIN_ID"],
                                     "--zone-id", s["ZONE_ID"], "--bridge", s["BP"], name="cfg-id"), "cfg")
    assert d.lower() == s["CFG_ID"].lower(), (d, s["CFG_ID"])
    once("DEF_ETH", lambda: grab(r"^token (\S+)", r.cli(dh, "create-wrapped", "--token-program", s["TP"], "--bridge", s["BP"], "--name",
                                                        "Wrapped ETH (privacy test)", "--payer", s["D"], name="wrap-eth"), "def"))
    once("DEF_USDC", lambda: grab(r"^token (\S+)", r.cli(dh, "create-wrapped", "--token-program", s["TP"], "--bridge", s["BP"], "--name",
                                                         "Wrapped USDC (privacy test)", "--payer", s["D"], name="wrap-usdc"), "def"))
    once("INIT", lambda: block_of(r.cli(dh, "init", "--bridge", s["BP"], "--token-program", s["TP"], "--admin", s["D"], "--vault", s["VAULT"],
                                        "--chain-id", s["EVM"], "--freezer", s["D"], "--attestor", s["D"], "--payer", s["D"], name="init")))
    once("REG_ETH", lambda: block_of(r.cli(dh, "register", "--bridge", s["BP"], "--admin", s["D"], "--eth-token", ETH, "--definition", s["DEF_ETH"],
                                           "--decimals", 18, "--denoms", "%d,%d" % tuple(ETH_D), name="register-eth")))
    once("REG_USDC", lambda: block_of(r.cli(dh, "register", "--bridge", s["BP"], "--admin", s["D"], "--eth-token", s["USDC"], "--definition",
                                            s["DEF_USDC"], "--decimals", 6, "--denoms", "%d,%d" % tuple(USDC_D), name="register-usdc")))
    once("SPONSOR", lambda: block_of(r.cli(dh, "set-sponsor", "--bridge", s["BP"], "--admin", s["D"], "--amount", SPONSOR, name="set-sponsor")))
    once("FLOAT", lambda: block_of(r.cli(dh, "fund-float", "--bridge", s["BP"], "--from", s["D"], "--amount", 4_000_000_000, name="fund-float")))
    t = r.cli(dh, "show", "--bridge", s["BP"], name="show0")
    assert grab(r"^cfg_id (\S+)", t, "cfg").lower() == s["CFG_ID"].lower()
    # The release relayer: one fresh Ethereum account sends every user's release (key in secret/, funded once).
    kf = os.path.join(r.d, "secret", "relayer.ethkey")
    if not os.path.exists(kf):
        with open(os.open(kf, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as f:
            f.write("0x" + secrets.token_hex(32) + "\n")
    s["RELAYER"] = r.cast("wallet", "address", "--private-key", relayer_key(r))
    r.esend(ANVIL_KEY0, s["RELAYER"], "--value", "10ether")
    r.save()
    r.mark("deploy")


def relayer_key(r):
    if "RELAYER_KEY" in r.s:  # runs made before the key moved to secret/ (2026-11-17 run a20)
        return r.s["RELAYER_KEY"]
    return open(os.path.join(r.d, "secret", "relayer.ethkey")).read().strip()


def cohort_sizes(r):
    n = r.s["n"]
    c = r.s["controls"]
    return [("main", n)] + ([(k, c) for k in ("c1_public", "c2_addrsecret", "c3_fee", "c4_self")] if c else [])


def p_users(r):
    s = r.s
    if r.done("users"):
        return
    users = []
    opw = os.path.join(r.d, "chain", "wallets", "operator")
    for cohort, cnt in cohort_sizes(r):
        for j in range(cnt):
            name = "%s-%02d" % (cohort, j)
            h = r.wallet(name)
            mn = os.path.join(r.d, "secret", name + ".mnemonic")
            r.cli(h, "wallet", "check-health", name=name + "-health", check=False, env_extra={"LEZ_BRIDGE_TEST_MNEMONIC_FILE": mn})
            u = {"name": name, "cohort": cohort, "home": h}
            tok, di, _ = COHORTS[cohort]
            u["token"] = ETH if tok == "ETH" else s["USDC"]
            u["di"] = di
            u["denom"] = (ETH_D if tok == "ETH" else USDC_D)[di]
            u["def"] = s["DEF_ETH"] if tok == "ETH" else s["DEF_USDC"]
            if cohort == "c1_public":
                t = r.cli(h, "wallet", "account", "new", "public", name=name + "-new")
                u["acct"] = acct("Public", t)
                u["kind"] = "public"
                pk = grab(r"^With pk ([0-9a-f]{64})", t, "pk")
                r.cli(opw, "wallet", "auth-transfer", "send", "--from", "Public/" + s["OP"], "--to-pk", pk, "--amount", 1_000_000_000, name=name + "-fund")
            else:
                t = r.cli(h, "wallet", "account", "new", "private", name=name + "-new")
                u["acct"] = acct("Private", t)
                u["kind"] = "private"
            k = "0x" + secrets.token_hex(32)
            kf = os.path.join(r.d, "secret", name + ".ethkey")
            with open(os.open(kf, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as f:
                f.write(k + "\n")
            u["eth"] = r.cast("wallet", "address", "--private-key", k)
            users.append(u)
            log("user", name, u["kind"], u["acct"][:10] + "…", "eth", u["eth"])
    # fund every Ethereum account from anvil account 0 (the same funder for all: no per-user signal)
    order = list(range(len(users)))
    r.rng.shuffle(order)
    for i in order:
        u = users[i]
        r.esend(ANVIL_KEY0, u["eth"], "--value", "1ether")
        if u["token"] != ETH:
            r.esend(ANVIL_KEY0, s["USDC"], "transfer(address,uint256)", u["eth"], u["denom"] + u["denom"] // 100)
    s["users"] = users
    r.save()
    r.mark("users")


def ukey(r, u):
    return open(os.path.join(r.d, "secret", u["name"] + ".ethkey")).read().strip()


def fee_tag(u):
    """c3_fee: a per-user fee value (wei for Ethereum's priority fee; the LEZ --gas-limit)."""
    return 3_000_000 + 7_919 * (1 + int(u["name"][-2:]))


def one_deposit(r, u):
    s = r.s
    if u["cohort"] == "c2_addrsecret":
        sec = "0x" + "00" * 12 + u["eth"][2:].lower()
        sf = os.path.join(r.d, "secret", u["name"] + ".depsecret")
        if not os.path.exists(sf):
            with open(os.open(sf, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as f:
                f.write(sec + "\n")
        t = r.cli(u["home"], "commitment", "--secret-file", sf, "--dest", u["acct"], "--eth-token", u["token"], "--denom", u["denom"],
                  "--cfg-id", s["CFG_ID"], name=u["name"] + "-commitment")
        c = t.strip().splitlines()[-1].strip()
        u["key_index"] = None
    else:
        t = r.cli(u["home"], "deposit-request", "--bridge", s["BP"], "--from", u["acct"], "--eth-token", u["token"], "--denom-index", u["di"],
                  name=u["name"] + "-depreq")
        c = grab(r"^COMMITMENT=(\S+)", t, "commitment")
        u["key_index"] = int(grab(r"^KEY_INDEX=(\d+)", t, "key index"))
    k = ukey(r, u)
    extra = []
    if u["cohort"] == "c3_fee":
        extra = ["--priority-gas-price", str(fee_tag(u)), "--gas-price", str(10 ** 10)]
    if u["token"] == ETH:
        j = r.esend(k, s["VAULT"], "depositETH(uint32,bytes32)", u["di"], c, "--value", str(u["denom"] + ETH_DF[u["di"]]), extra=extra)
    else:
        need = u["denom"] + u["denom"] // 100  # the vault's 1% deposit fee
        have = int(r.cast("call", s["USDC"], "balanceOf(address)(uint256)", u["eth"]).split()[0])
        if have < need:
            r.esend(ANVIL_KEY0, s["USDC"], "transfer(address,uint256)", u["eth"], need - have)
        r.esend(k, s["USDC"], "approve(address,uint256)", s["VAULT"], need)
        j = r.esend(k, s["VAULT"], "deposit(address,uint32,bytes32)", u["token"], u["di"], c, extra=extra)
    assert int(j["status"], 16) == 1, j
    u["C"] = c
    u["dep_tx"] = j["transactionHash"]
    r.truth("deposits", {"user": u["name"], "cohort": u["cohort"], "tx": u["dep_tx"], "commitment": c, "sender": u["eth"],
                         "token": u["token"], "denom": u["denom"], "block": int(j["blockNumber"], 16)})
    r.save()
    log("deposit", u["name"], "tx", u["dep_tx"][:12] + "…")


def p_deposits(r):
    s = r.s
    users = s["users"]
    order = [i for i in range(len(users)) if "dep_tx" not in users[i]]
    r.rng.shuffle(order)
    for i in order:
        one_deposit(r, users[i])
        r.pause(1, 6)
    r.mark("deposits") if not r.done("deposits") else None


def p_attest(r):
    s = r.s
    if r.done("attest"):
        return
    root = r.cast("call", s["VAULT"], "depositRoot()(bytes32)")
    t = r.cli(r.wallet("deployer"), "attest", "--bridge", s["BP"], "--attestor", s["D"], "--root", root, name="attest")
    s["ROOT"] = root
    r.truth("attest", {"root": root, "block": block_of(t)})
    r.mark("attest")


def finalize(r):
    """anvil's `finalized` block trails the head by 64; the CLI reads deposit leaves up to it: mine 70 empty blocks."""
    r.cast("rpc", "anvil_mine", "0x46")


def one_claim(r, u):
    s = r.s
    args = ["claim", "--bridge", s["BP"], "--token-program", s["TP"], "--from", u["acct"], "--eth-token", u["token"],
            "--denom-index", u["di"], "--denom", u["denom"], "--cfg-id", s["CFG_ID"]]
    pre = []
    if u["cohort"] == "c2_addrsecret" and not u.get("shielded"):
        # --reveal-recipient takes no sponsorship: fund V for its fee through `shield` (credits a one-time
        # address of V) from a public account of the user's own wallet, funded by the chain operator.
        opw = os.path.join(r.d, "chain", "wallets", "operator")
        t = r.cli(u["home"], "wallet", "account", "new", "public", name=u["name"] + "-feeacct")
        fa, pk = acct("Public", t), grab(r"^With pk ([0-9a-f]{64})", t, "pk")
        r.cli(opw, "wallet", "auth-transfer", "send", "--from", "Public/" + s["OP"], "--to-pk", pk, "--amount", 300_000_000, name=u["name"] + "-feefund")
        r.run([CLI, "shield", "--from", fa, "--to", u["acct"], "--amount", 150_000_000], home=u["home"], name=u["name"] + "-shield", guard=True, timeout=600)
        u["shielded"] = fa
        r.save()
    if u["cohort"] == "c2_addrsecret":
        args += ["--secret-file", os.path.join(r.d, "secret", u["name"] + ".depsecret"), "--reveal-recipient"]
    else:
        args += ["--key-index", u["key_index"]]
    if u["cohort"] == "c3_fee":
        pre = ["--gas-limit", fee_tag(u)]
    t, rc, dt = r.run([CLI] + [str(x) for x in pre + args], home=u["home"], name=u["name"] + "-claim", guard=(u["kind"] == "private"), timeout=600)
    b = block_of(t)
    assert b, t[-800:]
    u["claim_block"] = b
    m = re.search(r"recipient ([1-9A-HJ-NP-Za-km-z]{32,44}) \(one-time", t)
    u["one_time"] = m.group(1) if m else None
    u["claim_s"] = round(dt, 1)
    r.truth("claims", {"user": u["name"], "cohort": u["cohort"], "block": b, "account": u["acct"], "kind": u["kind"],
                       "one_time": u["one_time"], "seconds": u["claim_s"]})
    r.save()
    log("claim", u["name"], u["kind"], "block", b, "%.1fs" % dt)


def p_claims(r):
    s = r.s
    finalize(r)
    users = s["users"]
    order = [i for i in range(len(users)) if "claim_block" not in users[i]]
    r.rng.shuffle(order)
    for i in order:
        one_claim(r, users[i])
        r.pause(1, 6)
    if not r.done("claims"):
        r.mark("claims")


def one_burn(r, u):
    s = r.s
    pre = ["--gas-limit", fee_tag(u)] if u["cohort"] == "c3_fee" else []
    args = ["burn", "--bridge", s["BP"], "--token-program", s["TP"], "--from", u["acct"], "--definition", u["def"], "--amount", u["denom"]]
    t, rc, dt = r.run([CLI] + [str(x) for x in pre + args], home=u["home"], name=u["name"] + "-burn", guard=(u["kind"] == "private"), timeout=600)
    b = block_of(t)
    assert b, t[-800:]
    u["burn_block"] = b
    u["E"] = grab(r"^exit commitment (0x[0-9a-f]+)", t, "E")
    u["exit_index"] = int(grab(r"derived exit: index (\d+)", t, "exit index"))
    u["burn_s"] = round(dt, 1)
    r.truth("burns", {"user": u["name"], "cohort": u["cohort"], "block": b, "E": u["E"], "seconds": u["burn_s"]})
    r.save()
    log("burn", u["name"], "block", b, "%.1fs" % dt)


def p_burns(r):
    s = r.s
    users = s["users"]
    order = [i for i in range(len(users)) if "burn_block" not in users[i]]
    r.rng.shuffle(order)
    for i in order:
        one_burn(r, users[i])
        r.pause(1, 6)
    if not r.done("burns"):
        r.mark("burns")


def p_update(r, tag="update1"):
    s = r.s
    if r.done(tag):
        return
    prev = s.get("G2_LAST_STATE", os.path.join(r.d, "g2", "state0.bin"))
    tip = int(r.run([HOST, "last", "--rpc", SEQ], name="host-last-" + tag, dev=False)[0].strip())
    outd = os.path.join(r.d, "g2", tag)
    r.run(["nice", "-n", "19", HOST, "update", "--rpc", SEQ, "--dir", os.path.join(r.d, "g2", "blocks"), "--state", prev, "--to", tip, "--out", outd],
          name="g2-" + tag, dev=False, timeout=3600)
    j = open(os.path.join(outd, "journal.hex")).read().strip()
    seal = open(os.path.join(outd, "seal.hex")).read().strip()
    tx = r.esend(ANVIL_KEY0, s["VAULT"], "update(bytes,bytes)", j, seal)
    assert int(tx["status"], 16) == 1
    s["G2_LAST_STATE"] = os.path.join(outd, "next-state.bin")
    r.truth("updates", {"tag": tag, "to": tip, "tx": tx["transactionHash"]})
    r.save()
    r.mark(tag)


def exit_leaves(r):
    s = r.s
    logs = json.loads(r.cast("logs", "--from-block", "0", "--address", s["VAULT"], "StateUpdated(bytes32,bytes32,bytes32,uint64,bytes32[])", "--json"))
    leaves = []
    for l in logs:
        d = bytes.fromhex(l["data"][2:])
        off = int.from_bytes(d[128:160], "big")
        n = int.from_bytes(d[off:off + 32], "big")
        leaves += ["0x" + d[off + 32 + 32 * k: off + 64 + 32 * k].hex() for k in range(n)]
    return leaves


def exit_leaf(cfg, tok, denom, E):
    import hashlib
    tag = bytes.fromhex("22018742ad6028fda6cb0726afaae90ea20dff3cef6b1ec5b3c14f856d76c59c")
    return "0x" + hashlib.sha256(tag + bytes.fromhex(cfg[2:]) + bytes.fromhex(tok[2:]) + int(denom).to_bytes(32, "big") + bytes.fromhex(E[2:])).hexdigest()


def release_user(r, u, sender_key, relayer, fee, tag="", extra=()):
    s = r.s
    leaves = exit_leaves(r)
    leaf = exit_leaf(s["CFG_ID"], u["token"], u["denom"], u["E"])
    idx = leaves.index(leaf)
    lf = os.path.join(r.d, "out", "exit-leaves.txt")
    open(lf, "w").write("\n".join(leaves) + "\n")
    sf = os.path.join(r.d, "secret", "%s.exit%d%s" % (u["name"], u["exit_index"], tag))
    if not os.path.exists(sf):
        t = r.cli(u["home"], "exit-secret", "--bridge", s["BP"], "--from", u["acct"], "--eth-token", u["token"], "--denom", u["denom"],
                  "--key-index", u["exit_index"], "--out", sf, name=u["name"] + "-exitsecret" + tag)
        u["eth_dest"] = grab(r"release address (0x[0-9a-fA-F]{40})", t, "release address")
        assert grab(r"E (0x[0-9a-f]+)", t, "E") == u["E"]
    t = r.run([HOST, "release", "--secret-file", sf, "--eth-dest", u["eth_dest"], "--token", u["token"], "--denom", u["denom"],
               "--max-relay-fee", str(u["denom"] * 2 // 100), "--cfg-id", s["CFG_ID"], "--leaves", lf, "--index", idx,
               "--relayer", relayer, "--relay-fee", fee], name=u["name"] + "-g3" + tag, dev=False)[0]
    kv = dict(re.findall(r"^(ROOT|NULLIFIER|SEAL)=(\S+)", t, re.M))
    tx = r.esend(sender_key, s["VAULT"], "release(bytes,bytes32,bytes32,address,uint32,address,address,uint256)",
                 kv["SEAL"], kv["ROOT"], kv["NULLIFIER"], u["token"], u["di"], u["eth_dest"], relayer, fee, extra=extra)
    assert int(tx["status"], 16) == 1, tx
    return tx, kv


def one_release(r, u):
    s = r.s
    if u["cohort"] == "c4_self":
        key, relayer = ukey(r, u), u["eth"]
    else:
        key, relayer = relayer_key(r), s["RELAYER"]
    extra = ["--priority-gas-price", str(fee_tag(u)), "--gas-price", str(10 ** 10)] if u["cohort"] == "c3_fee" else []
    tx, kv = release_user(r, u, key, relayer, 0, extra=extra)
    u["rel_tx"] = tx["transactionHash"]
    r.truth("releases", {"user": u["name"], "cohort": u["cohort"], "tx": u["rel_tx"], "eth_dest": u["eth_dest"], "sender": relayer,
                         "nullifier": kv["NULLIFIER"], "root": kv["ROOT"]})
    r.save()
    log("release", u["name"], "to", u["eth_dest"], "tx", u["rel_tx"][:12] + "…")


def p_releases(r):
    s = r.s
    users = s["users"]
    order = [i for i in range(len(users)) if "rel_tx" not in users[i]]
    r.rng.shuffle(order)
    time.sleep(6)  # the vault's release delay (5 s)
    r.esend(ANVIL_KEY0, s["RELAYER"], "--value", "0")
    for i in order:
        one_release(r, users[i])
        r.pause(1, 6)
    if not r.done("releases"):
        r.mark("releases")


def p_capture(r):
    """Everything public: every Ethereum block, transaction, receipt (logs) and the vault's storage diffs;
    every LEZ block's bytes; the sequencer's account views of every account id that appears in any block."""
    cap = os.path.join(r.d, "capture")
    st = r.s
    cfg_acct = grab(r"^CONFIG=(\S+)", r.cli(r.wallet("deployer"), "program-id", "--bridge", st["BP"], name="program-id2"), "config")
    # The deployment's public constants (each is published by the deploy, init and register transactions).
    json.dump({"vault": st["VAULT"], "cfg_id": st["CFG_ID"], "bridge": st["BP"], "config": cfg_acct, "token_program": st["TP"],
               "tokens": [{"eth": ETH, "def": st["DEF_ETH"], "denoms": ETH_D}, {"eth": st["USDC"].lower(), "def": st["DEF_USDC"], "denoms": USDC_D}]},
              open(os.path.join(cap, "public.json"), "w"), indent=1)
    n = int(r.eth("eth_blockNumber"), 16)
    eth = []
    for b in range(0, n + 1):
        blk = r.eth("eth_getBlockByNumber", [hex(b), True])
        rec = {"block": blk, "receipts": [], "diffs": {}}
        for tx in blk["transactions"]:
            rec["receipts"].append(r.eth("eth_getTransactionReceipt", [tx["hash"]]))
            try:
                rec["diffs"][tx["hash"]] = r.eth("debug_traceTransaction", [tx["hash"], {"tracer": "prestateTracer", "tracerConfig": {"diffMode": True}}])
            except Exception as e:  # noqa
                rec["diffs"][tx["hash"]] = {"error": str(e)}
        eth.append(rec)
    json.dump(eth, open(os.path.join(cap, "ethereum.json"), "w"))
    tip = int(r.lez("getLastBlockId"))
    blocks = {}
    for b in range(1, tip + 1):
        blocks[b] = r.lez("getBlock", [b])
    json.dump(blocks, open(os.path.join(cap, "lez-blocks.json"), "w"))
    # Account views: the sequencer serves only current state (no history), so every account id the blocks
    # name as an actor (a 32-byte id followed by a program id that recurs in many blocks) is read once, at the end.
    import base64
    from collections import Counter
    raw = [base64.b64decode(v)[80:] for v in blocks.values()]
    cnt = Counter()
    for b in raw:
        cnt.update({b[i:i + 32] for i in range(len(b) - 31)})
    progs = {w for w, c in cnt.items() if c > len(raw) // 2 and len(set(w)) > 8}
    ids = set()
    for b in raw:
        for i in range(len(b) - 63):
            if b[i + 32:i + 64] in progs:
                ids.add(b[i:i + 32])
    A58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"

    def b58(x):
        n, out = int.from_bytes(x, "big"), ""
        while n:
            n, k = divmod(n, 58)
            out = A58[k] + out
        return out
    views = {}
    for x in sorted(ids):
        try:
            views[b58(x)] = r.lez("getAccount", [b58(x)])
        except Exception as e:  # noqa
            views[b58(x)] = {"error": str(e)[:200]}
    json.dump(views, open(os.path.join(cap, "lez-accounts.json"), "w"))
    log("account views:", len(views), "ids named as actors,", sum(1 for v in views.values() if v and "error" not in v), "answered")
    json.dump({"lez_rpc_methods_used": ["getBlock", "getLastBlockId", "getAccount (end state of every actor id)"], "eth_methods_used":
               ["eth_getBlockByNumber", "eth_getTransactionReceipt", "debug_traceTransaction(prestateTracer, diffMode)"],
               "lez_tip": tip, "eth_tip": n, "mode": LABEL}, open(os.path.join(cap, "meta.json"), "w"), indent=1)
    log("captured", n + 1, "Ethereum blocks,", tip, "LEZ blocks")


# ---------------------------------------------------------------------- interleaved schedule (run "mixed")
def p_inbound_mixed(r):
    """Deposits and claims interleaved: each user claims after a random delay (0-3 other deposits later);
    the attestor records a new root after every deposit, so each claim names the newest root at its time."""
    s = r.s
    users = s["users"]
    if "mixed_plan" not in s:
        order = list(range(len(users)))
        r.rng.shuffle(order)
        ev = []
        for pos, i in enumerate(order):
            ev.append((pos, 0, "deposit", i))
            ev.append((pos + r.rng.uniform(0.5, 3.5), 1, "claim", i))
        ev.sort()
        s["mixed_plan"] = [[e[2], e[3]] for e in ev]
        r.save()
    for k, (what, i) in enumerate(s["mixed_plan"]):
        u = users[i]
        if what == "deposit" and "dep_tx" not in u:
            r.s["users"] = users
            one_deposit(r, u)
            finalize(r)
            root = r.cast("call", s["VAULT"], "depositRoot()(bytes32)")
            t = r.cli(r.wallet("deployer"), "attest", "--bridge", s["BP"], "--attestor", s["D"], "--root", root, name="attest-%d" % k)
            r.truth("attest", {"root": root, "block": block_of(t)})
        elif what == "claim" and "claim_block" not in u:
            one_claim(r, u)


def p_outbound_mixed(r):
    """Burns in two batches with a G2 update after each; half of batch 1 releases between the updates."""
    s = r.s
    users = s["users"]
    if "out_plan" not in s:
        order = list(range(len(users)))
        r.rng.shuffle(order)
        h = len(order) // 2
        b1, b2 = order[:h], order[h:]
        early = b1[:len(b1) // 2]
        s["out_plan"] = {"b1": b1, "b2": b2, "early": early, "late": [i for i in order if i not in early]}
        r.save()
    P = s["out_plan"]
    for i in P["b1"]:
        if "burn_block" not in users[i]:
            one_burn(r, users[i])
    p_update(r, "update1")
    time.sleep(6)
    for i in P["early"]:
        if "rel_tx" not in users[i]:
            one_release(r, users[i])
    for i in P["b2"]:
        if "burn_block" not in users[i]:
            one_burn(r, users[i])
    p_update(r, "update2")
    time.sleep(6)
    late = P["late"][:]
    r.rng.shuffle(late)
    for i in late:
        if "rel_tx" not in users[i]:
            one_release(r, users[i])


# ---------------------------------------------------------------------- Part B: recovery from the seed alone
def rec_users(r):
    return [u for u in r.s["users"] if (u["cohort"] == "main" and int(u["name"][-2:]) < 10) or u["cohort"] == "c1_public"]


def lsdir(path):
    return subprocess.run(["ls", "-laR", path], capture_output=True, text=True).stdout


def p_recovery(r):
    """Part B (after the Part A capture is frozen): each recovery user makes two more deposits (claims one,
    leaves one), burns the new token (left unreleased, in the exit tree after an update); then its wallet
    directory is deleted, restored from the seed phrase alone and `recover` scans both chains. Found /
    expected is counted; then each restored wallet claims its claimable deposit and releases its exit."""
    s = r.s
    users = rec_users(r)
    B = s.setdefault("recovery", {})
    if "deposits" not in B:
        for u in users:
            if u["token"] != ETH:
                r.esend(ANVIL_KEY0, s["USDC"], "transfer(address,uint256)", u["eth"], 2 * (u["denom"] + u["denom"] // 100))
        for u in users:
            for _ in range(2):
                t = r.cli(u["home"], "deposit-request", "--bridge", s["BP"], "--from", u["acct"], "--eth-token", u["token"], "--denom-index", u["di"],
                          name=u["name"] + "-B-depreq")
                c = grab(r"^COMMITMENT=(\S+)", t, "commitment")
                ki = int(grab(r"^KEY_INDEX=(\d+)", t, "key index"))
                k = ukey(r, u)
                if u["token"] == ETH:
                    r.esend(k, s["VAULT"], "depositETH(uint32,bytes32)", u["di"], c, "--value", str(u["denom"] + ETH_DF[u["di"]]))
                else:
                    r.esend(k, s["USDC"], "approve(address,uint256)", s["VAULT"], u["denom"] + u["denom"] // 100)
                    r.esend(k, s["VAULT"], "deposit(address,uint32,bytes32)", u["token"], u["di"], c)
                u.setdefault("B_dep", []).append(ki)
            log("B deposits", u["name"], u["B_dep"])
        B["deposits"] = True
        r.save()
    if "claims" not in B:
        finalize(r)
        root = r.cast("call", s["VAULT"], "depositRoot()(bytes32)")
        r.cli(r.wallet("deployer"), "attest", "--bridge", s["BP"], "--attestor", s["D"], "--root", root, name="B-attest")
        for u in users:
            if u.get("B_claimed"):
                continue
            r.run([CLI, "claim", "--bridge", s["BP"], "--token-program", s["TP"], "--from", u["acct"], "--eth-token", u["token"], "--denom-index",
                   u["di"], "--denom", u["denom"], "--cfg-id", s["CFG_ID"], "--key-index", u["B_dep"][0]], home=u["home"], name=u["name"] + "-B-claim",
                  guard=(u["kind"] == "private"), timeout=600)
            u["B_claimed"] = u["B_dep"][0]
            r.save()
            log("B claim", u["name"], "key index", u["B_dep"][0])
        B["claims"] = True
        r.save()
    if "burns" not in B:
        for u in users:
            if u.get("B_exit") is not None:
                continue
            t = r.run([CLI, "burn", "--bridge", s["BP"], "--token-program", s["TP"], "--from", u["acct"], "--definition", u["def"], "--amount", u["denom"]],
                      home=u["home"], name=u["name"] + "-B-burn", guard=(u["kind"] == "private"), timeout=600)[0]
            u["B_exit"] = int(grab(r"derived exit: index (\d+)", t, "exit index"))
            u["B_E"] = grab(r"^exit commitment (0x[0-9a-f]+)", t, "E")
            r.save()
            log("B burn", u["name"], "exit index", u["B_exit"])
        B["burns"] = True
        r.save()
    p_update(r, "update2")
    # The truth each user's recovery must reproduce (from what this harness did).
    for u in users:
        u["B_expect"] = {"deposits": sorted([[u["key_index"], "claimed"], [u["B_dep"][0], "claimed"], [u["B_dep"][1], "claimable"]]),
                         "exits": sorted([[u["exit_index"], "released"], [u["B_exit"], "unreleased"]])}
    r.save()
    res = B.setdefault("results", {})
    for u in users:
        if u["name"] in res:
            continue
        before = lsdir(u["home"])
        shutil.rmtree(u["home"])
        os.makedirs(u["home"])
        os.chmod(u["home"], 0o700)
        after = lsdir(u["home"])
        open(os.path.join(r.d, "out", u["name"] + "-B-wipe.txt"), "w").write("BEFORE (ls -laR)\n" + before + "\nAFTER the delete\n" + after)
        mn = os.path.join(r.d, "secret", u["name"] + ".mnemonic")
        with open(mn) as f:
            p = subprocess.run([CLI, "wallet", "restore-keys", "--depth", "4", "--yes"], stdin=f, capture_output=True, text=True, env=r.env(u["home"]))
        open(os.path.join(r.d, "out", u["name"] + "-B-restore.out"), "w").write(p.stdout + p.stderr)
        assert p.returncode == 0, p.stderr[-500:]
        j = cli_json(r, u, "-B-recover", "recover", "--bridge", s["BP"])
        got_d = sorted([[d["key_index"], d["state"]] for d in j["deposits"] if d["account"] in (u["acct"], "0x" + b58hex(u["acct"]))])
        got_e = sorted([[e["key_index"], e["state"]] for e in j["exits"] if e["account"] in (u["acct"], "0x" + b58hex(u["acct"]))])
        exp = u["B_expect"]
        res[u["name"]] = {"kind": u["kind"], "deposits_found": sum(1 for x in exp["deposits"] if x in got_d), "deposits_expected": len(exp["deposits"]),
                          "exits_found": sum(1 for x in exp["exits"] if x in got_e), "exits_expected": len(exp["exits"]),
                          "extra": [x for x in got_d + got_e if x not in exp["deposits"] + exp["exits"]], "got_deposits": got_d, "got_exits": got_e,
                          "dir_before_files": before.count("\n-"), "dir_after_files": after.count("\n-")}
        r.save()
        log("B recover", u["name"], res[u["name"]]["deposits_found"], "/", 3, "deposits;", res[u["name"]]["exits_found"], "/", 2, "exits")
    # complete from the recovered state: claim the claimable deposit, release the unreleased exit
    done = B.setdefault("completed", {})
    finalize(r)
    root = r.cast("call", s["VAULT"], "depositRoot()(bytes32)")
    for u in users:
        if done.get(u["name"]):
            continue
        r.run([CLI, "claim", "--bridge", s["BP"], "--token-program", s["TP"], "--from", u["acct"], "--eth-token", u["token"], "--denom-index", u["di"],
               "--denom", u["denom"], "--cfg-id", s["CFG_ID"], "--key-index", u["B_dep"][1]], home=u["home"], name=u["name"] + "-B-claim2",
              guard=(u["kind"] == "private"), timeout=600)
        v = dict(u)
        v["E"], v["exit_index"] = u["B_E"], u["B_exit"]
        tx, kv = release_user(r, v, relayer_key(r), s["RELAYER"], 0, tag="-B")
        j = cli_json(r, u, "-B-recover2", "recover", "--bridge", s["BP"])
        done[u["name"]] = {"claim2": True, "release_tx": tx["transactionHash"],
                           "after": {"deposits": sorted([[d["key_index"], d["state"]] for d in j["deposits"]]),
                                     "exits": sorted([[e["key_index"], e["state"]] for e in j["exits"]])}}
        r.save()
        log("B completed", u["name"], done[u["name"]]["after"])
    json.dump({"results": res, "completed": done}, open(os.path.join(r.d, "truth", "recovery.json"), "w"), indent=1)


def cli_json(r, u, tag, *a):
    p = subprocess.run([CLI, "--json"] + [str(x) for x in a], capture_output=True, text=True, env=r.env(u["home"]), timeout=900)
    open(os.path.join(r.d, "out", u["name"] + tag + ".json"), "w").write(p.stdout)
    open(os.path.join(r.d, "out", u["name"] + tag + ".err"), "w").write(p.stderr)
    assert p.returncode == 0, p.stderr[-800:]
    return json.loads(p.stdout)


def b58hex(a):
    A = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    n = 0
    for c in a:
        n = n * 58 + A.index(c)
    return n.to_bytes(32, "big").hex()


def p_scrub(r):
    """After `stop`: delete what holds keys or chain state (secret/, wallets/, chain/, g2/, out/ (CLI prints with
    wallet paths)); keep capture/, capture-frozen/, truth/, adv*/ and state.json without the user homes' keys."""
    ev = os.path.join(r.d, "truth", "part-b-evidence")
    os.makedirs(ev, exist_ok=True)
    od = os.path.join(r.d, "out")
    for f in (os.listdir(od) if os.path.isdir(od) else []):
        if "-B-wipe" in f or "-B-recover" in f or "-B-restore" in f:
            shutil.copy(os.path.join(od, f), ev)
    for sub in ("secret", "wallets", "chain", "g2", "out"):
        shutil.rmtree(os.path.join(r.d, sub), ignore_errors=True)
    r.s.pop("RELAYER_KEY", None)
    r.save()
    log("scrubbed: kept", sorted(os.listdir(r.d)))

PHASES = {"chain": p_chain, "deploy": p_deploy, "users": p_users, "deposits": p_deposits, "attest": p_attest, "claims": p_claims,
          "burns": p_burns, "update": p_update, "releases": p_releases, "capture": p_capture, "stop": p_stop, "recovery": p_recovery,
          "inbound_mixed": p_inbound_mixed, "outbound_mixed": p_outbound_mixed, "scrub": p_scrub}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True)
    ap.add_argument("--n", type=int, default=20)
    ap.add_argument("--controls", type=int, default=5, help="users per control cohort (0: none)")
    ap.add_argument("--seed", type=int, default=20261117)
    ap.add_argument("phases", nargs="+")
    a = ap.parse_args()
    r = Run(a.run, a.n, a.seed, a.controls)
    log(LABEL)
    ph = a.phases
    if ph == ["all"]:
        ph = ["chain", "deploy", "users", "deposits", "attest", "claims", "burns", "update", "releases", "capture"]
    for p in ph:
        log("== phase", p)
        PHASES[p](r)


if __name__ == "__main__":
    main()
