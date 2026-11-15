#!/usr/bin/env python3
"""Independent reference for the bridge KDF (lez-bridge/client, crate docs "Key derivation").

Standard library only (hmac, hashlib). Writes kdf-v1.json next to this file. The release
addresses are filled in by Foundry's `cast wallet address --private-key` (an independent
secp256k1 + keccak implementation) when `cast` is on PATH:

    python3 gen_kdf_vectors.py
"""
import hashlib, hmac, json, os, shutil, subprocess

H = lambda *parts: hashlib.sha256(b"".join(parts)).digest()
u256 = lambda v: v.to_bytes(32, "big")


def prk(account_secret):
    return hmac.new(b"RFP022/bridge/root/v1", account_secret, hashlib.sha256).digest()


def expand(p, cfg, kind, account, index, ctx):
    info = b"RFP022/bridge/key/v1" + cfg + bytes([kind]) + account + index.to_bytes(8, "big") + ctx
    return hmac.new(p, info + b"\x01", hashlib.sha256).digest()


def case(name, secret, account, cfg, token, denom, index, eth_dest, fee):
    p = prk(secret)
    s = expand(p, cfg, 1, account, index, token + u256(denom))
    k = H(b"RFP022/ck", s)
    c = H(b"RFP022/dep", k, account, token, u256(denom), cfg)
    rel = expand(p, cfg, 3, account, index, token + u256(denom) + u256(fee))
    s2 = expand(p, cfg, 2, account, index, token + u256(denom) + eth_dest + u256(fee))
    k2 = H(b"RFP022/ek", s2)
    e = H(b"RFP022/exit", k2, eth_dest, token, u256(denom), u256(fee), cfg)
    hx = lambda b: "0x" + b.hex()
    out = {
        "name": name,
        "account_secret": hx(secret), "account": hx(account), "cfg_id": hx(cfg), "token": hx(token),
        "denom": str(denom), "index": index, "eth_dest": hx(eth_dest), "max_relay_fee": str(fee),
        "prk": hx(p),
        "deposit_master": hx(s), "claim_secret": hx(k), "commitment": hx(c),
        "claim_nullifier": hx(H(b"RFP022/nfc", k, account, token, u256(denom))), "recipient_opening": hx(H(b"RFP022/alias", k)),
        "reclaim_nullifier": hx(H(b"RFP022/nfrc", s)),
        "exit_master": hx(s2), "exit_secret": hx(k2), "exit_commitment": hx(e),
        "release_nullifier": hx(H(b"RFP022/nfr", k2)),
        "release_key": hx(rel),
    }
    if shutil.which("cast"):
        addr = subprocess.check_output(["cast", "wallet", "address", "--private-key", hx(rel)], text=True).strip()
        out["release_address"] = addr.lower()
    return out


cases = [
    case("all bytes 0x11..", bytes([0x11]) * 32, bytes([0xA1]) * 32, bytes([0xC0]) * 32, bytes([0xAA]) * 20,
         100_000_000, 0, bytes([0xBE]) * 20, 2_000_000),
    case("index 7, ETH, fee 0", bytes(range(32)), bytes(range(32, 64)), bytes(range(64, 96)), bytes(20),
         10**18, 7, bytes.fromhex("000000000000000000000000000000000000beef"), 0),
    case("index 2^40, large denom", bytes([0xFE]) * 32, bytes([0x01]) * 32, bytes([0x7F]) * 32,
         bytes.fromhex("ac207b9bfebf3ef7c26297ea6abadeeccc4aa71e"), 10**28, 1 << 40, bytes([0x22]) * 20, 2 * 10**26),
]
here = os.path.dirname(os.path.abspath(__file__))
with open(os.path.join(here, "kdf-v1.json"), "w") as f:
    json.dump({"spec": "lez-bridge/client/src/lib.rs, Key derivation (v1)", "cases": cases}, f, indent=1)
    f.write("\n")
print(json.dumps(cases, indent=1))
