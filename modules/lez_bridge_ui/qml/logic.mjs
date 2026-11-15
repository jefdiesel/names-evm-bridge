// Pure helpers for the LEZ Bridge mini-app. No arithmetic on amounts happens here: the
// lez_bridge module returns every amount as decimal text already formatted (`*_text`), because
// values reach 1e18 and more. These functions only choose words, tones and shapes.

export function shortId(text) {
    var s = String(text || "")
    if (s.length <= 14) return s
    return s.slice(0, 6) + "…" + s.slice(-4)
}

export function durationText(secs) {
    var s = Math.max(0, Math.floor(Number(secs) || 0))
    if (s < 90) return Math.max(1, s) + " s"
    if (s < 5400) return Math.round(s / 60) + " min"
    if (s < 172800) return Math.round(s / 3600) + " h"
    return Math.round(s / 86400) + " days"
}

export function clockText(secs) {
    var s = Math.max(0, Math.floor(Number(secs) || 0))
    var m = Math.floor(s / 60), r = s % 60
    return m + ":" + (r < 10 ? "0" : "") + r
}

// The privacy indicator's three states. Colour is never the only signal: each has its own
// word and glyph shape.
export function privacyLook(level) {
    if (level === "private") return { tone: "ok", word: "Private", glyph: "shield-full" }   // ink
    if (level === "reduced") return { tone: "warn", word: "Less private", glyph: "shield-half" }
    if (level === "exposed") return { tone: "bad", word: "Not private", glyph: "shield-open" }
    return { tone: "muted", word: "Checking privacy…", glyph: "shield-half" }
}

export const STAGES = ["requested", "included", "finalized", "anchored", "claimable", "claimed"]

export function stageLook(stage) {
    switch (stage) {
    case "requested": return { tone: "muted", word: "Not sent yet" }
    case "included": return { tone: "ink", word: "On Ethereum" }
    case "finalized": return { tone: "ink", word: "Final on Ethereum" }
    case "anchored": return { tone: "ink", word: "Claimable, best later" }
    case "claimable": return { tone: "solid", word: "Ready to claim" }
    case "claimed": return { tone: "muted", word: "Claimed" }
    }
    return { tone: "muted", word: String(stage || "unknown") }
}

export const PHASES = [
    { key: "fetch", word: "Fetch", hint: "Reads the deposit list, the storage proof and the anchor" },
    { key: "simulate", word: "Simulate", hint: "Runs the claim on this computer and checks it would pass" },
    { key: "prove", word: "Prove", hint: "Makes the zero-knowledge proof (the long part)" },
    { key: "submit", word: "Submit", hint: "Sends the proved transaction to the sequencer" },
    { key: "confirm", word: "Confirm", hint: "Waits for it to be included" }
]

// A Names Wallet job: the person approves in the Names app, then the wallet simulates again,
// proves on this computer, sends and waits for the block.
export const WALLET_PHASES = [
    { key: "approve", word: "Approve", hint: "Your Names app shows what it does and asks you" },
    { key: "simulate", word: "Check", hint: "Your wallet runs it again and stops if anything changed" },
    { key: "prove", word: "Prove", hint: "Your wallet makes the proof on this computer (the long part)" },
    { key: "submit", word: "Submit", hint: "Your wallet sends the proved transaction" },
    { key: "confirm", word: "Confirm", hint: "Waits for it to be included" }
]

// What the person must approve in the Names app before the bridge can go on (module `needs`).
export function needsTitle(needs) {
    switch (needs) {
    case "connect": return "Connect your Names Wallet"
    case "private_account": return "Give this app a private account"
    case "token_source": return "Approve the bridged tokens in your Names Wallet"
    case "secret": return "Approve this app's secrets in your Names Wallet"
    case "public_account": return "Give this app the account this deposit is for"
    case "programs": return "Approve the bridge's programs in your Names Wallet"
    case "network": return "Switch your Names Wallet to LEZ 0.4"
    case "uncertain": return "Look in your Names app first"
    }
    return "Approve in the Names app"
}

// What a Names Wallet failure the module passed on means (module wallet.rs BUSY_TEXT,
// NO_ANSWER_TEXT, UNREACHABLE_TEXT): busy is "ask again shortly", never "install the Names app".
export function walletProblem(text) {
    var s = String(text || "")
    if (/Names Wallet is busy/i.test(s)) return "busy"
    if (/Names Wallet didn't answer/i.test(s)) return "no_answer"
    if (/Names Wallet isn't reachable/i.test(s)) return "unreachable"
    return ""
}
export function walletProblemTitle(text) {
    switch (walletProblem(text)) {
    case "busy": return "Your Names Wallet is busy"
    case "no_answer": return "Your Names Wallet didn't answer"
    case "unreachable": return "Your Names Wallet isn't reachable"
    }
    return "The bridge app could not do that"
}
// The line under a claim's "Send to my Names Wallet" while the wallet can't say yet whether it is
// connected (relayed.rs relay_wallet_view `needs`).
export function walletCheckNote(needs) {
    switch (needs) {
    case "wallet": return "The Names Wallet is not answering here: install the Names app (0.17 or later) and open it, then send this."
    case "busy": return "Your Names Wallet is busy right now (another call is running in it). You can send this; it is asked again when you do."
    case "no_answer": return "Your Names Wallet didn't answer in time. You can send this; it is asked again when you do."
    }
    return ""
}

// A Names Wallet request's state, in words.
export function requestWords(state) {
    switch (state) {
    case "pending": return "Waiting for you in the Names app"
    case "approved": case "done": return "Approved"
    case "rejected": return "You rejected it in the Names app"
    case "expired": return "Nobody answered it within 10 minutes"
    case "cancelled": return "Cancelled"
    }
    return ""
}

export function phaseIndex(phase) {
    if (phase === "done") return PHASES.length
    for (var i = 0; i < PHASES.length; i++) if (PHASES[i].key === phase) return i
    return 0
}

// Share of the estimate used so far, for a bar labelled as an estimate. Never shows full
// until the job says it finished.
export function estimateFraction(elapsed, estimate) {
    var e = Number(estimate) || 0
    if (e <= 0) return 0
    return Math.max(0, Math.min(0.97, (Number(elapsed) || 0) / e))
}

export function jobLook(state) {
    switch (state) {
    case "running": return { tone: "accent", word: "Running" }
    case "paused": return { tone: "warn", word: "Paused" }
    case "done": return { tone: "solid", word: "Finished" }
    case "failed": return { tone: "bad", word: "Stopped with an error" }
    case "cancelled": return { tone: "muted", word: "Cancelled" }
    case "unknown": return { tone: "warn", word: "Outcome unknown" }
    }
    return { tone: "muted", word: String(state || "") }
}

// Titles for the CLI's error codes (CLI-CONTRACT.md §1, RFP-022 Usability #8). The message
// below the title is the local diagnosis, which never leaves this computer.
export function errorTitle(code) {
    var t = {
        cfg_mismatch: "This deposit is for another bridge",
        token_not_registered: "Token not supported",
        minting_frozen: "Claims are paused",
        minting_frozen_token: "Claims of this token are paused",
        burning_frozen: "Redemptions of this token are paused",
        bad_denomination: "Not a valid amount",
        wrong_amount: "Not a valid amount",
        wrong_leaf: "The deposit does not match",
        proof_invalid: "The proof did not verify",
        root_not_attested: "Not final enough yet",
        anchor_not_finalized: "Not final enough yet",
        already_claimed: "Already claimed",
        float_empty: "The bridge's fee float is empty",
        not_loopback: "Sequencer not allowed",
        wallet_missing: "No wallet",
        reverted: "Included, then refused",
        cap_exceeded: "Over the bridge's limit",
        cli_error: "The bridge program stopped",
        job_error: "The job stopped",
        wallet_check: "Your Names Wallet stopped this call",
        wallet_failed: "Your Names Wallet could not finish it",
        wallet_rejected: "Nothing was sent"
    }
    return t[code] || "Something went wrong"
}

export function capTone(pct) {
    var p = Number(pct)
    if (isNaN(p)) return "muted"
    if (p >= 90) return "bad"
    if (p >= 70) return "warn"
    return "ok"
}

export function capWidth(pct) {
    var p = Number(pct)
    if (isNaN(p) || p <= 0) return 0
    return Math.max(0.01, Math.min(1, p / 100))
}

// A quick shape check while typing; the module does the checksum.
export function ethAddressProblem(text) {
    var s = String(text || "").trim()
    if (s === "") return ""
    if (!/^0x[0-9a-fA-F]*$/.test(s)) return "An Ethereum address starts with 0x, then hex digits."
    if (s.length < 42) return "Too short: an address has 40 hex digits after 0x."
    if (s.length > 42) return "Too long: an address has 40 hex digits after 0x."
    return ""
}

export function amountProblem(text, decimals) {
    var s = String(text || "").replace(/[, _]/g, "").trim()
    if (s === "") return ""
    if (!/^\d*\.?\d*$/.test(s) || s === ".") return "Use digits, with one decimal point."
    var frac = s.indexOf(".") >= 0 ? s.split(".")[1].length : 0
    if (frac > decimals) return "This token has " + decimals + " decimals."
    return ""
}

// The privacy_report choice object for a set of chosen overrides.
export function choices(flow, chosen, confirmed) {
    var c = { flow: flow }
    for (var k in chosen) if (chosen[k]) c[k] = true
    if (confirmed) c.confirm_weaker = true
    return c
}

export function freezeWord(frozen) {
    if (frozen === true) return "Frozen"
    if (frozen === false) return "Open"
    return "Unknown"
}

export function plural(n, one, many) {
    return n + " " + (Number(n) === 1 ? one : many)
}

export function dateText(epochSecs) {
    var n = Number(epochSecs)
    if (!n) return ""
    var d = new Date(n * 1000)
    var mm = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
    var hh = d.getUTCHours(), mi = d.getUTCMinutes()
    return d.getUTCDate() + " " + mm[d.getUTCMonth()] + " " + (hh < 10 ? "0" : "") + hh + ":" + (mi < 10 ? "0" : "") + mi + " UTC"
}

export function untilText(epochSecs, nowSecs) {
    var d = Number(epochSecs) - Number(nowSecs)
    if (!(d > 0)) return "now"
    return "in " + durationText(d)
}

// The reply text of a module call: {success, value, error}; anything else is a broken module.
// A module reply as Basecamp hands it to QML. Since Basecamp 0.3.2 the bridge serialises every
// result as JSON (LogosQmlBridge::serializeResultForTesting, logos-view-module-runtime), so this
// module's own JSON text arrives as a JSON string literal: parse up to twice (as names_ui does).
// The bridge's own failures are an object with `error` and no `success`: say which.
export function unwrap(reply) {
    if (reply === undefined || reply === null || String(reply) === "") return { ok: false, error: "The lez_bridge module gave no answer; it may still be starting. Try again." }
    var r = reply
    for (var depth = 0; depth < 2 && typeof r === "string"; depth++) {
        try { r = JSON.parse(r) } catch (e) { return { ok: false, error: "The lez_bridge module answered something that isn't JSON." } }
    }
    if (r && typeof r === "object" && "success" in r) {
        if (r.success) return { ok: true, value: r.value, fixture: !!r.fixture }
        // The module's own busy answer (its dispatch, scaffold_patch.rs): not taken, ask again.
        if (r.busy === true) return { ok: false, transient: true, busy: true, error: String(r.error || "The LEZ Bridge is busy; ask again in a moment.") }
        return { ok: false, error: String(r.error || "The module did not say why.") }
    }
    var said = r && typeof r === "object" ? String(r.error || "") + (r.message ? ": " + r.message : "") : String(r)
    if (/timeout/i.test(said)) return { ok: false, error: "The lez_bridge module is still working on an earlier request (or starting). Try again in a moment." }
    if (/method not found|unknown_method/i.test(said)) return { ok: false, error: "The lez_bridge module is older than this app: install the matching lez_bridge package. (" + said + ")" }
    if (/not connected|unavailable|not available|crashed|not reachable|waiting for replica/i.test(said)) return { ok: false, transient: true, error: "The lez_bridge module is restarting or not running; this app reconnects by itself. If it stays like this, close and reopen the app. (" + said + ")" }
    return { ok: false, error: "The lez_bridge module is missing or too old. Install it from the Package Manager." + (said ? " (" + said.slice(0, 200) + ")" : "") }
}

// Thousands separators for a whole number given as text (no arithmetic: values can exceed
// what a JavaScript number holds exactly).
export function group(text) {
    var s = String(text === undefined || text === null ? "" : text)
    if (!/^\d+$/.test(s)) return s
    return s.replace(/\B(?=(\d{3})+(?!\d))/g, ",")
}

export function capitalized(text) {
    var s = String(text || "")
    return s.charAt(0).toUpperCase() + s.slice(1)
}

// Megabytes as gigabytes the way the measurements are quoted (9,403 MB = 9.4 GB).
export function gb(mb) {
    var n = Number(mb) || 0
    return (n / 1000).toFixed(1) + " GB"
}

// What causes each privacy override, and what undoes it, for the indicator's headline.
export const CAUSES = {
    reveal_recipient: ["you mint to your own account by name", "choose the one-time address", "minting to my own account by name"],
    claim_early: ["you claim before the suggested time", "choose Wait", "claiming before the suggested time"],
    public_account: ["the LEZ account is public", "choose a private account", "using a public account"],
    all_at_once: ["every piece goes at once", "spread the pieces out", "sending every piece at once"],
    odd_amount: ["you keep an amount that is not a denomination", "bridge whole denominations", "keeping an odd amount"],
    self_relay: ["you submit the release yourself", "choose a relayer", "submitting the release myself"],
    known_recipient: ["the recipient address is linked to you", "use a fresh address", "a recipient linked to me"]
}

// "I accept that …": the chosen overrides as noun phrases.
export function acceptText(report, what) {
    var w = report && report.weakened_by ? report.weakened_by : []
    var parts = []
    for (var i = 0; i < w.length; i++) parts.push(CAUSES[w[i].key] ? CAUSES[w[i].key][2] : w[i].title.toLowerCase())
    return "I accept that " + parts.join(" and ") + " makes " + what + " easier to link to me."
}

// The indicator's sentence: what it is, and for anything weaker, why and how to undo it.
export function privacyHeadline(report) {
    if (!report) return "Checking what this would make public…"
    if (report.level === "private")
        return "Nothing published links the two sides. Amount, token and timing are visible."
    var why = [], undo = []
    var w = report.weakened_by || []
    for (var i = 0; i < w.length; i++) {
        var c = CAUSES[w[i].key]
        if (c) { why.push(c[0]); undo.push(c[1]) }
    }
    if (why.length === 0) return report.headline || ""
    return "Because " + why.join(" and ") + ". To make it private, " + undo.join(" and ") + "."
}

// Plain advice for an error code; the CLI's own words are shown under it as details.
export function errorAdvice(code) {
    var t = {
        already_claimed: "This deposit was claimed already. Nothing was sent.",
        anchor_not_finalized: "The attestation on LEZ has not reached this deposit yet. It becomes claimable after the next attestation update.",
        root_not_attested: "The deposit is not attested on LEZ yet. Try again after the next attestation update.",
        minting_frozen: "The bridge's admin has paused claims for every token. Your deposit stays claimable; try again once claims reopen.",
        minting_frozen_token: "The bridge's admin has paused claims of this token. Your deposit stays claimable; try again once claims reopen.",
        burning_frozen: "Redemptions of this token are paused. Nothing was burned.",
        wrong_leaf: "This computer's record of the deposit does not match the vault. Nothing was sent.",
        cfg_mismatch: "This deposit was made for another bridge deployment. Switch the deployment in Settings.",
        float_empty: "The bridge's fee float cannot sponsor another claim right now. Nothing was proven or sent.",
        proof_invalid: "The Ethereum proof for this deposit did not check out. Nothing was sent.",
        token_not_registered: "This token is not supported by this bridge.",
        bad_denomination: "That is not one of the amounts this token moves in.",
        wrong_amount: "That is not one of the amounts this token moves in.",
        reverted: "The transaction was included and then refused, because something changed after it was checked. Check your holdings before trying again."
    }
    return t[code] || ""
}

// Call data as a function selector and 32-byte words, one per line, for checking by eye.
export function calldataLines(hex) {
    var h = String(hex || "").replace(/^0x/, "")
    if (h.length < 8) return h
    var out = ["0x" + h.slice(0, 8) + "  (function)"]
    for (var i = 8; i < h.length; i += 64) out.push(h.slice(i, i + 64))
    return out.join("\n")
}

// ── Claim a deposit (a claim file, into a public account; relayed) ──────

// The words every relayed-claim screen uses.
export const RELAY_WORDING = "The wallet signs the claim; the bridge's relay sends it and pays the fee."

// The steps of a relayed claim (module relayed.rs PHASES).
export const RELAY_PHASES = [
    { key: "approve", word: "Approve", hint: "Your Names app shows the call your wallet built; your wallet signs it" },
    { key: "relay", word: "Relay", hint: "The bridge's relay checks it, sends it and pays the fee" },
    { key: "confirm", word: "Confirm", hint: "Waits for its block on the bridge test network" }
]

// Titles for the relayed claim's codes (module relayed.rs): specific, never "official".
export function relayErrorTitle(code) {
    var t = {
        bad_file: "That is not a usable claim file",
        private_account: "This deposit is for a private account",
        other_chain: "This deposit is on another chain",
        other_vault: "This deposit went to another vault",
        cfg_mismatch: "This deposit is for another bridge",
        token_not_registered: "Token not supported",
        bad_denomination: "Not a valid amount",
        wrong_amount: "Not a valid amount",
        wrong_leaf: "The claim file's leaf does not match",
        not_in_vault: "No such deposit in the vault",
        not_final: "Not final on Sepolia yet",
        root_not_attested: "Not recorded on the bridge test network yet",
        already_claimed: "Already claimed",
        minting_frozen: "Claims are paused",
        minting_frozen_token: "Claims of this token are paused",
        leaves_mismatch: "The vault's deposits don't add up here",
        no_bridge: "No bridge on this network",
        wrong_chain: "This bridge belongs to another network",
        bridge_unreadable: "The bridge's settings can't be read",
        bad_config: "This app's bridge settings are out of date",
        no_relay: "No claim relay for this bridge",
        relay_cap: "The relay's limit is reached",
        relay_funds: "The relay is out of funds",
        relay_once: "The relay already sent this claim",
        relay_refused: "The relay refused the claim",
        relay_busy: "The relay is busy",
        wallet_no_answer: "Your Names Wallet didn't answer",
        no_deposit: "No such deposit on this computer",
        relay_unavailable: "The relay is not answering",
        rate_limited: "Too many requests just now",
        unreachable: "The bridge test network can't be reached",
        eth_unreachable: "Sepolia can't be read",
        wrong_account: "Wrong account",
        busy: "The relay was busy",
        landed_or_used: "Your account was used since you approved",
        reverted: "Included, then refused",
        wallet_failed: "Your Names Wallet could not finish it",
        wallet_rejected: "Nothing was sent"
    }
    return t[code] || "Something went wrong"
}

// Whether a code is worth trying again later as it is (time fixes it), for the button.
export function relayRetryable(code) {
    return ["not_final", "root_not_attested", "unreachable", "eth_unreachable", "rate_limited", "relay_busy", "relay_unavailable", "relay_cap", "relay_funds", "minting_frozen", "minting_frozen_token"].indexOf(code) >= 0
}

// What the person gave: a claim file's text, a path, or nothing usable yet.
export function claimSource(text) {
    var t = String(text || "").trim()
    if (t === "") return { kind: "empty", problem: "" }
    if (t.charAt(0) === "{") return t.indexOf("logos-bridge-claim") >= 0 ? { kind: "json", problem: "" } : { kind: "json", problem: "This JSON is not a bridge claim file (no \"logos-bridge-claim\" format)." }
    if (t.indexOf("file://") === 0 || t.charAt(0) === "/") return { kind: "path", problem: "" }
    return { kind: "other", problem: "Paste the whole claim file (it starts with {), or choose the file." }
}

// The network label rule: "LEZ 0.4 · bridge test network", never "official".
export function networkLabel(text) {
    var t = String(text || "").trim()
    if (t === "" || /official/i.test(t)) return "LEZ 0.4 · bridge test network"
    return t
}

// Reads the app may send again by itself when the module host was restarting (never anything that
// proposes, signs or sends).
export const RETRY_SAFE = ["config", "registry", "deposits", "holdings", "jobs", "job", "accounts", "exits", "admin_status", "wallet_status",
    "wallet_request", "privacy_report", "relayed_claim_check", "relayed_claim_check_with", "relayed_claim_status", "relayed_claims", "claim_plan", "plan_deposit", "plan_redeem"]

// An Ethereum RPC that rate-limits (HTTP 429) is a wait, not an error.
export function isBusy(text) {
    return /\b429\b|too many requests|rate.?limit|is busy/i.test(String(text || ""))
}
