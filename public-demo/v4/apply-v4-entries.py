#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Fill the v4 entries of the two deployment lists from the bring-up's values (V4-PLAN step C1).

    python3 -I public-demo/v4/apply-v4-entries.py --env <v4.env> [--default] [--root <checkout>]
    python3 -I public-demo/v4/apply-v4-entries.py --check [--root <checkout>]

--env: KEY=VALUE lines (VAULT, VAULT_CREATE_BLOCK, DESK, CFG_ID, BP4, TP, DEF_ETH, DEF_USDC, GATE),
the values read back from Sepolia and the LEZ chain during bring-up. Every value is checked for
its shape (an Ethereum address, a 32-byte hex, a LEZ account id, a block number); a missing or
malformed one stops the script before any file is written. It inserts the v4 entry FIRST in
modules/lez_bridge/rust-lib/deployments.json and as SEPOLIA_V4 in web/deposit/src/lib/deployments.js
(v2 and v3 stay listed), and refuses if v4 is already there. --default also makes v4 the module's
`default` and the page's LIVE_DEFAULT (the switch; without it v4 is listed but not the default).
--check: exits 1 if either list holds a '<<V4:' placeholder (run before any build or publish).
Afterwards: `cd web/deposit && npm test && node tools/manifest.mjs`, and the module's tests.
stdlib only; writes nothing on any error.
"""
import argparse
import json
import pathlib
import re
import sys

HERE = pathlib.Path(__file__).resolve().parent
MARK = "<<V4:"
SHAPES = {
    "VAULT": r"0x[0-9a-fA-F]{40}",
    "DESK": r"0x[0-9a-fA-F]{40}",
    "GATE": r"0x[0-9a-fA-F]{40}",
    "CFG_ID": r"0x[0-9a-fA-F]{64}",
    "VAULT_CREATE_BLOCK": r"[1-9][0-9]{6,9}",
    "BP4": r"[1-9A-HJ-NP-Za-km-z]{32,44}",
    "TP": r"[1-9A-HJ-NP-Za-km-z]{32,44}",
    "DEF_ETH": r"[1-9A-HJ-NP-Za-km-z]{32,44}",
    "DEF_USDC": r"[1-9A-HJ-NP-Za-km-z]{32,44}",
}
ZERO_ADDR = re.compile(r"0x0{40}$")
ZERO_32 = re.compile(r"0x0{64}$")
INT_FIELDS = {"vault_create_block", "deployBlock"}
JS_ANCHOR = "/**\n * THE ONE LINE that picks the page's deployment"
JS_LIST = re.compile(r"^export const DEPLOYMENTS = .*$", re.M)
JS_LIVE = re.compile(r"^const LIVE_DEFAULT = '[^']*';$", re.M)


def fail(msg):
    print(f"apply-v4-entries: {msg}", file=sys.stderr)
    sys.exit(1)


def read_env(path):
    vals = {}
    for line in pathlib.Path(path).read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        vals[k.strip()] = v.strip().split()[0] if v.strip() else ""
    for k, shape in SHAPES.items():
        v = vals.get(k, "")
        if MARK in v or not re.fullmatch(shape, v):
            fail(f"{k} is missing or not a {shape} ({v!r})")
        if ZERO_ADDR.fullmatch(v) or ZERO_32.fullmatch(v):
            fail(f"{k} is zero")
    return vals


def fill(obj, vals):
    if isinstance(obj, dict):
        out = {}
        for k, v in obj.items():
            v = fill(v, vals)
            if k in INT_FIELDS:
                v = int(v)
            out[k] = v
        return out
    if isinstance(obj, list):
        return [fill(x, vals) for x in obj]
    if isinstance(obj, str):
        s = re.sub(r"<<V4:([A-Z_0-9]+)>>", lambda m: vals[m.group(1)] if m.group(1) in vals else m.group(0), obj)
        if MARK in s:
            fail(f"placeholder left in {obj!r}")
        return s
    return obj


def js_value(v):
    if v is None:
        return "null"
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(v)
    return "'" + str(v).replace("\\", "\\\\").replace("'", "\\'") + "'"


def js_entry(page):
    const = page["const"]
    lines = [
        "/**",
        " * v4 (public-demo/V4-PLAN.md; vault/DEPLOYMENTS.md, v4): RISC Zero's Groth16 verifier 3.1.0 checks every",
        " * update and release proof on Sepolia, behind an UpdateGatedVerifier (updates only from its submitter: the",
        " * LEZ finality is the operator's word until a finality proof exists). Written by public-demo/v4/apply-v4-entries.py.",
        " */",
        f"const {const} = {{",
    ]
    for k, v in page.items():
        if k == "const":
            continue
        if k == "vault" or k == "cfgId":
            v = v.lower()
        lines.append(f"    {k}: {js_value(v)},")
    lines.append("    tokens: SEPOLIA_DEMO_V2.tokens,")
    lines.append("};")
    return "\n".join(lines) + "\n\n"


def check(root):
    bad = 0
    for p in (root / "modules/lez_bridge/rust-lib/deployments.json", root / "web/deposit/src/lib/deployments.js"):
        if MARK in p.read_text():
            print(f"PLACEHOLDER in {p}", file=sys.stderr)
            bad += 1
    print("no v4 placeholder in either deployment list" if not bad else f"{bad} file(s) hold a placeholder")
    return 1 if bad else 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--env")
    ap.add_argument("--root", default=str(HERE.parent.parent))
    ap.add_argument("--default", action="store_true")
    ap.add_argument("--check", action="store_true")
    a = ap.parse_args()
    root = pathlib.Path(a.root)
    if a.check:
        sys.exit(check(root))
    if not a.env:
        fail("--env <v4.env> is required")
    vals = read_env(a.env)
    tpl = json.loads((HERE / "deployment-v4.template.json").read_text())
    mod = fill(tpl["module"], vals)
    page = fill(tpl["page"], vals)

    mpath = root / "modules/lez_bridge/rust-lib/deployments.json"
    jpath = root / "web/deposit/src/lib/deployments.js"
    m = json.loads(mpath.read_text())
    if any(d["name"] == mod["name"] for d in m["deployments"]):
        fail(f"{mod['name']} is already in {mpath}")
    if any(d.get("vault", "").lower() == mod["vault"].lower() for d in m["deployments"]):
        fail("that vault is already listed")
    m["deployments"].insert(0, mod)
    if a.default:
        m["default"] = mod["name"]

    js = jpath.read_text()
    if f"const {page['const']} =" in js or page["vault"].lower() in js.lower():
        fail(f"v4 is already in {jpath}")
    if js.count(JS_ANCHOR) != 1 or len(JS_LIST.findall(js)) != 1 or len(JS_LIVE.findall(js)) != 1:
        fail(f"{jpath} does not have the expected LIVE_DEFAULT / DEPLOYMENTS lines (edit by hand)")
    js = js.replace(JS_ANCHOR, js_entry(page) + JS_ANCHOR)
    js = JS_LIST.sub(
        "const ALL_DEPLOYMENTS = [SEPOLIA_V4, SEPOLIA_DEMO_V3, SEPOLIA_DEMO_V2];\n"
        "export const DEPLOYMENTS = [...ALL_DEPLOYMENTS.filter((d) => d.id === LIVE_DEFAULT), ...ALL_DEPLOYMENTS.filter((d) => d.id !== LIVE_DEFAULT)];",
        js,
    )
    if a.default:
        js = JS_LIVE.sub(f"const LIVE_DEFAULT = '{page['id']}';", js)
    for text, where in ((json.dumps(m), mpath), (js, jpath)):
        if MARK in text:
            fail(f"placeholder left for {where}")
    mpath.write_text(json.dumps(m, indent=1, ensure_ascii=False) + "\n")
    jpath.write_text(js)
    print(f"v4 entry written to {mpath} and {jpath}" + (" (v4 is the default)" if a.default else " (listed, not the default)"))


if __name__ == "__main__":
    main()
