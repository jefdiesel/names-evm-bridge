// node modules/lez_bridge_ui/logic.test.mjs
import assert from "node:assert/strict"
import * as L from "./qml/logic.mjs"

let n = 0
function t(name, f) { f(); n++; console.log("ok", name) }

t("ids are shortened in the middle", () => {
    assert.equal(L.shortId("2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC"), "2if1EJ…6HUC")
    assert.equal(L.shortId("0x12"), "0x12")
})
t("durations read naturally", () => {
    assert.equal(L.durationText(671), "11 min")
    assert.equal(L.durationText(86400), "24 h")
    assert.equal(L.clockText(292), "4:52")
})
t("privacy levels have words and shapes, not just colours", () => {
    for (const lvl of ["private", "reduced", "exposed"]) {
        const look = L.privacyLook(lvl)
        assert.ok(look.word.length > 3 && look.glyph.startsWith("shield"))
    }
    assert.notEqual(L.privacyLook("private").glyph, L.privacyLook("exposed").glyph)
})
t("phases are in the contract's order", () => {
    assert.deepEqual(L.PHASES.map(p => p.key), ["fetch", "simulate", "prove", "submit", "confirm"])
    assert.equal(L.phaseIndex("prove"), 2)
    assert.equal(L.phaseIndex("done"), 5)
})
t("a Names Wallet job starts with the person's approval", () => {
    assert.deepEqual(L.WALLET_PHASES.map(p => p.key), ["approve", "simulate", "prove", "submit", "confirm"])
    assert.equal(L.needsTitle("token_source"), "Approve the bridged tokens in your Names Wallet")
    assert.equal(L.errorTitle("wallet_check"), "Your Names Wallet stopped this call")
    assert.equal(L.requestWords("pending"), "Waiting for you in the Names app")
})
t("the estimate bar never fills before the job ends", () => {
    assert.equal(L.estimateFraction(2000, 671), 0.97)
    assert.equal(L.estimateFraction(0, 0), 0)
    assert.ok(Math.abs(L.estimateFraction(292, 671) - 0.435) < 0.001)
})
t("every CLI error code has a title", () => {
    for (const c of ["already_claimed", "anchor_not_finalized", "minting_frozen", "bad_denomination", "cap_exceeded", "token_not_registered", "proof_invalid"])
        assert.notEqual(L.errorTitle(c), "Something went wrong")
})
t("address and amount checks while typing", () => {
    assert.equal(L.ethAddressProblem(""), "")
    assert.match(L.ethAddressProblem("37a4"), /starts with 0x/)
    assert.match(L.ethAddressProblem("0x37a4"), /Too short/)
    assert.equal(L.ethAddressProblem("0x37a46E7334Dd061E0C7E2298247EE9278d0d321F"), "")
    assert.equal(L.amountProblem("1,250.5", 6), "")
    assert.match(L.amountProblem("1.1234567", 6), /6 decimals/)
    assert.match(L.amountProblem("1e6", 6), /digits/)
})
t("caps colour by use", () => {
    assert.equal(L.capTone(2.6), "ok")
    assert.equal(L.capTone(75), "warn")
    assert.equal(L.capTone(95), "bad")
    assert.equal(L.capWidth(0), 0)
    assert.equal(L.capWidth(0.2), 0.01)
})
t("choices carry only what was chosen", () => {
    assert.deepEqual(L.choices("claim", { reveal_recipient: true, claim_early: false }, false), { flow: "claim", reveal_recipient: true })
    assert.deepEqual(L.choices("claim", {}, true), { flow: "claim", confirm_weaker: true })
})
t("module replies unwrap, and a missing module is said plainly", () => {
    assert.deepEqual(L.unwrap('{"success":true,"value":1,"error":null}'), { ok: true, value: 1, fixture: false })
    assert.equal(L.unwrap('{"success":false,"value":null,"error":"no"}').error, "no")
    assert.match(L.unwrap("").error, /no answer/)
    assert.match(L.unwrap("{nope").error, /JSON/)
    assert.match(L.unwrap('"x"').error, /JSON/)
    // Basecamp 0.3.2: the module's JSON text, serialised again as a JSON string (LogosQmlBridge).
    const text = '{"success":true,"value":{"module_version":"0.1.0"},"error":null}'
    assert.deepEqual(L.unwrap(JSON.stringify(text)), { ok: true, value: { module_version: "0.1.0" }, fixture: false })
    assert.equal(L.unwrap(JSON.stringify('{"success":false,"value":null,"error":"no"}')).error, "no")
    // The bridge's own failures.
    assert.match(L.unwrap('{"error":"timeout","module":"lez_bridge","method":"config"}').error, /still working/)
    assert.match(L.unwrap('{"error":"Method not found","module":"lez_bridge","method":"relayed_claim_check"}').error, /older than this app/)
    assert.match(L.unwrap('{"error":"Module not connected","module":"lez_bridge"}').error, /not running/)
    assert.match(L.unwrap('{"something":1}').error, /missing or too old/)
})
t("stages map to words", () => {
    for (const s of L.STAGES) assert.notEqual(L.stageLook(s).word, s)
    assert.equal(L.dateText(1794571092).endsWith("UTC"), true)
    assert.equal(L.untilText(10, 20), "now")
})
t("the relayed claim says who signs and who pays, in the agreed words", () => {
    assert.equal(L.RELAY_WORDING, "The wallet signs the claim; the bridge's relay sends it and pays the fee.")
    assert.deepEqual(L.RELAY_PHASES.map(p => p.key), ["approve", "relay", "confirm"])
    assert.equal(L.needsTitle("programs"), "Approve the bridge's programs in your Names Wallet")
    assert.equal(L.needsTitle("public_account"), "Give this app the account this deposit is for")
})
t("every relayed-claim code has its own title, and the network is never called official", () => {
    const codes = ["bad_file", "private_account", "other_chain", "other_vault", "cfg_mismatch", "not_in_vault", "not_final", "root_not_attested",
        "already_claimed", "relay_cap", "relay_funds", "relay_once", "relay_refused", "relay_busy", "wrong_account", "busy", "leaves_mismatch", "no_relay", "wrong_chain"]
    const titles = new Set()
    for (const c of codes) { assert.notEqual(L.relayErrorTitle(c), "Something went wrong", c); titles.add(L.relayErrorTitle(c)) }
    assert.ok(titles.size >= codes.length - 2, "titles are specific")
    for (const c of codes) assert.doesNotMatch(L.relayErrorTitle(c), /official/i)
    assert.equal(L.networkLabel("the official network"), "LEZ 0.4 · bridge test network")
    assert.equal(L.networkLabel("LEZ 0.4 · bridge test network"), "LEZ 0.4 · bridge test network")
    assert.ok(L.relayRetryable("not_final") && L.relayRetryable("root_not_attested") && !L.relayRetryable("already_claimed"))
})
t("a claim file is pasted or chosen", () => {
    assert.equal(L.claimSource("").kind, "empty")
    assert.equal(L.claimSource('{"format":"logos-bridge-claim"}').problem, "")
    assert.match(L.claimSource('{"a":1}').problem, /not a bridge claim file/)
    assert.equal(L.claimSource("file:///srv/x/claim.json").kind, "path")
    assert.match(L.claimSource("0xabc").problem, /Paste the whole claim file/)
})
t("a restarting module host is retried for reads only; a busy RPC is a wait", () => {
    const r = L.unwrap('{"error":"Module source unavailable","module":"lez_bridge","message":"Module process crashed"}')
    assert.equal(r.transient, true)
    assert.equal(L.unwrap('{"error":"timeout","module":"lez_bridge"}').transient, undefined)
    for (const m of ["relayed_claim", "claim", "redeem", "admin_action", "make_deposit_request", "relayed_claim_cancel"]) assert.ok(!L.RETRY_SAFE.includes(m), m)
    for (const m of ["config", "registry", "relayed_claim_check", "relayed_claim_status"]) assert.ok(L.RETRY_SAFE.includes(m), m)
    assert.ok(L.isBusy("Sepolia's RPC is busy (rate limited, HTTP 429); retrying shortly."))
    assert.ok(L.isBusy("https://sepolia.gateway.tenderly.co: status code 429"))
    assert.ok(!L.isBusy("the Ethereum RPC did not answer in time"))
})
t("a busy Names Wallet is a wait, never 'install the Names app'", () => {
    const busy = "Your Names Wallet is busy; nothing was sent; try again."
    assert.equal(L.walletProblem(busy), "busy")
    assert.equal(L.walletProblemTitle(busy), "Your Names Wallet is busy")
    assert.equal(L.walletProblemTitle("Your Names Wallet didn't answer in time (it may be busy, or the Names app isn't installed or open). Nothing was sent; try again."), "Your Names Wallet didn't answer")
    assert.equal(L.walletProblemTitle("Your Names Wallet isn't reachable here (is the Names app installed and open?). Nothing was sent; try again."), "Your Names Wallet isn't reachable")
    assert.equal(L.walletProblemTitle("the bridge deployment changed"), "The bridge app could not do that")
    assert.doesNotMatch(L.walletCheckNote("busy"), /install/i)
    assert.doesNotMatch(L.walletCheckNote("no_answer"), /install/i)
    assert.match(L.walletCheckNote("wallet"), /install the Names app/)
    assert.equal(L.walletCheckNote(null), "")
    assert.equal(L.needsTitle("uncertain"), "Look in your Names app first")
    assert.equal(L.relayErrorTitle("wallet_no_answer"), "Your Names Wallet didn't answer")
    assert.equal(L.relayErrorTitle("no_deposit"), "No such deposit on this computer")
})
t("the module's busy answer is a retry, however Basecamp encodes it", () => {
    const env = JSON.stringify({ success: false, busy: true, value: null, error: "The LEZ Bridge is busy with another call; nothing was done. Ask again in a moment." })
    for (const reply of [env, JSON.stringify(env)]) {
        const r = L.unwrap(reply)
        assert.equal(r.ok, false); assert.equal(r.transient, true); assert.equal(r.busy, true)
        assert.match(r.error, /busy/)
    }
    assert.equal(L.unwrap(JSON.stringify({ success: false, value: null, error: "no" })).transient, undefined)
})
console.log(n, "passed")
