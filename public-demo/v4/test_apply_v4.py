#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Offline tests of apply-v4-entries.py, on temporary copies of the two deployment lists.

    PYTHONDONTWRITEBYTECODE=1 python3 -I public-demo/v4/test_apply_v4.py      # from the root
The generated page list is also imported with node (if installed) to check DEPLOYMENTS[0].
"""
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
SCRIPT = HERE / "apply-v4-entries.py"
MOD = "modules/lez_bridge/rust-lib/deployments.json"
JS = "web/deposit/src/lib/deployments.js"
# Shapes only: made-up values, no deployment has them.
ENV = {
    "VAULT": "0x1111111111111111111111111111111111111111",
    "DESK": "0x2222222222222222222222222222222222222222",
    "GATE": "0x3333333333333333333333333333333333333333",
    "CFG_ID": "0x" + "44" * 32,
    "VAULT_CREATE_BLOCK": "12150000",
    "BP4": "4eaeK3yiqATXhpCEYEFMMtjUn1bS7HLxPwfrH31mXCvA",
    "TP": "5w6nTkftFFDy8mtdpnKpZVetGQJ2WCKdhmpJsAYArbU7",
    "DEF_ETH": "CUsiaS9iH4MYtbHbp6mpfqAPaHCa1py5UT5GN8vQYXzA",
    "DEF_USDC": "2MjYbRMi9VW81HGM5o6yptnCwvnY9NXfLLtFeqELkNXA",
}


class ApplyV4(unittest.TestCase):
    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        for rel in (MOD, JS):
            (self.tmp / rel).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(ROOT / rel, self.tmp / rel)
        self.before = {rel: (self.tmp / rel).read_text() for rel in (MOD, JS)}

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def run_apply(self, env, *extra):
        e = self.tmp / "v4.env"
        e.write_text("".join(f"{k}={v}\n" for k, v in env.items()))
        return subprocess.run([sys.executable, "-I", str(SCRIPT), "--env", str(e), "--root", str(self.tmp), *extra],
                              capture_output=True, text=True)

    def unchanged(self):
        for rel in (MOD, JS):
            self.assertEqual((self.tmp / rel).read_text(), self.before[rel], rel)

    def test_listed_not_default(self):
        r = self.run_apply(ENV)
        self.assertEqual(r.returncode, 0, r.stderr)
        m = json.loads((self.tmp / MOD).read_text())
        self.assertEqual(m["deployments"][0]["name"], "bridge-test-network-v4")
        self.assertEqual(m["deployments"][0]["vault_create_block"], 12150000)
        self.assertEqual(m["deployments"][0]["release"], "proof")
        self.assertEqual(m["default"], json.loads(self.before[MOD])["default"])
        self.assertEqual(len(m["deployments"]), len(json.loads(self.before[MOD])["deployments"]) + 1)
        js = (self.tmp / JS).read_text()
        self.assertIn("const SEPOLIA_V4 = {", js)
        self.assertIn("vault: '0x1111111111111111111111111111111111111111',", js)
        self.assertNotIn("<<V4:", js + (self.tmp / MOD).read_text())
        self.assertEqual(self.node_first(), "sepolia-demo-v3")
        # a second run is refused
        r2 = self.run_apply(ENV)
        self.assertNotEqual(r2.returncode, 0)
        self.assertIn("already", r2.stderr)

    def test_default(self):
        r = self.run_apply(ENV, "--default")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(json.loads((self.tmp / MOD).read_text())["default"], "bridge-test-network-v4")
        self.assertEqual(self.node_first(), "sepolia-v4")

    def test_refusals(self):
        for k, bad in (("VAULT", ""), ("VAULT", "0x" + "0" * 40), ("CFG_ID", "0x" + "0" * 64), ("CFG_ID", "0x1234"),
                       ("BP4", "<<V4:BP4>>"), ("VAULT_CREATE_BLOCK", "abc"), ("DESK", "0xZZ")):
            env = dict(ENV, **{k: bad})
            r = self.run_apply(env)
            self.assertNotEqual(r.returncode, 0, f"{k}={bad!r} accepted")
            self.unchanged()
        env = dict(ENV)
        del env["GATE"]
        self.assertNotEqual(self.run_apply(env).returncode, 0)
        self.unchanged()

    def test_check(self):
        ok = subprocess.run([sys.executable, "-I", str(SCRIPT), "--check", "--root", str(self.tmp)], capture_output=True, text=True)
        self.assertEqual(ok.returncode, 0, ok.stdout + ok.stderr)
        (self.tmp / JS).write_text(self.before[JS] + "\n// <<V4:VAULT>>\n")
        bad = subprocess.run([sys.executable, "-I", str(SCRIPT), "--check", "--root", str(self.tmp)], capture_output=True, text=True)
        self.assertEqual(bad.returncode, 1)

    def node_first(self):
        node = shutil.which("node")
        if not node:
            self.skipTest("no node")
        code = f"import('{(self.tmp / JS).as_uri()}').then(m => console.log(m.DEPLOYMENTS[0].id + ' ' + m.DEPLOYMENTS.length))"
        out = subprocess.run([node, "-e", code], capture_output=True, text=True)
        self.assertEqual(out.returncode, 0, out.stderr)
        first, n = out.stdout.split()
        self.assertEqual(int(n), 3)
        return first


if __name__ == "__main__":
    unittest.main(verbosity=2)
