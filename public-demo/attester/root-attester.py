#!/usr/bin/python3
"""Stand-in root attester for the public-demo chain (PLAN.md section 6, option (a)).

Every POLL seconds (60, never under 30) it reads the Sepolia vault's depositRoot() at block tag
`finalized` (SEPOLIA_RPC: one RPC or a comma-separated list, each request tried in order until
one answers; default PublicNode, then Tenderly's public gateway; it reads no logs, only state at
the finalized block, which PublicNode still serves), and if that root differs from the last one it recorded, records it
on the LEZ demo chain with `lez-bridge attest --bridge B --attestor OP --root R` (a public LEZ
transaction, sent to the loopback sequencer; nothing is proved). It writes only on change.

Fail-safe:
- at start, and every hour, the vault's cfgId() must equal the bridge config's cfg_id, else it
  stops (exit 3);
- if Sepolia's finalized block is older than MAX_FINALITY_AGE (30 min), or the RPC fails, it
  attests nothing and tries again next poll; claims just wait;
- it never reads, prints or copies a key: the operator wallet stays in its directory on the test machine
  and only the CLI opens it.

Labelled on the page as "operator-attested": whoever holds the attestor key could record a root
the vault never had. Anyone can check each recorded root with the vault's isDepositRoot(root).

Stdlib only (/usr/bin/python3 3.9). Run by attester.sh (nohup, pid file).
"""
import json, os, subprocess, sys, time, urllib.request

P = os.environ.get("P", "./public-chain")
CLI = os.environ.get("CLI", P + "/bin/lez-bridge")
WALLET = os.environ.get("OPERATOR_WALLET", P + "/wallets/operator")
SEQUENCER = os.environ.get("SEQUENCER", "http://127.0.0.1:3290")
SEPOLIAS = [u.strip() for u in os.environ.get(
    "SEPOLIA_RPC", "https://ethereum-sepolia-rpc.publicnode.com,https://sepolia.gateway.tenderly.co").split(",") if u.strip()]
BRIDGE = os.environ.get("BRIDGE", "JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7")
ATTESTOR = os.environ.get("ATTESTOR", "BVDcg8SbSdCTKzx4mJbSpVgEuaoPEMhxmmRwvVCGyBnD")
VAULT = os.environ.get("VAULT", "0x8C8b2664De28e5287158db4DB65D448638871b74")
POLL = max(30, int(os.environ.get("POLL", "60")))
MAX_FINALITY_AGE = 30 * 60
STATE = P + "/attester/state.json"
LOG = P + "/logs/attester.log"

SEL_DEPOSIT_ROOT = "0x8955fe8a"    # depositRoot()
SEL_DEPOSIT_COUNT = "0x2dfdf0b5"   # depositCount()
SEL_CFG_ID = "0xed9305e3"          # cfgId()
SEL_IS_DEPOSIT_ROOT = "0xc5bba38b" # isDepositRoot(bytes32)


def log(msg):
    with open(LOG, "a") as f:
        f.write("%s %s\n" % (time.strftime("%Y-%m-%d %H:%M:%S"), msg))


def rpc_at(url, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(url, body, {"Content-Type": "application/json",
                                             "User-Agent": "lez-demo-attester/1"})
    with urllib.request.urlopen(req, timeout=20) as r:
        out = json.load(r)
    if "error" in out:
        raise RuntimeError("%s: %s" % (method, out["error"]))
    if out.get("result") is None:
        raise RuntimeError("%s: null result" % method)
    return out["result"]


def rpc(method, params):
    """Through the first RPC of SEPOLIAS that answers; the error names each when none does."""
    fails = []
    for url in SEPOLIAS:
        try:
            return rpc_at(url, method, params)
        except Exception as e:
            fails.append("%s: %s" % (url, str(e)[:200]))
    raise RuntimeError("no Sepolia RPC answered %s: %s" % (method, "; ".join(fails) or "SEPOLIA_RPC is empty"))


def call(data, block):
    return rpc("eth_call", [{"to": VAULT, "data": data}, block])


def cli(*args):
    env = dict(os.environ, LEE_WALLET_HOME_DIR=WALLET, LEE_WALLET_PASSWORD="", LEZ_BRIDGE_SEQUENCER=SEQUENCER)
    r = subprocess.run([CLI, *args], env=env, capture_output=True, text=True, timeout=300)
    return r.returncode, r.stdout + r.stderr


def bridge_cfg_id():
    rc, out = cli("show", "--bridge", BRIDGE)
    if rc != 0:
        raise RuntimeError("show failed: " + out.strip().splitlines()[-1][:200])
    for line in out.splitlines():
        if line.startswith("cfg_id "):
            return line.split()[1].lower()
    raise RuntimeError("no cfg_id in show")


def check_cfg():
    vault = call(SEL_CFG_ID, "latest").lower()
    bridge = bridge_cfg_id()
    if vault != bridge:
        log("STOP: vault cfgId %s != bridge cfg_id %s" % (vault, bridge))
        sys.exit(3)
    return vault


def load_state():
    try:
        with open(STATE) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def save_state(s):
    tmp = STATE + ".tmp"
    with open(tmp, "w") as f:
        json.dump(s, f, indent=1)
    os.replace(tmp, STATE)


def main():
    os.makedirs(os.path.dirname(STATE), exist_ok=True)
    state = load_state()
    cfg = check_cfg()
    log("start: vault %s cfgId %s bridge %s attestor %s poll %ds; last attested %s; Sepolia RPCs %s"
        % (VAULT, cfg, BRIDGE, ATTESTOR, POLL, state.get("root"), ", ".join(SEPOLIAS)))
    last_cfg_check = time.time()
    last_heartbeat = 0
    failures = 0
    while True:
        try:
            if time.time() - last_cfg_check > 3600:
                check_cfg()
                last_cfg_check = time.time()
            fin = rpc("eth_getBlockByNumber", ["finalized", False])
            fin_no, fin_ts = int(fin["number"], 16), int(fin["timestamp"], 16)
            age = time.time() - fin_ts
            if age > MAX_FINALITY_AGE:
                if failures % 10 == 0:
                    log("Sepolia finalized block %d is %d s old: not attesting" % (fin_no, age))
                failures += 1
            else:
                block = hex(fin_no)
                root = call(SEL_DEPOSIT_ROOT, block).lower()
                count = int(call(SEL_DEPOSIT_COUNT, block), 16)
                if root != state.get("root"):
                    known = int(call(SEL_IS_DEPOSIT_ROOT + root[2:], block), 16)
                    if known != 1:
                        raise RuntimeError("vault says isDepositRoot(%s) = %d" % (root, known))
                    rc, out = cli("attest", "--bridge", BRIDGE, "--attestor", ATTESTOR, "--root", root)
                    applied = [l for l in out.splitlines() if l.startswith("applied in block")]
                    if rc != 0 or not applied:
                        raise RuntimeError("attest failed rc=%d: %s" % (rc, out.strip().splitlines()[-1][:200] if out.strip() else ""))
                    state = {"root": root, "deposit_count": count, "sepolia_block": fin_no,
                             "lez": applied[0], "attested_at": int(time.time())}
                    save_state(state)
                    log("attested root %s (deposits %d, Sepolia finalized block %d), %s" % (root, count, fin_no, applied[0]))
                failures = 0
                if time.time() - last_heartbeat > 3600:
                    log("alive: finalized %d, deposits %d, root %s" % (fin_no, count, root))
                    last_heartbeat = time.time()
        except SystemExit:
            raise
        except Exception as e:  # network, RPC or CLI trouble: log (rate-limited) and retry later
            failures += 1
            if failures <= 3 or failures % 30 == 0:
                log("error (%d in a row): %s" % (failures, str(e)[:300]))
        time.sleep(POLL if failures < 5 else min(600, POLL * 5))


if __name__ == "__main__":
    main()
