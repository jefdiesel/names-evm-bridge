#!/usr/bin/python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""borsh encodings of the raw messages e2e-security.sh sends (stdlib only; prints hex).

  mint DEF TO AMOUNT                        token Message::Mint (tag 2; same bytes before and after the fixes)
  mint-account DEF TO AMOUNT                same bytes as mint (an account-authority control)
  newdef-account NAME AUTH SUPPLY_TO        token NewFungibleDefinition, supply 0, Some(Authority::Account(AUTH))
  setauth-old DEF NEW                       token SetAuthority, before the fixes: Option<AccountId>
  setauth-account DEF NEW                   token SetAuthority, after: Some(Authority::Account(NEW))
  setauth-none DEF                          token SetAuthority { new_authority: None }
  initialize ADMIN VAULT EVM LEZCHAIN TP FREEZER ATTESTOR   bridge Instruction::Initialize (variant 6)
  init-legacy VAULT EVM LEZCHAIN ZONE TP FREEZER ATTESTOR   bridge Instruction::Init (variant 0, retired)

Account ids are base58; VAULT is 0x + 20 bytes; LEZCHAIN and ZONE are 0x + 32 bytes.
"""
import hashlib, sys

B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58d(s):
    n = 0
    for c in s:
        n = n * 58 + B58.index(c)
    raw = n.to_bytes((n.bit_length() + 7) // 8, "big") if n else b""
    raw = b"\0" * (len(s) - len(s.lstrip("1"))) + raw
    if len(raw) != 32:
        raise SystemExit(f"{s}: not a 32-byte account id")
    return raw


def b58e(b):
    n = int.from_bytes(b, "big")
    out = ""
    while n:
        n, r = divmod(n, 58)
        out = B58[r] + out
    return "1" * (len(b) - len(b.lstrip(b"\0"))) + out


def hx(s, n):
    b = bytes.fromhex(s[2:] if s.startswith("0x") else s)
    if len(b) != n:
        raise SystemExit(f"{s}: not {n} bytes")
    return b


u64 = lambda v: int(v).to_bytes(8, "little")
u128 = lambda v: int(v).to_bytes(16, "little")


def main(cmd, *a):
    if cmd == "mint":
        out = bytes([2]) + b58d(a[0]) + b58d(a[1]) + u128(a[2])
    elif cmd == "mint-account":
        out = bytes([2]) + b58d(a[0]) + b58d(a[1]) + u128(a[2])
    elif cmd == "newdef-account":
        name = a[0].encode()
        out = bytes([1]) + len(name).to_bytes(4, "little") + name + u128(0) + b"\x01\x00" + b58d(a[1]) + b58d(a[2])
    elif cmd == "setauth-old":
        out = bytes([4]) + b58d(a[0]) + b"\x01" + b58d(a[1])
    elif cmd == "setauth-account":
        out = bytes([4]) + b58d(a[0]) + b"\x01\x00" + b58d(a[1])
    elif cmd == "setauth-none":
        out = bytes([4]) + b58d(a[0]) + b"\x00"
    elif cmd == "initialize":
        admin, vault, evm, chain, tp, fr, at = a
        out = b"\x00\x06" + b58d(admin) + hx(vault, 20) + u64(evm) + hx(chain, 32) + b58d(tp) + b58d(fr) + b58d(at)
    elif cmd == "init-legacy":
        vault, evm, chain, zone, tp, fr, at = a
        out = b"\x00\x00" + hx(vault, 20) + u64(evm) + hx(chain, 32) + hx(zone, 32) + b58d(tp) + b58d(fr) + b58d(at)
    else:
        raise SystemExit(__doc__)
    print(out.hex())


if __name__ == "__main__":
    main(*sys.argv[1:])
