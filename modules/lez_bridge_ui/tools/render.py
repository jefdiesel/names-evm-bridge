#!/usr/bin/env python3
"""Renders qml/Main.qml to PNG files without Basecamp, in light and dark.

Usage: render.py <qml binary> <output dir> [scenario ...]

A stand-in for Basecamp's `logos` bridge answers every lez_bridge call with the replies
recorded from the real module against the real v2 demo (modules/fixtures/v2-demo/). Nothing
is asked of a wallet or a chain. Needs QML_IMPORT_PATH with the Logos design system's qml
folder, QT_PLUGIN_PATH, and QT_QPA_PLATFORM=offscreen. The theme is chosen through the app's
`forcedPalette`, never through Theme.setTheme, so the shared design-system.ini that every Logos
process reads is not touched; HOME is also pointed at a scratch folder while rendering.
"""
import json, os, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
QML = os.path.join(HERE, "..", "qml")
FX = os.path.join(HERE, "..", "..", "fixtures", "v2-demo")


def fixtures():
    out = {}
    for name in os.listdir(FX):
        if name.endswith(".json"):
            out[name[:-5]] = json.load(open(os.path.join(FX, name)))
    return out


BRIDGE = r"""
    width: %(width)s; height: %(height)s
    property var fx: (%(fx)s)
    property var mock: (%(mock)s)
    property var logos: QtObject {
        function reply(method, args) {
            var over = root.mock[method]
            if (over === "never") return null
            if (over !== undefined && over !== null && typeof over === "object") return over
            var key = method
            if (typeof over === "string") key = over
            if (method === "privacy_report") {
                var c = JSON.parse(args[0]); var ks = []
                for (var k in c) if (k !== "flow" && k !== "confirm_weaker" && k !== "anonymity_set" && c[k] === true) ks.push(k)
                ks.sort(); key = "privacy_report__" + [c.flow].concat(ks).join("__")
                var r = root.fx[key]
                if (r && c.confirm_weaker) { r = JSON.parse(JSON.stringify(r)); r.value.confirmed = true }
                return r || root.fx["privacy_report__" + c.flow]
            }
            return root.fx[key] || { success: false, value: null, error: "no recorded reply for " + key }
        }
        function callModuleAsync(module, method, args, callback, timeout) {
            var r = reply(method, args)
            if (r === null) return
            // Recorded replies say so, as the module's own fixture mode does.
            var shown = JSON.parse(JSON.stringify(r)); shown.fixture = true
            Qt.callLater(function() { callback(JSON.stringify(shown)) })
        }
    }
    Timer { interval: 400; running: true; onTriggered: { %(scenario)s; shot.start() } }
    Timer { id: shot; interval: %(wait)s; onTriggered: root.grabToImage(function(result) { result.saveToFile("%(out)s"); Qt.quit() }) }
"""

LEAF1 = 'root.deposits.deposits.filter(function(d) { return d.leaf_index === 1 })[0]'
LEAF0 = 'root.deposits.deposits.filter(function(d) { return d.leaf_index === 0 })[0]'

NAMES = {"config": "config__names", "accounts": "accounts__names", "wallet_status": "wallet_status"}

# name: (scenario JS, mock overrides, height)
SCENARIOS = {
    "home": ("root.go('home')", {}, 1000),
    "registry": ("root.go('registry')", {}, 2050),
    "deposit-plan": ("root.go('deposit'); root.depAmount = '1,250'; root.planDeposit()", {}, 1500),
    # The recorded request and recovery were made for the v2 demo account V2.
    "deposit-request": ("root.go('deposit'); root.depAccount = '2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC'; root.makeRequest(0)", {}, 1350),
    "pending": ("root.go('pending')", {}, 2450),
    "claim": ("root.openClaim(" + LEAF1 + ")", {}, 1250),
    "claim-now": ("root.openClaim(" + LEAF1 + "); nowPicker.start()", {}, 1350),
    "claim-reveal": ("root.openClaim(" + LEAF1 + "); root.claimReveal = true; root.claimChoicesChanged()", {}, 1250),
    "claim-reveal-open": ("root.openClaim(" + LEAF1 + "); root.claimReveal = true; root.claimChoicesChanged(); railOpener.start()", {}, 1700),
    "claim-refused": ("root.openClaim(" + LEAF0 + ")", {"claim_plan": "claim_plan__claimed"}, 800),
    "proving": ("root.showJob(root.fx.job__proving.value)", {"job": "job__proving", "jobs": "never"}, 1250),
    "job-done": ("root.showJob(root.fx.job.value)", {}, 1250),
    "holdings": ("root.go('holdings')", {}, 900),
    "redeem": ("root.go('redeem'); root.rdmTo = '0x37a46E7334Dd061E0C7E2298247EE9278d0d321F'; root.planRedeem()", {}, 1600),
    "exits": ("root.go('exits')", {}, 900),
    # The v3 vault (it can't pay out: its outbound state was deployed at zero): the v2-demo
    # recording with v3's release fields (fixtures/v2-demo/*__v3.json, hand-made).
    "registry-v3": ("root.go('registry')", {"registry": "registry__v3"}, 2050),
    "redeem-v3": ("root.go('redeem'); root.rdmTo = '0x37a46E7334Dd061E0C7E2298247EE9278d0d321F'; root.planRedeem()", {"registry": "registry__v3", "plan_redeem": "plan_redeem__v3"}, 1650),
    "exits-v3": ("root.go('exits')", {"registry": "registry__v3", "exits": "exits__v3"}, 950),
    "recover": ("root.go('recover'); root.recAccount = '2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC'; root.recover()", {}, 1100),
    "settings": ("root.go('settings')", {}, 1500),
    "admin": ("root.go('admin')", {}, 2000),
    "loading": ("root.go('registry')", {"registry": "never", "deposits": "never", "holdings": "never"}, 700),
    "error": ("root.go('registry')", {"registry": {"success": False, "value": None, "error": "the Ethereum RPC https://ethereum-sepolia-rpc.publicnode.com did not answer in time; try again, or choose another in Settings"}}, 700),
    "empty": ("root.go('pending')", {"deposits": {"success": True, "value": {"deposits": [], "as_of": 0}, "error": None}}, 700),
    # Names Wallet mode (config().wallet == "names"): hand-made replies in the module's shapes
    # (fixtures/v2-demo/*names*, *wallet*), since no Names Wallet ran for these.
    "settings-names": ("root.go('settings')", dict(NAMES, wallet_status="wallet_status__pending"), 1900),
    "deposit-needs": ("root.go('deposit'); root.depAccount = '2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC'; root.makeRequest(0, 0)", dict(NAMES, make_deposit_request="make_deposit_request__needs"), 1100),
    "claim-names": ("root.openClaim(" + LEAF1 + ")", dict(NAMES, claim_plan="claim_plan__names"), 1350),
    "proving-wallet-approve": ("root.showJob(root.fx.job__wallet_approve.value)", dict(NAMES, job="job__wallet_approve", jobs="never"), 900),
    "proving-wallet": ("root.showJob(root.fx.job__wallet.value)", dict(NAMES, job="job__wallet", jobs="never"), 1150),
    "proving-wallet-check": ("root.showJob(root.fx.job__wallet_check.value)", dict(NAMES, job="job__wallet_check", jobs="never"), 900),
    "recover-names": ("root.go('recover'); root.recAccount = '2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC'; root.recover()", dict(NAMES, recover="recover__names"), 1150),
    # Claim a deposit from a claim file (public account; the relay pays): hand-made replies in
    # relayed.rs's shapes (fixtures/v2-demo/relayed_*), since no wallet or chain ran for these.
    "claimfile": ("root.go('claimfile')", {}, 900),
    "claimfile-checked": ("root.go('claimfile'); root.rcSource = '/srv/claims/logos-bridge-claim-5c1e0f2d.json'; root.checkClaimFile()", {}, 1500),
    "claimfile-refused": ("root.go('claimfile'); root.rcSource = '/srv/claims/logos-bridge-claim-5c1e0f2d.json'; root.checkClaimFile()", {"relayed_claim_check": "relayed_claim_check__not_final"}, 800),
    "claimfile-needs": ("root.go('claimfile'); root.rcCheck = root.fx.relayed_claim_check.value; root.startRelayedClaim()", {"relayed_claim": "relayed_claim__needs", "wallet_request": "never"}, 1700),
    "claimfile-approve": ("root.go('claimfile'); root.rcCheck = root.fx.relayed_claim_check.value; root.rcJob = root.fx.relayed_claim_status__approve.value", {"relayed_claim_status": "never"}, 900),
    "claimfile-relay": ("root.go('claimfile'); root.rcCheck = root.fx.relayed_claim_check.value; root.rcJob = root.fx.relayed_claim_status__relay.value", {"relayed_claim_status": "never"}, 900),
    "claimfile-done": ("root.go('claimfile'); root.rcCheck = root.fx.relayed_claim_check.value; root.rcJob = root.fx.relayed_claim_status__done.value", {"relayed_claim_status": "never"}, 1000),
    "claimfile-wallet-timeout": ("root.go('claimfile'); root.rcCheck = root.fx.relayed_claim_check.value; root.startRelayedClaim()", {"relayed_claim": {"success": False, "value": None, "error": "Your Names Wallet didn't answer in time (it may be busy, or the Names app isn't installed or open). Nothing was sent; try again."}}, 1600),
    "claimfile-wallet-busy": ("root.go('claimfile'); root.rcCheck = root.fx.relayed_claim_check.value; root.startRelayedClaim()", {"relayed_claim": {"success": False, "value": None, "error": "Your Names Wallet is busy; nothing was sent; try again."}}, 1600),
    "claimfile-uncertain": ("root.go('claimfile'); root.rcCheck = root.fx.relayed_claim_check.value; root.startRelayedClaim()", {"relayed_claim": {"success": True, "error": None, "value": {"needs": "uncertain", "uncertain": "propose:x", "request": None, "since": 0, "lapses_at": 660000, "message": "Your Names Wallet didn't answer in time when this app proposed the claim, so it may be waiting in the Names app. This app can't follow that request: if the Names app shows a claim for this deposit, reject it there, then press Propose again (or wait 10 minutes and it lapses). Nothing was signed or sent."}}}, 1600),
    "claimfile-failed":("root.go('claimfile'); root.rcCheck = root.fx.relayed_claim_check.value; root.rcJob = root.fx.relayed_claim_status__failed.value", {"relayed_claim_status": "never"}, 950),
}
# Opens the privacy rail on the claim screen (a click, as a person would).
EXTRA = {"claim-now": "    Timer { id: nowPicker; interval: 300; onTriggered: { root.claimNow = true; root.claimChoicesChanged() } }\n", "claim-reveal-open": "    Timer { id: railOpener; interval: 600; onTriggered: { var r = root.findRail(root); if (r) r.open = true } }\n    function findRail(it) { if (it.hasOwnProperty('report') && it.hasOwnProperty('open') && it.visible) return it; for (var i = 0; i < it.children.length; i++) { var f = findRail(it.children[i]); if (f) return f } return null }\n"}


def main():
    qml, out_dir = sys.argv[1], sys.argv[2]
    wanted = sys.argv[3:] or list(SCENARIOS)
    source = open(os.path.join(QML, "Main.qml")).read()
    marker = "Item {\n    id: root\n"
    assert marker in source and "property int forcedMode: -1" in source, "Main.qml no longer starts the way the harness expects"
    os.makedirs(out_dir, exist_ok=True)
    fx = json.dumps(fixtures())
    home = tempfile.mkdtemp(prefix="render-home-", dir=os.environ.get("TMPDIR"))
    env = dict(os.environ, HOME=home)
    failed = 0
    for scenario in wanted:
        js, mock, height = SCENARIOS[scenario]
        for theme in ("dark", "light"):
            out = os.path.join(out_dir, "%s-%s.png" % (scenario, theme))
            bridge = BRIDGE % {"width": 1180, "height": height, "fx": fx, "mock": json.dumps(mock),
                               "scenario": js, "out": out, "wait": 2600 if scenario in ("claim-reveal-open", "claim-now") else 1500}
            bridge += EXTRA.get(scenario, "")
            with tempfile.NamedTemporaryFile("w", suffix=".qml", delete=False, dir=QML) as handle:
                themed = source.replace("property int forcedMode: -1", "property int forcedMode: %d" % (1 if theme == "dark" else 0), 1)
                handle.write(themed.replace(marker, marker + bridge, 1))
            try:
                result = subprocess.run([qml, handle.name], capture_output=True, text=True, timeout=90, env=env)
            except subprocess.TimeoutExpired:
                print(scenario, theme, "FAILED: did not quit within 90 s"); failed += 1
                continue
            finally:
                os.unlink(handle.name)  # never leave it beside Main.qml: the package copies qml/ whole
            problems = [l for l in result.stderr.splitlines() if l.strip() and "qt.qpa.fonts" not in l and "propagateSizeHints" not in l]
            ok = os.path.exists(out)
            failed += 0 if ok else 1
            print(scenario, theme, "ok" if ok else "FAILED", *problems[:8], sep="\n  " if problems else " ")
    sys.exit(1 if failed else 0)


main()
