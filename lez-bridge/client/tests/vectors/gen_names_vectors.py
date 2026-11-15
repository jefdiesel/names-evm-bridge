#!/usr/bin/env python3
"""Independent reference for the bridge's Names app scheme (lez-bridge/client/src/kdf.rs,
"Names app scheme"): the Names Wallet's `app_secret` (Names branch `bridge-sdk`,
basecamp-module/rust-lib/src/app_secret.rs) with the bridge's scopes, contexts and tags.

Standard library only (hmac, hashlib). Writes kdf-names-v1.json next to this file. Release
addresses come from Foundry's `cast wallet address --private-key` when `cast` is on PATH.

The first case reproduces the Names branch's own test vectors (`app_secret.rs` `vectors`:
secret 0x07.., account 0x09.., context abcd, indexes 0 and 1) and fails if it does not.

    python3 gen_names_vectors.py
"""
import hashlib, hmac, json, os, shutil, subprocess

H = lambda *parts: hashlib.sha256(b"".join(parts)).digest()
u256 = lambda v: v.to_bytes(32, "big")
APP = b"lez_bridge"


def master(secret, app, scope, context, account, index):
    prk = hmac.new(b"logos-names/app-secret/v1", secret, hashlib.sha256).digest()
    info = (b"logos-names/app-secret/v1/info" + bytes([len(app)]) + app + bytes([len(scope)]) + scope
            + len(context).to_bytes(2, "big") + context + account + index.to_bytes(8, "big"))
    return hmac.new(prk, info + b"\x01", hashlib.sha256).digest()


def value(tag, m):
    return H(tag, m)


# The Names branch's literals (app_secret.rs, `vectors`).
NAMES_MASTER_0 = "d3d73d74e8003e7236d01e9b4bfcd9db81d6ade99c2304c4527ef46f5ef4b716"
NAMES_VALUES = ["8bdd25262f56570aa34cd58d46bd80fc6de18deedc75ac8fdc8994c500f19e59",
                "566f88b5c0b2ad570680236cdfb51278ba3b74e772be69443d4560036e2acdfb"]
m0 = master(bytes([7]) * 32, APP, b"RFP022/deposit", bytes.fromhex("abcd"), bytes([9]) * 32, 0)
assert m0.hex() == NAMES_MASTER_0, m0.hex()
got = [value(b"RFP022/ck", master(bytes([7]) * 32, APP, b"RFP022/deposit", bytes.fromhex("abcd"), bytes([9]) * 32, i)).hex() for i in (0, 1)]
assert got == NAMES_VALUES, got


def case(name, secret, account, cfg, token, denom, index, eth_dest, fee):
    dctx = cfg + token + u256(denom)
    ectx = cfg + token + u256(denom) + eth_dest + u256(fee)
    rctx = cfg + token + u256(denom) + u256(fee)
    s = master(secret, APP, b"RFP022/deposit", dctx, account, index)
    k = value(b"RFP022/ck", s)
    c = H(b"RFP022/dep", k, account, token, u256(denom), cfg)
    s2 = master(secret, APP, b"RFP022/exit", ectx, account, index)
    k2 = value(b"RFP022/ek", s2)
    e = H(b"RFP022/exit", k2, eth_dest, token, u256(denom), u256(fee), cfg)
    r = master(secret, APP, b"RFP022/release", rctx, account, index)
    rk = value(b"RFP022/release-key", r)
    hx = lambda b: "0x" + b.hex()
    out = {
        "name": name,
        "app": APP.decode(),
        "account_secret": hx(secret), "account": hx(account), "cfg_id": hx(cfg), "token": hx(token),
        "denom": str(denom), "index": index, "eth_dest": hx(eth_dest), "max_relay_fee": str(fee),
        "deposit_context": hx(dctx), "exit_context": hx(ectx), "release_context": hx(rctx),
        "deposit_master": hx(s), "claim_secret": hx(k), "commitment": hx(c),
        "claim_nullifier": hx(H(b"RFP022/nfc", k, account, token, u256(denom))), "recipient_opening": hx(H(b"RFP022/alias", k)),
        "reclaim_nullifier": hx(H(b"RFP022/nfrc", s)),
        "exit_master": hx(s2), "exit_secret": hx(k2), "exit_commitment": hx(e),
        "release_nullifier": hx(H(b"RFP022/nfr", k2)),
        "release_master": hx(r), "release_key": hx(rk),
    }
    if shutil.which("cast"):
        addr = subprocess.check_output(["cast", "wallet", "address", "--private-key", hx(rk)], text=True).strip()
        out["release_address"] = addr.lower()
    return out


cases = [
    case("all bytes 0x11..", bytes([0x11]) * 32, bytes([0xA1]) * 32, bytes([0xC0]) * 32, bytes([0xAA]) * 20,
         100_000_000, 0, bytes([0xBE]) * 20, 2_000_000),
    case("index 7, ETH, fee 0", bytes(range(32)), bytes(range(32, 64)), bytes(range(64, 96)), bytes(20),
         10**18, 7, bytes.fromhex("000000000000000000000000000000000000beef"), 0),
    case("v2 demo shape (dUSDC 100, cfg 0x7f1a..)", bytes([0xFE]) * 32,
         bytes.fromhex("1a2b" * 16),
         bytes.fromhex("7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11"),
         bytes.fromhex("ac207b9bfebf3ef7c26297ea6abadeeccc4aa71e"), 100_000_000, 3, bytes([0x22]) * 20, 2_000_000),
]
here = os.path.dirname(os.path.abspath(__file__))
doc = {
    "spec": "lez-bridge/client/src/kdf.rs, Names app scheme (Names Wallet app_secret, branch bridge-sdk)",
    "names_branch_vectors": {"account_secret": "0x" + "07" * 32, "account": "0x" + "09" * 32, "app": "lez_bridge",
                             "scope": "RFP022/deposit", "context": "0xabcd", "tag": "RFP022/ck",
                             "master_0": "0x" + NAMES_MASTER_0, "values": ["0x" + v for v in NAMES_VALUES]},
    "cases": cases,
}
with open(os.path.join(here, "kdf-names-v1.json"), "w") as f:
    json.dump(doc, f, indent=1)
    f.write("\n")
print(json.dumps(doc, indent=1))
