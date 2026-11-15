// LEZ Bridge for Basecamp: move ERC-20 tokens and ETH between Ethereum and LEZ privately.
//
// A front end on the `lez_bridge` core module. QML in Basecamp has no network, so every read
// and every action is a module call; anything long (a claim proves for about 11 minutes) is a
// job the module runs, which this screen polls by id. The module answers JSON text
// `{success, value, error}`; amounts arrive as formatted text, so no arithmetic on amounts
// happens here.
//
// Privacy is the default everywhere (RFP-022 Usability #5-#7): the private choice is
// pre-selected, the privacy indicator sits at the top of every flow, and any choice that
// weakens privacy needs an explicit tick.
//
// Controls are drawn here, not taken from Qt Quick Controls, so the app looks the same
// whatever style the host applies. See DESIGN-NOTES.md.

import QtQuick
import QtQuick.Layouts
import Logos.Theme
import "logic.mjs" as Logic

Item {
    id: root

    // ── Look ────────────────────────────────────────────────────────────
    // The brand: cream paper, near-black ink and ONE accent, orange. State is carried by ink
    // weight, fills of ink or dim, shape, glyphs and words, never by a "success" hue: no green,
    // no blue. Ochre marks only "Less private"; a restrained red marks errors and "Not private".
    // Light or dark follows Basecamp (Theme.isDark); the harness sets `forcedMode` instead, so
    // it never writes the theme setting every Logos process reads. See DESIGN-NOTES.md.
    property int forcedMode: -1          // -1 follow Basecamp, 0 light, 1 dark
    readonly property bool dark: forcedMode === -1 ? Theme.isDark : forcedMode === 1
    readonly property var pal: dark ? darkTokens : lightTokens
    QtObject {
        id: lightTokens
        readonly property color background: "#f4f2ec"            // paper
        readonly property color backgroundSecondary: "#fffefb"   // card
        readonly property color backgroundInset: "#f4f2ec"
        readonly property color backgroundTertiary: "#e7e3d9"    // tracks, pending steps
        readonly property color sidebar: "#ece9e1"
        readonly property color border: "#d6d1c4"                // line
        readonly property color borderSecondary: "#e2ddd1"
        readonly property color borderStrong: "#b7b0a1"
        readonly property color text: "#1b1a17"                  // ink
        readonly property color textSecondary: "#6d6960"         // dim
        readonly property color textTertiary: "#747067"
        readonly property color textPlaceholder: "#a29d91"
        readonly property color primary: "#e8662f"               // the one accent
        readonly property color primaryHover: "#d9571f"
        readonly property color primaryPressed: "#c24a17"
        readonly property color accentText: "#b4481c"            // orange as small text on paper (5.1:1)
        readonly property color onAccent: "#1b1a17"              // ink on orange (5.9:1)
        readonly property color caution: "#7f5d0c"               // ochre, "Less private" only
        readonly property color cautionFill: "#f2e7c9"
        readonly property color error: "#a8322a"
        readonly property color errorFill: "#f6e3df"
        readonly property color focus: "#e8662f"
        readonly property color surfaceInteractiveHover: "#efece4"
    }
    QtObject {
        id: darkTokens
        readonly property color background: "#11110f"
        readonly property color backgroundSecondary: "#201f1b"
        readonly property color backgroundInset: "#171714"
        readonly property color backgroundTertiary: "#2e2d28"
        readonly property color sidebar: "#181815"
        readonly property color border: "#3a3832"
        readonly property color borderSecondary: "#2d2c27"
        readonly property color borderStrong: "#57544b"
        readonly property color text: "#edeae2"
        readonly property color textSecondary: "#a29d91"
        readonly property color textTertiary: "#908b80"
        readonly property color textPlaceholder: "#6f6b61"
        readonly property color primary: "#f0794a"
        readonly property color primaryHover: "#f48c62"
        readonly property color primaryPressed: "#d9622f"
        readonly property color accentText: "#f0794a"
        readonly property color onAccent: "#11110f"
        readonly property color caution: "#d9b45f"
        readonly property color cautionFill: "#2c2617"
        readonly property color error: "#e8806f"
        readonly property color errorFill: "#2f1c18"
        readonly property color focus: "#f0794a"
        readonly property color surfaceInteractiveHover: "#262520"
    }
    readonly property color cInset: pal.backgroundInset
    readonly property color cTrack: pal.backgroundTertiary
    readonly property color cLine: pal.borderSecondary
    readonly property string sans: Theme.typography.publicSans
    readonly property string mono: Theme.typography.mono
    readonly property int fsTitle: Theme.typography.panelTitleText
    readonly property int fsLead: Theme.typography.subtitleText
    readonly property int fsBody: Theme.typography.primaryText
    readonly property int fsSmall: Theme.typography.secondaryText
    readonly property int sp1: 4
    readonly property int sp2: 8
    readonly property int sp3: 12
    readonly property int sp4: 16
    readonly property int sp5: 24
    readonly property int sp6: 40
    readonly property int radius: 8
    readonly property int contentWidth: 760

    // Tones: ok and info are ink (state by shape and word), warn is ochre (Less private),
    // bad is red, accent is the orange, muted is dim.
    function tone(t) {
        if (t === "ok" || t === "info" || t === "ink") return pal.text
        if (t === "warn") return pal.caution
        if (t === "bad") return pal.error
        if (t === "accent") return pal.primary
        return pal.textSecondary
    }
    function soft(t) {
        if (t === "warn") return pal.cautionFill
        if (t === "bad") return pal.errorFill
        if (t === "accent") return Qt.rgba(pal.primary.r, pal.primary.g, pal.primary.b, 0.07)
        return "transparent"
    }
    function edge(t) {
        if (t === "warn") return pal.caution
        if (t === "bad") return pal.error
        if (t === "accent") return pal.primary
        if (t === "ok" || t === "ink") return pal.text
        return pal.border
    }

    // ── State ───────────────────────────────────────────────────────────
    property string section: "home"
    property var cfg: null
    property var registry: null
    property string registryError: ""
    property bool registryBusy: false     // the Ethereum RPC rate-limited: a calm wait, retried
    property bool moduleRestarting: false
    property bool registryLoading: false
    property var deposits: null
    property string depositsError: ""
    property var holdings: null
    property string holdingsError: ""
    property var jobsList: []
    property var accountsList: []
    property var admin: null
    property string adminError: ""
    property bool fixtureMode: false
    property string moduleProblem: ""

    // Deposit flow
    property int depToken: 0
    property string depAmount: "100"
    property string depAccount: ""
    property var depPlan: null
    property string depError: ""
    property bool depBusy: false
    property var depMade: ({})         // piece index -> the request made for it
    property int depShown: -1          // the piece whose request is shown
    readonly property var depRequest: depShown >= 0 && depMade[depShown] ? depMade[depShown] : null
    property bool showCalldata: false
    property bool depAllAtOnce: false
    property bool depConfirm: false
    property var depPrivacy: null

    // Claim flow
    property var claimFor: null        // the deposit being claimed
    property var claimPlan: null
    property string claimError: ""
    property bool claimBusy: false
    property bool claimReveal: false
    property bool claimEarly: false
    property bool claimNow: false       // claim before the suggested time (an explicit choice)
    readonly property bool claimIsEarly: !!(claimPlan && claimPlan.claim_early)
    property bool claimConfirm: false
    property var claimPrivacy: null

    // The job on screen (proving view)
    property var job: null
    property string jobId: ""
    property string jobError: ""

    // Redeem flow
    property int rdmToken: 0
    property string rdmAmount: "100"
    property string rdmTo: ""
    property int rdmRelay: 1
    property var rdmPlan: null
    property string rdmError: ""
    property bool rdmBusy: false
    property bool rdmConfirm: false
    property var exitsList: []

    // The Names Wallet (config().wallet === "names")
    readonly property bool namesWallet: !!(cfg && cfg.wallet === "names")
    property var walletState: null      // wallet_status
    property var walletNeeds: null      // a `needs` reply: {needs, request, message}
    property var walletRetry: null      // what to run again once it is approved
    property string walletNeedsState: ""
    property var rdmBuilt: null         // a burn built, not proposed (vault cannot release)

    // Recovery
    property string recAccount: ""
    property string recGap: "20"
    property var recResult: null
    property string recError: ""

    // Claim a deposit from a claim file (public account, relayed: relayed.rs)
    property string rcSource: ""       // pasted file text or a path; cleared once checked
    property var rcCheck: null          // relayed_claim_check
    property bool rcBusy: false
    property string rcError: ""
    property var rcJob: null            // relayed_claim_status
    property var rcUncertain: null      // relayed_claim's `uncertain` answer: a proposal got no answer
    property bool rcPolling: false
    property bool needsPolling: false   // one poll at a time: the module answers one call at a time
    property bool jobPolling: false
    property string rcPickerProblem: ""

    // Settings form
    property string setRpc: ""
    property string setProofRpc: ""
    property string setSeq: ""
    property string setCli: ""
    property string setWallet: ""
    property string setNotice: ""
    property bool setNoticeBad: false

    // Admin form
    property bool admMint: true
    property bool admBurn: false
    property string admToken: ""
    property string admAmount: ""
    property string admNotice: ""

    property real now: Date.now()
    Timer { interval: 1000; running: true; repeat: true; onTriggered: root.now = Date.now() }

    readonly property var tokens: registry && registry.tokens ? registry.tokens : []
    readonly property var activeJob: {
        for (var i = 0; i < jobsList.length; i++)
            if (jobsList[i].state === "running" || jobsList[i].state === "paused") return jobsList[i]
        return null
    }

    // ── Module calls ────────────────────────────────────────────────────
    function call(method, args, done, failed, timeout) {
        if (typeof logos === "undefined" || !logos.callModuleAsync) {
            moduleProblem = "Basecamp's module bridge is not available here."
            if (failed) failed(moduleProblem)
            return
        }
        callAttempt(method, args, done, failed, timeout, 0)
    }
    // The default wait (120 s) is longer than the module's worst case for one Names call while the
    // wallet is busy (about 32 s: each busy answer takes 15 s, asked again within 30 s) times the
    // few Names calls one module method makes: the screen shows the module's own answer, not a
    // timeout of its own.
    // A read whose module host was restarting (Basecamp reopens a closed app's tab against a fresh
    // host) is sent again after 2, 4 and 6 s; anything else reports at once.
    function callAttempt(method, args, done, failed, timeout, attempt) {
        logos.callModuleAsync("lez_bridge", method, args, function(reply) {
            var r = Logic.unwrap(reply)
            if (r.fixture) fixtureMode = true
            if (r.ok) { moduleRestarting = false; if (done) done(r.value); return }
            if (r.transient && attempt < 3 && Logic.RETRY_SAFE.indexOf(method) >= 0) {
                moduleRestarting = !r.busy
                later(2000 * (attempt + 1), function() { callAttempt(method, args, done, failed, timeout, attempt + 1) })
                return
            }
            moduleRestarting = false
            if (failed) failed(r.error)
        }, timeout || 120000)
    }
    function later(ms, fn) {
        var t = Qt.createQmlObject('import QtQuick; Timer { repeat: false }', root, "later")
        t.interval = ms
        t.triggered.connect(function() { t.destroy(); fn() })
        t.start()
    }

    function loadConfig() {
        call("config", [], function(v) {
            cfg = v
            if (v.fixtures) fixtureMode = true
            var rpc = v.eth_rpc ? v.eth_rpc[String(v.deployment.chain_id)] : ""
            setRpc = rpc || ""
            setProofRpc = v.eth_proof_rpc ? (v.eth_proof_rpc[String(v.deployment.chain_id)] || "") : ""
            setSeq = v.sequencer || ""
            setCli = v.cli || ""
            setWallet = v.wallet_dir || ""
        }, function(e) { moduleProblem = e })
    }
    Timer { id: registryRetry; interval: 15000; repeat: false; onTriggered: root.loadRegistry() }
    function loadRegistry() {
        registryLoading = true
        call("registry", [], function(v) { registry = v; registryError = ""; registryBusy = false; registryLoading = false },
             function(e) {
                 registryLoading = false
                 if (Logic.isBusy(e)) { registryBusy = true; registryError = ""; registryRetry.restart(); return }
                 registryBusy = false; registryError = e
             }, 60000)
    }
    function loadDeposits() {
        call("deposits", [], function(v) { deposits = v; depositsError = "" }, function(e) { depositsError = e })
    }
    function loadHoldings() {
        call("holdings", [], function(v) { holdings = v; holdingsError = "" }, function(e) { holdingsError = e })
    }
    function loadJobs() {
        call("jobs", [], function(v) { jobsList = v.jobs || [] })
    }
    function loadAccounts() {
        call("accounts", [], function(v) {
            accountsList = v.accounts || []
            if (depAccount === "" && v.suggested) depAccount = v.suggested
            if (recAccount === "" && v.suggested) recAccount = v.suggested
        })
    }
    function loadExits() { call("exits", [], function(v) { exitsList = v.exits || [] }) }
    function loadAdmin() { call("admin_status", [], function(v) { admin = v; adminError = "" }, function(e) { adminError = e }) }
    function loadWallet() { call("wallet_status", [], function(v) { walletState = v }, function(e) { walletState = { mode: "names", problem: e } }) }
    // A reply that says the person must approve something in the Names app first: shown where
    // they are, polled, and `retry` runs again once it is approved.
    function takeNeeds(v, retry) {
        if (!v || !v.needs) return false
        walletNeeds = v; walletRetry = retry || null; walletNeedsState = v.request && v.request.state ? v.request.state : "pending"
        return true
    }
    function connectWallet() {
        call("wallet_connect", [], function(v) {
            if (v && v.state === "approved") { loadWallet(); loadAccounts(); return }
            takeNeeds({ needs: "connect", request: v, message: "Approve this app in the Names app and give it a private account." }, function() { loadWallet(); loadAccounts() })
        }, function(e) { setNotice = e; setNoticeBad = true })
    }
    function pollNeeds() {
        var id = walletNeeds && walletNeeds.request ? (walletNeeds.request.request || "") : ""
        if (id === "" || needsPolling) return
        needsPolling = true
        call("wallet_request", [id], function(v) {
            needsPolling = false
            walletNeedsState = v.state || ""
            if (v.state === "approved" || v.state === "done") {
                var again = walletRetry
                walletNeeds = null; walletRetry = null; walletNeedsState = ""
                loadWallet()
                if (again) again()
            }
        }, function(e) { needsPolling = false })
    }
    Timer {
        interval: 2500; repeat: true
        running: root.walletNeeds !== null && (root.walletNeedsState === "" || root.walletNeedsState === "pending")
        onTriggered: root.pollNeeds()
    }
    function refreshAll() { loadRegistry(); loadDeposits(); loadHoldings(); loadJobs(); loadExits() }

    function go(s) {
        section = s
        if (s === "admin") { loadAdmin(); loadAccounts() }
        if (s === "deposit" || s === "recover") loadAccounts()
        if (s === "settings" || s === "home") loadWallet()
        if (s === "exits") loadExits()
        if (s === "pending") loadDeposits()
        if (s === "holdings") loadHoldings()
    }

    function privacyFor(flow, chosen, confirmed, target) {
        call("privacy_report", [JSON.stringify(Logic.choices(flow, chosen, confirmed))], function(v) { root[target] = v })
    }

    function planDeposit() {
        if (!tokens.length) return
        depBusy = true; depError = ""; depPlan = null; depMade = ({}); depShown = -1
        call("plan_deposit", [tokens[depToken].symbol, depAmount], function(v) { depPlan = v; depBusy = false },
             function(e) { depError = e; depBusy = false })
    }
    readonly property bool depPublic: { for (var i = 0; i < accountsList.length; i++) if (accountsList[i].account === depAccount) return accountsList[i].kind === "public"; return false }
    function depChoicesChanged() { privacyFor("deposit", { public_account: depPublic }, depConfirm, "depPrivacy") }
    onDepAccountChanged: { depConfirm = false; depChoicesChanged() }
    function makeRequest(pieceIndex, denomIndex) {
        depBusy = true; depError = ""
        var method = depPublic ? "make_public_deposit_request" : "make_deposit_request"
        var args = depPublic ? [tokens[depToken].symbol, String(denomIndex), depAccount, depConfirm ? "true" : "false"] : [tokens[depToken].symbol, String(denomIndex), depAccount]
        call(method, args, function(v) {
            depBusy = false
            if (takeNeeds(v, function() { makeRequest(pieceIndex, denomIndex) })) return
            var m = Object.assign({}, depMade); m[pieceIndex] = v; depMade = m; depShown = pieceIndex
            showCalldata = false; loadDeposits()
        }, function(e) { depError = e; depBusy = false })
    }
    function openClaim(d) {
        claimFor = d; claimPlan = null; claimError = ""; claimReveal = false; claimEarly = false; claimNow = false; claimConfirm = false
        section = "claim"; claimBusy = true
        privacyFor("claim", {}, false, "claimPrivacy")
        call("claim_plan", [d.id], function(v) {
            claimBusy = false
            if (v.needs && takeNeeds(v.needs, function() { openClaim(d) })) return
            claimPlan = v; claimPrivacy = v.privacy; claimChoicesChanged()
        }, function(e) { claimError = e; claimBusy = false }, 150000)
    }
    function claimChoicesChanged() {
        privacyFor("claim", { reveal_recipient: claimReveal, claim_early: claimIsEarly && claimNow, public_account: claimFor && claimFor.account_kind === "public" },
                   claimConfirm, "claimPrivacy")
    }
    function startClaim() {
        claimBusy = true; claimError = ""
        var o = { reveal_recipient: claimReveal, claim_early: claimIsEarly && claimNow, confirm_weaker: claimConfirm }
        call("claim", [claimFor.id, JSON.stringify(o)], function(v) {
            claimBusy = false
            if (takeNeeds(v, function() { startClaim() })) return
            showJob(v)
        }, function(e) { claimError = e; claimBusy = false }, 150000)
    }
    function showJob(j) { job = j; jobId = j.id; jobError = ""; section = "proving"; loadJobs() }
    function jobAction(m) {
        call(m, [jobId], function(v) { if (v && v.id && v.state) job = v; pollJob() }, function(e) { jobError = e })
    }
    function pollJob() {
        if (!jobId || jobPolling) return
        jobPolling = true
        call("job", [jobId], function(v) { jobPolling = false; job = v }, function(e) { jobPolling = false; jobError = e })
    }
    Timer {
        interval: 2000; repeat: true
        running: root.section === "proving" && root.job !== null && (root.job.state === "running" || root.job.state === "paused")
        onTriggered: root.pollJob()
    }
    Timer { interval: 30000; repeat: true; running: true; onTriggered: { root.loadRegistry(); root.loadJobs() } }

    function planRedeem() {
        if (!tokens.length) return
        rdmBusy = true; rdmError = ""; rdmPlan = null; rdmConfirm = false
        rdmBuilt = null
        call("plan_redeem", [tokens[rdmToken].symbol, rdmAmount, rdmTo, String(rdmRelay)], function(v) {
            rdmBusy = false
            if (takeNeeds(v, function() { planRedeem() })) return
            rdmPlan = v
        }, function(e) { rdmError = e; rdmBusy = false }, 150000)
    }
    function startRedeem() {
        rdmBusy = true
        call("redeem", [rdmPlan.id, JSON.stringify({ confirm_weaker: rdmConfirm })], function(v) {
            rdmBusy = false
            if (takeNeeds(v, function() { startRedeem() })) return
            if (v.proposed === false) { rdmBuilt = v; return }
            showJob(v)
        }, function(e) { rdmError = e; rdmBusy = false }, 130000)
    }
    function recover() {
        recError = ""; recResult = null
        call("recover", [recAccount, recGap], function(v) {
            if (takeNeeds(v, function() { recover() })) return
            recResult = v
        }, function(e) { recError = e }, 240000)
    }
    function saveSettings() {
        setNotice = ""; setNoticeBad = false
        var chain = cfg ? String(cfg.deployment.chain_id) : "11155111"
        call("set_eth_rpc", [chain, setRpc], function(v) {
            setNotice = v.answers ? (v.chain_matches ? "Saved. The Ethereum RPC answers for chain " + chain + "." : "Saved, but that RPC serves another chain.") : "Saved, but the RPC did not answer: " + (v.problem || "")
            setNoticeBad = !(v.answers && v.chain_matches)
            call("set_eth_proof_rpc", [chain, setProofRpc], null, function(e) { setNotice = e; setNoticeBad = true })
            call("set_sequencer", [setSeq], function() { loadConfig(); loadRegistry() }, function(e) { setNotice = e; setNoticeBad = true })
        }, function(e) { setNotice = e; setNoticeBad = true })
    }
    function saveCli() {
        call("set_cli", [setCli, setWallet], function() { setNotice = "Saved the program and wallet folder."; setNoticeBad = false; loadConfig(); refreshAll() },
             function(e) { setNotice = e; setNoticeBad = true })
    }
    function adminDo(o) {
        admNotice = ""
        call("admin_action", [JSON.stringify(o)], function(v) {
            if (v.available === false) { admNotice = v.reason; return }
            showJob(v)
        }, function(e) { admNotice = e })
    }

    // ── Claim a deposit (claim file → public account; the relay pays) ──
    function openClaimFile() { section = "claimfile" }
    function pickClaimFile() {
        rcPickerProblem = ""
        try {
            var d = Qt.createQmlObject('import QtQuick.Dialogs; FileDialog { title: "Choose your claim file"; nameFilters: ["Claim files (*.json)", "All files (*)"] }', root, "claimFilePicker")
            d.accepted.connect(function() { rcSource = String(d.selectedFile); d.destroy(); checkClaimFile() })
            d.rejected.connect(function() { d.destroy() })
            d.open()
        } catch (e) {
            rcPickerProblem = "This Basecamp has no file picker: paste the claim file's text, or its full path, below."
        }
    }
    function checkClaimFile() {
        var src = rcSource
        if (Logic.claimSource(src).kind === "empty") return
        rcBusy = true; rcError = ""; rcCheck = null; rcJob = null; rcUncertain = null
        // The file's text holds the claim key: it leaves this screen as soon as it is sent.
        if (Logic.claimSource(src).kind === "json") rcSource = ""
        call("relayed_claim_check", [src], function(v) { rcBusy = false; rcCheck = v }, function(e) { rcBusy = false; rcError = e }, 90000)
    }
    // "Try again" comes here too: the module returns the claim's running job if it made one (this
    // screen may have stopped waiting first), and after a proposal that got no answer it answers
    // `uncertain` instead of proposing twice. `afterUncertain`: the person looked in the Names app.
    function startRelayedClaim(afterUncertain) {
        if (!rcCheck || !rcCheck.ok) return
        rcBusy = true; rcError = ""; rcUncertain = null
        var opts = afterUncertain ? JSON.stringify({ after_uncertain: true }) : "{}"
        call("relayed_claim", [rcCheck.id, opts], function(v) {
            rcBusy = false
            if (v && v.needs === "uncertain") { rcUncertain = v; return }
            if (takeNeeds(v, function() { startRelayedClaim() })) return
            rcJob = v
        }, function(e) { rcBusy = false; rcError = e }, 240000)
    }
    // One poll at a time: the module answers one call at a time, and a busy wallet makes one slow.
    function pollRelayed() {
        if (!rcJob || rcPolling) return
        rcPolling = true
        call("relayed_claim_status", [rcJob.id], function(v) { rcPolling = false; rcJob = v }, function(e) { rcPolling = false; rcError = e }, 150000)
    }
    function cancelRelayed() { call("relayed_claim_cancel", [rcJob.id], function(v) { rcJob = v }, function(e) { rcError = e }) }
    function restartClaimFile() {
        if (rcCheck && rcCheck.id) call("relayed_claim_forget", [rcCheck.id])
        rcCheck = null; rcJob = null; rcError = ""; rcSource = ""; rcUncertain = null
    }
    Timer {
        interval: 2500; repeat: true
        running: root.section === "claimfile" && root.rcJob !== null && root.rcJob.state === "running"
        onTriggered: root.pollRelayed()
    }

    Component.onCompleted: { loadConfig(); refreshAll(); loadAccounts(); loadWallet(); depChoicesChanged() }

    // ── Building blocks ─────────────────────────────────────────────────

    component Label: Text {
        color: root.pal.text
        font.family: root.sans
        font.pixelSize: root.fsBody
        wrapMode: Text.WordWrap
        lineHeight: 1.3
        font.features: { "tnum": 1 }
    }
    component Muted: Label { color: root.pal.textSecondary }
    component Small: Label { font.pixelSize: root.fsSmall; color: root.pal.textSecondary }
    component Mono: Text {
        color: root.pal.text
        font.family: root.mono
        font.pixelSize: root.fsSmall
        wrapMode: Text.WrapAnywhere
    }
    component Heading: Label { font.pixelSize: root.fsTitle + 2; font.weight: Font.DemiBold; font.letterSpacing: -0.4; lineHeight: 1.1; Accessible.role: Accessible.Heading }
    component SubHeading: Label { font.pixelSize: root.fsLead; font.weight: Font.DemiBold; Accessible.role: Accessible.Heading }

    component Card: Rectangle {
        default property alias body: inner.data
        property string tone: ""
        Layout.fillWidth: true
        implicitHeight: inner.implicitHeight + 2 * root.sp4
        radius: root.radius
        color: tone === "" ? root.pal.backgroundSecondary : root.soft(tone)
        border.color: tone === "" ? root.cLine : root.edge(tone)
        border.width: 1
        ColumnLayout { id: inner; anchors.fill: parent; anchors.margins: root.sp4; spacing: root.sp2 }
    }

    // A tag. Tones: ink (outlined ink), solid (ink fill), accent (orange fill), warn (ochre),
    // bad (red), muted (dim). `glyph` "shield" draws a small shield before the word.
    component Chip: Rectangle {
        id: chip
        property string text: ""
        property string tone: "muted"
        property string glyph: ""
        implicitWidth: chipRow.implicitWidth + 2 * root.sp2
        implicitHeight: 22
        radius: 5
        color: tone === "solid" ? root.pal.text : tone === "accent" ? root.pal.primary : root.soft(tone)
        border.color: tone === "solid" ? root.pal.text : root.edge(tone)
        border.width: 1
        Accessible.role: Accessible.StaticText
        Accessible.name: text
        readonly property color ink: tone === "solid" ? root.pal.background : tone === "accent" ? root.pal.onAccent : root.tone(tone)
        Row {
            id: chipRow; anchors.centerIn: parent; spacing: 4
            Shield { visible: chip.glyph === "shield"; kind: "shield-full"; color: chip.ink; width: 9; height: 11; implicitWidth: 9; implicitHeight: 11; anchors.verticalCenter: parent.verticalCenter }
            Text { text: chip.text; color: chip.ink; font.family: root.sans; font.pixelSize: root.fsSmall; font.weight: Font.DemiBold; anchors.verticalCenter: parent.verticalCenter; font.features: { "tnum": 1 } }
        }
    }

    component Btn: Rectangle {
        id: btn
        property string text: ""
        property bool primary: false
        property bool enabledState: true
        property string why: ""        // why it is disabled, said beside it and to screen readers
        signal clicked()
        implicitWidth: btnText.implicitWidth + 2 * root.sp4
        implicitHeight: 34
        radius: 6
        activeFocusOnTab: enabledState
        color: !enabledState ? "transparent" : primary ? (area.pressed ? root.pal.primaryPressed : area.containsMouse ? root.pal.primaryHover : root.pal.primary) : (area.containsMouse ? root.pal.surfaceInteractiveHover : root.pal.backgroundSecondary)
        border.color: activeFocus ? root.pal.text : (primary && enabledState ? "transparent" : root.pal.border)
        border.width: activeFocus ? 2 : 1
        Accessible.role: Accessible.Button
        Accessible.name: text + (enabledState ? "" : (why ? ". Unavailable: " + why : ". Unavailable"))
        Keys.onReturnPressed: if (enabledState) clicked()
        Keys.onSpacePressed: if (enabledState) clicked()
        Text {
            id: btnText; anchors.centerIn: parent; text: btn.text
            color: !btn.enabledState ? root.pal.textTertiary : btn.primary ? root.pal.onAccent : root.pal.text
            font.family: root.sans; font.pixelSize: root.fsBody; font.weight: Font.DemiBold
        }
        MouseArea { id: area; anchors.fill: parent; hoverEnabled: true; enabled: btn.enabledState; cursorShape: Qt.PointingHandCursor; onClicked: { btn.forceActiveFocus(); btn.clicked() } }
    }

    component Field: Rectangle {
        id: fld
        property alias text: input.text
        property string label: ""
        property string placeholder: ""
        property string problem: ""
        property bool monoFont: false
        Layout.fillWidth: true
        implicitHeight: 38
        radius: 6
        color: root.cInset
        border.color: problem !== "" ? root.pal.error : input.activeFocus ? root.pal.focus : root.pal.border
        border.width: input.activeFocus || problem !== "" ? 2 : 1
        TextInput {
            id: input
            anchors.fill: parent; anchors.leftMargin: root.sp3; anchors.rightMargin: root.sp3
            verticalAlignment: TextInput.AlignVCenter
            color: root.pal.text; selectionColor: root.pal.primary; selectedTextColor: root.pal.onAccent
            font.family: fld.monoFont ? root.mono : root.sans; font.pixelSize: root.fsBody
            clip: true; activeFocusOnTab: true; selectByMouse: true
            Accessible.role: Accessible.EditableText
            Accessible.name: fld.label
            Accessible.description: fld.problem
        }
        Text {
            anchors.verticalCenter: parent.verticalCenter; anchors.left: parent.left; anchors.leftMargin: root.sp3
            visible: !!(input.text === "" && !input.activeFocus); text: fld.placeholder
            color: root.pal.textPlaceholder; font.family: root.sans; font.pixelSize: root.fsBody
        }
    }

    // A quiet note: a fact the reader should know, not a state. Inset paper, no border, no hue.
    component Note: Rectangle {
        default property alias body: noteCol.data
        Layout.fillWidth: true
        implicitHeight: noteCol.implicitHeight + 2 * root.sp3
        radius: 6
        color: root.cInset
        border.color: root.cLine
        ColumnLayout { id: noteCol; anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; anchors.margins: root.sp3; spacing: 4 }
    }

    // A radio-style option: a circle, a title and what it means.
    component Option: Rectangle {
        id: opt
        property bool selected: false
        property string title: ""
        property string detail: ""
        property string badge: ""
        property string badgeTone: "ink"
        property string badgeGlyph: ""
        signal chosen()
        Layout.fillWidth: true
        implicitHeight: optCol.implicitHeight + 2 * root.sp3
        radius: 6
        color: selected ? root.soft("accent") : "transparent"
        border.color: activeFocus ? root.pal.focus : selected ? root.edge("accent") : root.pal.border
        border.width: activeFocus ? 2 : 1
        activeFocusOnTab: true
        Accessible.role: Accessible.RadioButton
        Accessible.name: title + (badge ? ", " + badge : "")
        Accessible.checked: selected
        Accessible.description: detail
        Keys.onSpacePressed: chosen()
        Keys.onReturnPressed: chosen()
        RowLayout {
            anchors.fill: parent; anchors.margins: root.sp3; spacing: root.sp3
            Rectangle {
                Layout.alignment: Qt.AlignTop; Layout.topMargin: 2
                width: 16; height: 16; radius: 8; color: "transparent"; border.width: 2
                border.color: opt.selected ? root.pal.primary : root.pal.borderStrong
                Rectangle { anchors.centerIn: parent; width: 8; height: 8; radius: 4; color: root.pal.primary; visible: opt.selected }
            }
            ColumnLayout {
                id: optCol; Layout.fillWidth: true; spacing: 2
                RowLayout {
                    spacing: root.sp2
                    Label { text: opt.title; font.weight: Font.DemiBold; Layout.fillWidth: false }
                    Chip { visible: opt.badge !== ""; text: opt.badge; tone: opt.badgeTone; glyph: opt.badgeGlyph }
                }
                Small { text: opt.detail; Layout.fillWidth: true; visible: text !== "" }
            }
        }
        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: { opt.forceActiveFocus(); opt.chosen() } }
    }

    component Check: Item {
        id: chk
        property bool checked: false
        property string text: ""
        property string tone: "warn"
        signal toggled()
        Layout.fillWidth: true
        implicitHeight: Math.max(20, chkText.implicitHeight)
        activeFocusOnTab: true
        Accessible.role: Accessible.CheckBox
        Accessible.name: text
        Accessible.checked: checked
        Keys.onSpacePressed: toggled()
        Rectangle {
            id: box; width: 18; height: 18; radius: 4; y: 1
            color: chk.checked ? root.tone(chk.tone) : "transparent"
            border.color: chk.activeFocus ? root.pal.text : root.tone(chk.tone); border.width: 2
            Text { anchors.centerIn: parent; text: "✓"; visible: chk.checked; color: chk.tone === "accent" ? root.pal.onAccent : root.pal.background; font.pixelSize: 13; font.bold: true }
        }
        Label { id: chkText; anchors.left: box.right; anchors.leftMargin: root.sp2; anchors.right: parent.right; text: chk.text }
        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: { chk.forceActiveFocus(); chk.toggled() } }
    }

    component KV: RowLayout {
        id: kv
        property string k: ""
        property string v: ""
        property bool monoValue: false
        property int keyWidth: 170
        Layout.fillWidth: true
        spacing: root.sp3
        Small { text: kv.k; Layout.preferredWidth: kv.keyWidth; Layout.alignment: Qt.AlignTop }
        Text {
            text: kv.v; Layout.fillWidth: true; color: root.pal.text
            font.family: kv.monoValue ? root.mono : root.sans; font.pixelSize: kv.monoValue ? root.fsSmall : root.fsBody
            wrapMode: kv.monoValue ? Text.WrapAnywhere : Text.WordWrap
        }
    }

    // A utilisation bar: label, used of cap, the share, coloured by how full.
    component UsageBar: ColumnLayout {
        id: ub
        property string label: ""
        property string usedText: ""
        property string capText: ""
        property var pct: null
        Layout.fillWidth: true
        spacing: 4
        Accessible.role: Accessible.ProgressBar
        Accessible.name: label + ": " + usedText + " of " + capText + (pct !== null && pct !== undefined ? " (" + pct + "%)" : "")
        RowLayout {
            Layout.fillWidth: true
            Small { text: ub.label; Layout.fillWidth: true; color: root.pal.text }
            Small { text: ub.usedText + " of " + ub.capText + (ub.pct !== null && ub.pct !== undefined ? " (" + ub.pct + "%)" : "") }
        }
        Rectangle {
            Layout.fillWidth: true; height: 8; radius: 4; color: root.cTrack
            Rectangle {
                height: parent.height; radius: 4
                width: parent.width * Logic.capWidth(ub.pct)
                color: Logic.capTone(ub.pct) === "ok" ? root.pal.textSecondary : root.tone(Logic.capTone(ub.pct))
            }
        }
    }

    // Loading, empty and error states share one look.
    component StateBox: Card {
        id: sb
        property string kind: "loading"   // loading, empty, error
        property string title: ""
        property string message: ""
        property string action: ""
        signal acted()
        tone: kind === "error" ? "bad" : ""
        Accessible.role: kind === "error" ? Accessible.AlertMessage : Accessible.StaticText
        Accessible.name: title + ". " + message
        RowLayout {
            spacing: root.sp3
            Layout.fillWidth: true
            Rectangle {
                width: 10; height: 10; radius: 5; Layout.alignment: Qt.AlignTop; Layout.topMargin: 6
                color: sb.kind === "error" ? root.pal.error : sb.kind === "loading" ? root.pal.primary : root.pal.textTertiary
                SequentialAnimation on opacity { running: sb.kind === "loading"; loops: Animation.Infinite; NumberAnimation { from: 1; to: 0.3; duration: 700 } NumberAnimation { from: 0.3; to: 1; duration: 700 } }
            }
            ColumnLayout {
                Layout.fillWidth: true; spacing: 4
                Label { text: sb.title; font.weight: Font.DemiBold; Layout.fillWidth: true }
                Muted { text: Logic.capitalized(sb.message); Layout.fillWidth: true; visible: text !== "" }
            }
        }
        Btn { visible: sb.action !== ""; text: sb.action; onClicked: sb.acted() }
    }

    // What a failed step means, the local diagnosis, and the on-chain truth (Usability #8).
    component ErrorNote: Card {
        id: en
        property string code: ""
        property string message: ""
        property bool claimContext: false
        tone: "bad"
        Accessible.role: Accessible.AlertMessage
        Accessible.name: Logic.errorTitle(code) + ". " + message
        Label { text: Logic.errorTitle(en.code); font.weight: Font.Bold; color: root.pal.error; Layout.fillWidth: true }
        Label { text: Logic.errorAdvice(en.code) !== "" ? Logic.errorAdvice(en.code) : Logic.capitalized(en.message); Layout.fillWidth: true }
        Small { visible: Logic.errorAdvice(en.code) !== "" && en.message !== ""; text: "Details: " + en.message; Layout.fillWidth: true }
        Small {
            Layout.fillWidth: true; visible: en.claimContext
            text: "This explanation was worked out on this computer and was not sent anywhere. On chain, every refused claim reads only “claim refused”, so nobody can tell which deposit was involved."
        }
    }

    // What the person must approve in the Names app before the bridge can go on: shown at the
    // top of whichever screen asked, polled, and the screen carries on once it is approved. No
    // hue: the current-step orange for "waiting for you", ink otherwise.
    component WalletNeedsCard: Card {
        visible: root.walletNeeds !== null
        Accessible.role: Accessible.AlertMessage
        Accessible.name: root.walletNeeds ? Logic.needsTitle(root.walletNeeds.needs) + ". " + root.walletNeeds.message : ""
        RowLayout {
            Layout.fillWidth: true
            SubHeading { text: root.walletNeeds ? Logic.needsTitle(root.walletNeeds.needs) : ""; Layout.fillWidth: true }
            Chip {
                readonly property bool waiting: root.walletNeedsState === "" || root.walletNeedsState === "pending"
                text: waiting ? "Waiting for you" : Logic.requestWords(root.walletNeedsState)
                tone: waiting ? "accent" : "muted"
            }
        }
        Label { text: root.walletNeeds ? root.walletNeeds.message : ""; Layout.fillWidth: true }
        RowLayout {
            Layout.fillWidth: true
            Small { text: root.walletNeedsState === "" || root.walletNeedsState === "pending" ? "Open the Names app to answer it. This screen carries on by itself once you approve." : Logic.requestWords(root.walletNeedsState) + ": nothing was signed or sent."; Layout.fillWidth: true }
            Btn { text: "Check now"; visible: root.walletNeedsState === "" || root.walletNeedsState === "pending"; onClicked: root.pollNeeds() }
            Btn { text: "Dismiss"; onClicked: { root.walletNeeds = null; root.walletRetry = null; root.walletNeedsState = "" } }
        }
    }

    // The privacy indicator, at the top of every flow. One line closed; open, it lists what is
    // public, what stays private and what each chosen option leaks.
    component PrivacyRail: Rectangle {
        id: rail
        property var report: null
        property bool open: false
        readonly property var look: Logic.privacyLook(report ? report.level : "")
        Layout.fillWidth: true
        implicitHeight: railCol.implicitHeight + 2 * root.sp3
        radius: root.radius
        color: look.tone === "ok" ? root.pal.backgroundSecondary : root.soft(look.tone)
        border.color: activeFocus ? root.pal.focus : root.edge(look.tone)
        border.width: activeFocus ? 2 : 1.5
        activeFocusOnTab: true
        Accessible.role: Accessible.Button
        Accessible.name: "Privacy: " + look.word + ". " + Logic.privacyHeadline(report) + (open ? " Details shown." : " Press to show details.")
        Keys.onSpacePressed: open = !open
        Keys.onReturnPressed: open = !open
        ColumnLayout {
            id: railCol; anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; anchors.margins: root.sp3; spacing: root.sp2
            RowLayout {
                Layout.fillWidth: true; spacing: root.sp3
                Shield { kind: rail.look.glyph; color: root.tone(rail.look.tone); Layout.alignment: Qt.AlignTop }
                Label { text: rail.look.word; font.weight: Font.Bold; color: root.tone(rail.look.tone); Layout.alignment: Qt.AlignTop; wrapMode: Text.NoWrap }
                Label { text: Logic.privacyHeadline(rail.report); Layout.fillWidth: true; color: rail.look.tone === "ok" ? root.pal.textSecondary : root.pal.text }
                Small { text: rail.open ? "Hide" : "What is public?"; color: root.pal.accentText; font.weight: Font.DemiBold; Layout.alignment: Qt.AlignTop }
            }
            ColumnLayout {
                visible: !!(rail.open && rail.report !== null); Layout.fillWidth: true; spacing: root.sp2
                Repeater {
                    model: rail.report && rail.report.weakened_by ? rail.report.weakened_by : []
                    delegate: ColumnLayout {
                        Layout.fillWidth: true; spacing: 2
                        Label { text: "Your choice: " + modelData.title; font.weight: Font.DemiBold; color: root.tone(rail.look.tone); Layout.fillWidth: true }
                        Label { text: modelData.leaks; Layout.fillWidth: true }
                        Small { text: "Visible to " + modelData.who; Layout.fillWidth: true }
                    }
                }
                Label { text: "Always public"; font.weight: Font.DemiBold; Layout.topMargin: root.sp1 }
                Repeater {
                    model: rail.report ? rail.report.always_public : []
                    delegate: RowLayout {
                        Layout.fillWidth: true; spacing: root.sp2
                        Text { text: "•"; color: root.pal.textTertiary; Layout.alignment: Qt.AlignTop }
                        Label { text: "<b>" + modelData.item + "</b> (" + modelData.where + "). " + modelData.why; textFormat: Text.StyledText; Layout.fillWidth: true }
                    }
                }
                Label { text: "Stays on this computer"; font.weight: Font.DemiBold; Layout.topMargin: root.sp1 }
                Repeater {
                    model: rail.report ? rail.report.stays_private : []
                    delegate: RowLayout {
                        Layout.fillWidth: true; spacing: root.sp2
                        Text { text: "•"; color: root.pal.text; Layout.alignment: Qt.AlignTop }
                        Label { text: modelData; Layout.fillWidth: true }
                    }
                }
                Repeater {
                    model: rail.report ? rail.report.notes : []
                    delegate: Small { text: modelData; Layout.fillWidth: true; color: root.pal.text }
                }
            }
        }
        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: { rail.forceActiveFocus(); rail.open = !rail.open } }
    }

    // A shield drawn with Canvas: full, half or open (the shape carries the level too).
    component Shield: Canvas {
        property string kind: "shield-full"
        property color color: "white"
        width: 18; height: 20; implicitWidth: 18; implicitHeight: 20
        onKindChanged: requestPaint()
        onColorChanged: requestPaint()
        onPaint: {
            var c = getContext("2d"); c.reset()
            c.scale(width / 18, height / 20)
            c.strokeStyle = color; c.fillStyle = color; c.lineWidth = 2
            c.beginPath(); c.moveTo(9, 1); c.lineTo(17, 4); c.lineTo(17, 10); c.quadraticCurveTo(17, 16, 9, 19); c.quadraticCurveTo(1, 16, 1, 10); c.lineTo(1, 4); c.closePath()
            if (kind === "shield-full") c.fill()
            c.stroke()
            if (kind === "shield-half") { c.beginPath(); c.moveTo(9, 1); c.lineTo(9, 19); c.quadraticCurveTo(1, 16, 1, 10); c.lineTo(1, 4); c.closePath(); c.fill() }
            if (kind === "shield-open") { c.beginPath(); c.moveTo(5, 7); c.lineTo(13, 14); c.moveTo(13, 7); c.lineTo(5, 14); c.stroke() }
        }
    }

    component NavItem: Rectangle {
        id: nav
        property string key: ""
        property string text: ""
        property string badge: ""
        property string badgeTone: "solid"
        Layout.fillWidth: true
        implicitHeight: 34
        radius: 6
        readonly property bool current: root.section === key || (key === "pending" && root.section === "claim")
        color: current ? root.pal.backgroundSecondary : navArea.containsMouse ? root.pal.surfaceInteractiveHover : "transparent"
        border.color: activeFocus ? root.pal.focus : "transparent"
        border.width: 2
        activeFocusOnTab: true
        Accessible.role: Accessible.PageTab
        Accessible.name: text + (badge ? ", " + badge : "")
        Accessible.selected: current
        Keys.onReturnPressed: root.go(key)
        Keys.onSpacePressed: root.go(key)
        Rectangle { visible: nav.current; width: 3; height: 18; radius: 2; color: root.pal.primary; anchors.left: parent.left; anchors.leftMargin: 2; anchors.verticalCenter: parent.verticalCenter }
        RowLayout {
            anchors.fill: parent; anchors.leftMargin: root.sp3; anchors.rightMargin: root.sp2
            Label { text: nav.text; font.weight: nav.current ? Font.DemiBold : Font.Normal; Layout.fillWidth: true; wrapMode: Text.NoWrap }
            Chip { visible: nav.badge !== ""; text: nav.badge; tone: nav.badgeTone }
        }
        MouseArea { id: navArea; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: { nav.forceActiveFocus(); root.go(nav.key) } }
    }

    // ── Layout ──────────────────────────────────────────────────────────

    Rectangle { anchors.fill: parent; color: root.pal.background }

    RowLayout {
        anchors.fill: parent
        spacing: 0

        // Sidebar
        Rectangle {
            Layout.fillHeight: true
            Layout.preferredWidth: 232
            color: root.pal.sidebar
            Rectangle { anchors.right: parent.right; width: 1; height: parent.height; color: root.pal.border }
            ColumnLayout {
                anchors.fill: parent; anchors.margins: root.sp3; spacing: 2
                RowLayout {
                    Layout.bottomMargin: root.sp4; Layout.leftMargin: root.sp1; Layout.topMargin: root.sp1; spacing: root.sp3
                    // The mark: a white η on an orange rounded tile.
                    Rectangle {
                        width: 32; height: 32; radius: 8; color: root.pal.primary
                        Accessible.role: Accessible.Graphic; Accessible.name: "LEZ Bridge"
                        Text { anchors.centerIn: parent; anchors.verticalCenterOffset: 2; text: "η"; color: "#ffffff"; font.family: root.sans; font.pixelSize: 22; font.weight: Font.DemiBold }
                    }
                    ColumnLayout {
                        spacing: 0
                        Label { text: "LEZ Bridge"; font.weight: Font.Bold; font.pixelSize: root.fsLead }
                        Small { text: "Ethereum and LEZ, privately" }
                    }
                }
                NavItem { key: "home"; text: "Overview" }
                NavItem { key: "registry"; text: "Registry" }
                Rectangle { Layout.fillWidth: true; Layout.topMargin: root.sp2; Layout.bottomMargin: root.sp2; height: 1; color: root.pal.border }
                NavItem { key: "deposit"; text: "Deposit" }
                NavItem {
                    key: "pending"; text: "Deposits and claims"
                    readonly property int ready: { var n = 0; var l = root.deposits ? root.deposits.deposits : []; for (var i = 0; i < l.length; i++) if (l[i].stage === "claimable" || l[i].stage === "anchored") n++; return n }
                    badge: ready > 0 ? String(ready) : ""; badgeTone: "solid"
                }
                NavItem { key: "claimfile"; text: "Claim a deposit"; badge: root.rcJob && root.rcJob.state === "running" ? "running" : ""; badgeTone: "accent" }
                Rectangle { Layout.fillWidth: true; Layout.topMargin: root.sp2; Layout.bottomMargin: root.sp2; height: 1; color: root.pal.border }
                NavItem { key: "holdings"; text: "Holdings" }
                NavItem { key: "redeem"; text: "Redeem" }
                NavItem { key: "exits"; text: "Exits and release" }
                Rectangle { Layout.fillWidth: true; Layout.topMargin: root.sp2; Layout.bottomMargin: root.sp2; height: 1; color: root.pal.border }
                NavItem { key: "proving"; text: "Jobs"; badge: root.activeJob ? "running" : ""; badgeTone: "accent" }
                NavItem { key: "recover"; text: "Recovery" }
                NavItem { key: "settings"; text: "Settings" }
                NavItem { key: "admin"; text: "Admin" }
                Item { Layout.fillHeight: true }
                ColumnLayout {
                    Layout.fillWidth: true; Layout.leftMargin: root.sp2; Layout.bottomMargin: root.sp1; spacing: 4
                    Small { text: "Ethereum: Sepolia, a public testnet"; visible: !root.cfg || root.cfg.deployment.chain_id === 11155111 }
                    Small { text: "LEZ: " + (root.cfg ? root.cfg.lez_network : "…") }
                    RowLayout {
                        spacing: 6
                        // Live: a solid ink dot. Recorded data: a hollow one.
                        Rectangle { width: 7; height: 7; radius: 4; color: root.fixtureMode ? "transparent" : root.pal.text; border.color: root.pal.text; border.width: 1.5 }
                        Small { text: root.fixtureMode ? "Recorded data" : "Reading the chains"; color: root.pal.text }
                    }
                }
            }
        }

        // Content
        Flickable {
            id: flick
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: width
            contentHeight: page.implicitHeight + 2 * root.sp6
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            ColumnLayout {
                id: page
                x: Math.max(root.sp5, (flick.width - root.contentWidth) / 2)
                y: root.sp5
                width: Math.min(root.contentWidth, flick.width - 2 * root.sp5)
                spacing: root.sp4

                StateBox {
                    visible: root.moduleProblem !== ""
                    kind: "error"; title: "The lez_bridge module is not answering"; message: root.moduleProblem
                }
                StateBox {
                    visible: root.moduleRestarting
                    kind: "loading"; title: "Reconnecting to the bridge module"; message: "Basecamp is restarting the lez_bridge module; this app asks again by itself."
                }
                WalletNeedsCard {}

                // Every screen: one ColumnLayout, shown when its section is current.

                // ── Overview ──
                ColumnLayout {
                    visible: root.section === "home"; Layout.fillWidth: true; spacing: root.sp4
                    Heading { text: "Overview" }
                    Muted {
                        Layout.fillWidth: true
                        text: root.registry ? root.registry.deployment.label + "." : "Reading the bridge…"
                    }
                    // Facts about this deployment, said once, quietly.
                    Rectangle {
                        visible: root.registry !== null && root.registry.deployment.notes.length > 0
                        Layout.fillWidth: true; implicitHeight: notesCol.implicitHeight + 2 * root.sp3; radius: root.radius
                        color: "transparent"; border.color: root.pal.border
                        ColumnLayout {
                            id: notesCol; anchors.fill: parent; anchors.margins: root.sp3; spacing: 4
                            Repeater {
                                model: root.registry ? root.registry.deployment.notes : []
                                delegate: RowLayout {
                                    Layout.fillWidth: true; spacing: root.sp2
                                    Text { text: "–"; color: root.pal.textSecondary; Layout.alignment: Qt.AlignTop; font.pixelSize: root.fsBody }
                                    Muted { text: modelData; Layout.fillWidth: true }
                                }
                            }
                        }
                    }
                    StateBox { visible: !root.registry && root.registryError === ""; kind: "loading"; title: root.registryBusy ? "Sepolia's RPC is busy" : "Reading the vault on Ethereum and the bridge on LEZ"; message: root.registryBusy ? "It asked us to slow down; this app reads again in a few seconds." : "This takes a few seconds." }
                    StateBox { visible: !root.registry && root.registryError !== ""; kind: "error"; title: "Could not read the bridge"; message: root.registryError; action: "Try again"; onActed: root.loadRegistry() }
                    Note {
                        visible: !!(root.registry !== null && root.registry.partial === true)
                        Label { text: "Part of the bridge could not be read"; font.weight: Font.DemiBold }
                        Repeater { model: root.registry ? root.registry.problems : []; delegate: Small { text: modelData; Layout.fillWidth: true; color: root.pal.text } }
                    }

                    SubHeading { text: "Your deposits"; Layout.topMargin: root.sp2 }
                    Rectangle {
                        Layout.fillWidth: true; radius: root.radius; color: root.pal.backgroundSecondary; border.color: root.cLine
                        implicitHeight: depList.implicitHeight
                        visible: root.deposits !== null && root.deposits.deposits.length > 0
                        ColumnLayout {
                            id: depList; anchors.left: parent.left; anchors.right: parent.right; spacing: 0
                            Repeater {
                                model: root.deposits ? root.deposits.deposits : []
                                delegate: Rectangle {
                                    Layout.fillWidth: true; implicitHeight: 56; color: "transparent"
                                    Rectangle { visible: index > 0; anchors.top: parent.top; anchors.left: parent.left; anchors.right: parent.right; anchors.leftMargin: root.sp4; anchors.rightMargin: root.sp4; height: 1; color: root.cLine }
                                    RowLayout {
                                        anchors.fill: parent; anchors.leftMargin: root.sp4; anchors.rightMargin: root.sp3; spacing: root.sp3
                                        Label { text: modelData.denom_text + " " + modelData.symbol; font.weight: Font.DemiBold; Layout.preferredWidth: 130; wrapMode: Text.NoWrap }
                                        ColumnLayout {
                                            Layout.fillWidth: true; spacing: 0
                                            Label { text: Logic.stageLook(modelData.stage).word; wrapMode: Text.NoWrap }
                                            Small {
                                                Layout.fillWidth: true; wrapMode: Text.NoWrap; elide: Text.ElideRight
                                                text: modelData.stage === "anchored" ? "Waiting until " + Logic.dateText(modelData.claim_after) + " is more private"
                                                    : modelData.stage === "requested" ? "Send it from an Ethereum wallet"
                                                    : modelData.stage === "claimed" ? "In your holdings"
                                                    : modelData.stage === "claimable" ? "The suggested waiting time has passed" : ""
                                            }
                                        }
                                        Btn {
                                            visible: modelData.stage === "claimable" || modelData.stage === "anchored"
                                            text: "Plan claim"; primary: modelData.stage === "claimable"
                                            onClicked: root.openClaim(modelData)
                                        }
                                    }
                                }
                            }
                        }
                    }
                    StateBox { visible: !!(root.deposits !== null && root.deposits.deposits.length === 0); kind: "empty"; title: "No deposits yet"; message: "Plan a deposit: the app splits the amount and makes a request to send from any Ethereum wallet."; action: "Plan a deposit"; onActed: root.go("deposit") }
                    Card {
                        visible: root.activeJob !== null
                        RowLayout {
                            Layout.fillWidth: true
                            Label { text: root.activeJob ? Logic.capitalized(root.activeJob.kind) + " running for " + Logic.durationText(root.activeJob.elapsed_s) : ""; Layout.fillWidth: true }
                            Btn { text: "Open job"; onClicked: root.showJob(root.activeJob) }
                        }
                    }

                    SubHeading { text: "The bridge now"; Layout.topMargin: root.sp2; visible: root.registry !== null }
                    GridLayout {
                        visible: root.registry !== null; Layout.fillWidth: true; columns: 3; columnSpacing: root.sp3; rowSpacing: root.sp3
                        Repeater {
                            model: root.registry ? [
                                { t: "Vault on Ethereum", big: String(root.registry.vault ? root.registry.vault.deposit_count : "—"), unit: "deposits by everyone",
                                  sub: root.registry.vault ? (root.registry.vault.deposits_frozen ? "Deposits are frozen." : "Deposits are open.") + (root.registry.vault.release_kind === "operator" ? " Releases: on the operator's attestation." : root.registry.vault.release_note ? " " + root.registry.vault.release_note : root.registry.vault.exits_possible ? "" : " This demo vault never releases.") : "Not read" },
                                { t: "Fee float on LEZ", big: root.registry.lez && root.registry.lez.claims_float_covers !== null ? String(root.registry.lez.claims_float_covers) : "—", unit: "sponsored claims left",
                                  sub: root.registry.lez ? (root.registry.lez.mint_frozen ? "Claims are frozen." : "Claims are open.") + " " + (root.registry.lez.burn_frozen ? "Burns are frozen." : "Burns are open.") : "Not read" },
                                { t: "Ethereum attestation", big: root.registry.attestation && root.registry.attestation.anchor_age_text ? root.registry.attestation.anchor_age_text : "—", unit: "old",
                                  sub: root.registry.attestation ? "Covers deposits up to block " + Logic.group(root.registry.attestation.anchor_block) + ". Newer ones wait for the next update; nobody runs updates automatically yet." : "Not read" }
                            ] : []
                            delegate: Rectangle {
                                Layout.fillWidth: true; Layout.preferredWidth: 1; Layout.fillHeight: true; implicitHeight: tile.implicitHeight + 2 * root.sp4; radius: root.radius
                                color: root.pal.backgroundSecondary; border.color: root.cLine
                                Accessible.role: Accessible.StaticText; Accessible.name: modelData.t + ": " + modelData.big + " " + modelData.unit + ". " + modelData.sub
                                ColumnLayout {
                                    id: tile; anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; anchors.margins: root.sp4; spacing: 2
                                    Small { text: modelData.t }
                                    RowLayout { spacing: root.sp2; Label { text: modelData.big; font.pixelSize: 28; font.weight: Font.DemiBold; font.letterSpacing: -0.5 } Small { text: modelData.unit; Layout.fillWidth: true; Layout.alignment: Qt.AlignBottom; Layout.bottomMargin: 5 } }
                                    Small { text: modelData.sub; Layout.fillWidth: true }
                                }
                            }
                        }
                    }
                }

                // ── Registry ──
                ColumnLayout {
                    visible: root.section === "registry"; Layout.fillWidth: true; spacing: root.sp4
                    RowLayout {
                        Layout.fillWidth: true
                        Heading { text: "Registry"; Layout.fillWidth: true }
                        Small { text: root.registry ? "Read " + Logic.durationText((root.now - root.registry.as_of) / 1000) + " ago" : ""; }
                        Btn { text: root.registryLoading ? "Reading…" : "Refresh"; enabledState: !root.registryLoading; onClicked: root.loadRegistry() }
                    }
                    Muted { Layout.fillWidth: true; text: "What the vault on Ethereum and the bridge on LEZ allow: the amounts you can move, the fees, the limits and how much of them is used, and whether anything is frozen." }
                    StateBox { visible: !!(!root.registry && root.registryError === ""); kind: "loading"; title: root.registryBusy ? "Sepolia's RPC is busy" : "Reading the registry"; message: root.registryBusy ? "It asked us to slow down; this app reads again in a few seconds." : "Both chains are asked; this takes a few seconds." }
                    StateBox { visible: !!(!root.registry && root.registryError !== ""); kind: "error"; title: "Could not read the registry"; message: root.registryError; action: "Try again"; onActed: root.loadRegistry() }
                    Repeater {
                        model: root.tokens
                        delegate: Card {
                            id: tokCard
                            property var t: modelData
                            RowLayout {
                                Layout.fillWidth: true; spacing: root.sp2
                                SubHeading { text: tokCard.t.symbol; Layout.fillWidth: false }
                                Mono { text: tokCard.t.eth_token === "0x0000000000000000000000000000000000000000" ? "native ETH" : Logic.shortId(tokCard.t.eth_token); color: root.pal.textSecondary; Layout.fillWidth: true }
                                Chip { text: tokCard.t.registered ? "Registered" : "Delisted (claims and releases continue)"; tone: tokCard.t.registered ? "ink" : "muted" }
                            }
                            // Denominations table
                            GridLayout {
                                Layout.fillWidth: true; columns: 5; columnSpacing: root.sp3; rowSpacing: 6; Layout.topMargin: root.sp2
                                Small { text: "Amount"; font.weight: Font.DemiBold } Small { text: "Deposit / withdraw fee"; font.weight: Font.DemiBold }
                                Small { text: "Relay-fee menu"; font.weight: Font.DemiBold; Layout.fillWidth: true } Small { text: "Deposits"; font.weight: Font.DemiBold } Small { text: "Claimed"; font.weight: Font.DemiBold }
                                Repeater {
                                    model: tokCard.t.denoms.length * 5
                                    delegate: Text {
                                        readonly property var row: tokCard.t.denoms[Math.floor(index / 5)]
                                        readonly property int col: index % 5
                                        text: col === 0 ? row.text + " " + tokCard.t.symbol
                                            : col === 1 ? row.deposit_fee_text + " / " + row.withdraw_fee_text
                                            : col === 2 ? "0 or " + row.relay_fee_menu.map(function(m) { return m.text + " (" + m.pct + ")" }).join(", ")
                                            : col === 3 ? String(row.deposits === undefined || row.deposits === null ? "—" : row.deposits)
                                            : String(row.lez_claims === undefined || row.lez_claims === null ? "—" : row.lez_claims)
                                        color: col === 0 ? root.pal.text : root.pal.textSecondary
                                        font.family: root.sans; font.pixelSize: root.fsBody; font.weight: col === 0 ? Font.DemiBold : Font.Normal
                                        Layout.fillWidth: col === 2; wrapMode: Text.WordWrap
                                    }
                                }
                            }
                            Rectangle { Layout.fillWidth: true; height: 1; color: root.cLine; Layout.topMargin: root.sp2 }
                            UsageBar { label: "Deposits, last " + Logic.durationText(tokCard.t.caps.window_s); usedText: tokCard.t.caps.deposit.used_text + " " + tokCard.t.symbol; capText: tokCard.t.caps.deposit.cap_text || "no cap"; pct: tokCard.t.caps.deposit.pct }
                            UsageBar { label: "Releases, last " + Logic.durationText(tokCard.t.caps.window_s); usedText: tokCard.t.caps.release.used_text + " " + tokCard.t.symbol; capText: tokCard.t.caps.release.cap_text || "no cap"; pct: tokCard.t.caps.release.pct }
                            Small { Layout.fillWidth: true; text: "The rolling window is estimated from two buckets, so close to twice a cap can pass within one window at worst." }
                            Rectangle { Layout.fillWidth: true; height: 1; color: root.cLine; Layout.topMargin: root.sp2 }
                            GridLayout {
                                Layout.fillWidth: true; columns: 4; columnSpacing: root.sp3; rowSpacing: 4
                                Repeater {
                                    model: [
                                        { k: "Ethereum deposits", v: tokCard.t.freeze.eth_deposits },
                                        { k: "Ethereum releases", v: tokCard.t.freeze.eth_releases },
                                        { k: "LEZ claims (mint)", v: tokCard.t.freeze.lez_mint },
                                        { k: "LEZ burns", v: tokCard.t.freeze.lez_burn }
                                    ]
                                    delegate: ColumnLayout {
                                        spacing: 2; Layout.fillWidth: true
                                        Small { text: modelData.k }
                                        Chip { text: Logic.freezeWord(modelData.v); tone: modelData.v === true ? "bad" : modelData.v === false ? "ok" : "muted" }
                                    }
                                }
                            }
                            KV { k: "Release delay"; v: tokCard.t.release_delay_s !== null && tokCard.t.release_delay_s !== undefined ? Logic.durationText(tokCard.t.release_delay_s) + " after an exit is recorded" : "unread" }
                            KV { k: "Global cap weight"; v: tokCard.t.cap_weight_text || "—" }
                            KV { k: "Claimed on LEZ"; v: (tokCard.t.lez_claimed_text || "—") + " " + tokCard.t.symbol }
                            KV { k: "Wrapped token on LEZ"; v: tokCard.t.definition; monoValue: true }
                        }
                    }
                    Card {
                        visible: !!(root.registry !== null && root.registry.vault !== null)
                        SubHeading { text: "Whole bridge" }
                        UsageBar {
                            visible: !!(root.registry && root.registry.vault)
                            label: "All tokens, deposits (in USD), last " + (root.registry && root.registry.vault ? Logic.durationText(root.registry.vault.global_cap.window_s) : "")
                            usedText: root.registry && root.registry.vault ? root.registry.vault.global_cap_text.deposit_used : ""
                            capText: root.registry && root.registry.vault ? root.registry.vault.global_cap_text.deposit_cap : ""
                            pct: root.registry && root.registry.vault ? root.registry.vault.global_cap_pct.deposit : null
                        }
                        Small { Layout.fillWidth: true; text: "The global cap counts every token by its cap weight (set by the admin), in dollars." }
                        KV { k: "Minimum release delay"; v: root.registry && root.registry.vault ? Logic.durationText(root.registry.vault.min_release_delay_s) + " (fixed for this vault)" : "" }
                        KV { k: "Ethereum freeze"; v: root.registry && root.registry.vault ? "deposits " + Logic.freezeWord(root.registry.vault.deposits_frozen).toLowerCase() + ", releases " + Logic.freezeWord(root.registry.vault.releases_frozen).toLowerCase() : "" }
                        KV { k: "LEZ freeze"; v: root.registry && root.registry.lez ? "claims " + Logic.freezeWord(root.registry.lez.mint_frozen).toLowerCase() + ", burns " + Logic.freezeWord(root.registry.lez.burn_frozen).toLowerCase() : "" }
                        KV { k: "Fee float on LEZ"; v: root.registry && root.registry.lez ? root.registry.lez.float_text + " LEZ, paying " + root.registry.lez.sponsor_text + " to each private claim: " + Logic.plural(root.registry.lez.claims_float_covers, "claim", "claims") + " left" : "" }
                        KV { k: "Attestation instance"; v: root.registry && root.registry.attestation ? root.registry.attestation.instance : ""; monoValue: true }
                        KV { k: "Latest anchor"; v: root.registry && root.registry.attestation ? "Ethereum block " + root.registry.attestation.anchor_block + ", " + root.registry.attestation.anchor_age_text + " ago, " + root.registry.attestation.behind_finalized_blocks + " blocks behind finality" : "" }
                        Small { Layout.fillWidth: true; text: root.registry && root.registry.attestation ? root.registry.attestation.updater : "" }
                        KV { k: "Exits"; v: root.registry && root.registry.vault ? (root.registry.vault.release_kind === "operator" ? "on the bridge operator's attestation (operator-attested demo vault; not trustless)" : root.registry.vault.release_note ? root.registry.vault.release_note : root.registry.vault.exits_possible ? "possible once the outbound proof runs" : "never, on this demo vault (placeholder outbound image)") : "" }
                        Small { Layout.fillWidth: true; visible: text !== ""; text: root.registry && root.registry.vault && root.registry.vault.release_detail ? root.registry.vault.release_detail : "" }
                    }
                }

                // ── Deposit ──
                ColumnLayout {
                    visible: root.section === "deposit"; Layout.fillWidth: true; spacing: root.sp4
                    Heading { text: "Deposit" }
                    PrivacyRail { report: root.depRequest ? root.depRequest.privacy : root.depPrivacy }
                    Muted { Layout.fillWidth: true; text: "The bridge moves fixed amounts only, so the amount hides you among everyone who moved the same amount. Larger amounts are split into pieces, best sent hours apart." }
                    StateBox { visible: root.tokens.length === 0; kind: root.registryError === "" ? "loading" : "error"; title: root.registryError === "" ? "Reading the tokens" : "Could not read the tokens"; message: root.registryError }
                    Card {
                        visible: root.tokens.length > 0
                        Small { text: "Token"; font.weight: Font.DemiBold; color: root.pal.text }
                        RowLayout {
                            spacing: root.sp2
                            Repeater {
                                model: root.tokens
                                delegate: Option {
                                    Layout.fillWidth: false; Layout.preferredWidth: 240
                                    title: modelData.symbol; detail: "Moves in " + modelData.denoms.map(function(d) { return d.text }).join(", ")
                                    selected: root.depToken === index
                                    onChosen: { root.depToken = index; root.depPlan = null; root.depMade = ({}); root.depShown = -1 }
                                }
                            }
                        }
                        Small { text: "Amount"; font.weight: Font.DemiBold; color: root.pal.text; Layout.topMargin: root.sp2 }
                        RowLayout {
                            Layout.fillWidth: true; spacing: root.sp2
                            Field {
                                id: depAmountField; label: "Amount to deposit"; text: root.depAmount
                                problem: root.tokens.length ? Logic.amountProblem(text, root.tokens[root.depToken].decimals) : ""
                                onTextChanged: root.depAmount = text
                            }
                            Btn { text: root.depBusy ? "Planning…" : "Plan the deposit"; primary: true; enabledState: !!(!root.depBusy && depAmountField.problem === "" && root.depAmount !== ""); onClicked: root.planDeposit() }
                        }
                        Small { visible: depAmountField.problem !== ""; text: depAmountField.problem; color: root.pal.error }
                        Small { text: "Receive on LEZ in"; font.weight: Font.DemiBold; color: root.pal.text; Layout.topMargin: root.sp2 }
                        Repeater {
                            model: root.accountsList.slice(0, 3)
                            delegate: Option {
                                title: Logic.shortId(modelData.account)
                                detail: modelData.kind === "private" ? "The claim mints to a one-time address, and the tokens land here without this account being named." : "Claiming into a public account publishes the deposit's secret and links your Ethereum address to it."
                                badge: modelData.kind === "private" ? "Private" : "Public"; badgeTone: modelData.kind === "private" ? "ink" : "bad"
                                badgeGlyph: modelData.kind === "private" ? "shield" : ""
                                selected: root.depAccount === modelData.account
                                onChosen: { root.depAccount = modelData.account; root.depConfirm = false }
                            }
                        }
                        Small { visible: root.accountsList.length === 0; text: root.namesWallet ? "Connect your Names Wallet (Settings) and give this app a private account to choose one here." : "Reading this wallet's accounts…"; Layout.fillWidth: true }
                        Check {
                            visible: root.depPublic; checked: root.depConfirm; tone: "bad"
                            text: "I understand that claiming into a public account publishes this deposit's secret and links my Ethereum address to this account for good."
                            onToggled: { root.depConfirm = !root.depConfirm; root.depChoicesChanged() }
                        }
                    }
                    ErrorNote { visible: root.depError !== ""; code: "cli_error"; message: root.depError }
                    Card {
                        visible: root.depPlan !== null
                        SubHeading { text: root.depPlan ? Logic.capitalized(root.depPlan.spread_text) : "" }
                        Small { Layout.fillWidth: true; text: "Make a request for each piece and send it from any Ethereum wallet around the suggested time. Claim each piece no sooner than a day after it." }
                        Repeater {
                            model: root.depPlan ? root.depPlan.pieces : []
                            delegate: Rectangle {
                                Layout.fillWidth: true; implicitHeight: 52; color: "transparent"
                                readonly property bool made: root.depMade[index] !== undefined
                                Rectangle { anchors.top: parent.top; width: parent.width; height: 1; color: root.cLine }
                                RowLayout {
                                    anchors.fill: parent; spacing: root.sp3
                                    // Numbered: the pieces are a sequence in time.
                                    Rectangle {
                                        width: 24; height: 24; radius: 12
                                        color: parent.parent.made ? root.pal.text : "transparent"; border.color: root.pal.text; border.width: 1.5
                                        Text { anchors.centerIn: parent; text: parent.parent.parent.made ? "✓" : index + 1; color: parent.parent.parent.made ? root.pal.background : root.pal.text; font.pixelSize: root.fsSmall; font.bold: true; font.family: root.sans }
                                    }
                                    Label { text: modelData.denom_text + " " + root.depPlan.symbol; font.weight: Font.DemiBold; Layout.preferredWidth: 120; wrapMode: Text.NoWrap }
                                    ColumnLayout {
                                        Layout.fillWidth: true; spacing: 0
                                        Label { text: "Send " + modelData.send_after_text; wrapMode: Text.NoWrap }
                                        Small { text: Logic.plural(modelData.anonymity_set, "earlier deposit", "earlier deposits") + " of this amount"; wrapMode: Text.NoWrap }
                                    }
                                    Btn {
                                        text: parent.parent.made ? (root.depShown === index ? "Shown below" : "Show request") : "Make request"
                                        primary: !parent.parent.made && index === 0
                                        enabledState: !!(parent.parent.made ? root.depShown !== index : (root.depAccount !== "" && !root.depBusy && (!root.depPublic || root.depConfirm)))
                                        why: root.depPublic && !root.depConfirm ? "confirm the public account first" : "choose an account"
                                        onClicked: parent.parent.made ? (root.depShown = index) : root.makeRequest(index, modelData.denom_index)
                                    }
                                }
                            }
                        }
                        Rectangle { Layout.fillWidth: true; height: 1; color: root.cLine }
                        KV { k: "Bridged"; v: root.depPlan ? root.depPlan.bridged_text + " " + root.depPlan.symbol : "" }
                        KV { k: "Deposit fees"; v: root.depPlan ? (root.depPlan.deposit_fees === "0" ? "None" : root.depPlan.deposit_fees_text + " " + root.depPlan.symbol) : "" }
                        KV { k: "You send in total"; v: root.depPlan ? root.depPlan.total_to_send_text + " " + root.depPlan.symbol + ", plus Ethereum gas" : "" }
                        KV { k: "Fee on LEZ"; v: root.depPlan ? "Paid by the bridge's fee float, so nothing has to fund your account first." : "" }
                        KV { k: "Stays with you"; v: root.depPlan ? root.depPlan.remainder_text + " " + root.depPlan.symbol + " (not a denomination)" : ""; visible: !!(root.depPlan && root.depPlan.remainder !== "0") }
                        Repeater { model: root.depPlan ? root.depPlan.warnings.filter(function(w) { return w.indexOf("does not fit a denomination") < 0 }) : []; delegate: Card { tone: "warn"; Label { text: modelData; Layout.fillWidth: true } } }
                        Card { visible: !!(root.depPlan && root.depPlan.blocked); tone: "bad"; Label { text: root.depPlan && root.depPlan.blocked ? root.depPlan.blocked : ""; Layout.fillWidth: true } }
                    }
                    Card {
                        visible: root.depRequest !== null
                        RowLayout {
                            Layout.fillWidth: true
                            SubHeading { text: root.depRequest ? "Request for " + root.depRequest.denom_text + " " + root.depRequest.symbol : ""; Layout.fillWidth: true }
                            Chip { text: root.namesWallet ? "Secret from your Names Wallet" : "Secret kept on this computer"; tone: "ink"; glyph: "shield" }
                        }
                        RowLayout {
                            Layout.fillWidth: true; spacing: root.sp5; Layout.topMargin: root.sp2
                            // QR of the EIP-681 link, drawn from the module's matrix.
                            Rectangle {
                                id: qrBox
                                readonly property var qr: root.depRequest ? root.depRequest.qr : null
                                Layout.alignment: Qt.AlignTop
                                width: 180; height: 180; color: "#ffffff"; radius: 6; border.color: root.pal.border
                                Accessible.role: Accessible.Graphic
                                Accessible.name: "QR code of the deposit link for an Ethereum wallet"
                                Canvas {
                                    anchors.fill: parent; anchors.margins: 12
                                    property var m: qrBox.qr
                                    onMChanged: requestPaint()
                                    onPaint: {
                                        var c = getContext("2d"); c.reset()
                                        if (!m) return
                                        var n = m.size, s = Math.floor(width / n), off = Math.floor((width - n * s) / 2)
                                        c.fillStyle = "#1b1a17"
                                        for (var y = 0; y < n; y++) { var row = m.rows[y]; for (var x = 0; x < n; x++) if (row.charAt(x) === "1") c.fillRect(off + x * s, off + y * s, s, s) }
                                    }
                                }
                            }
                            ColumnLayout {
                                Layout.fillWidth: true; Layout.alignment: Qt.AlignTop; spacing: root.sp2
                                Label { Layout.fillWidth: true; text: "Send it from any Ethereum wallet: scan the code, or copy the request." }
                                Repeater {
                                    model: root.depRequest ? root.depRequest.steps : []
                                    delegate: RowLayout {
                                        Layout.fillWidth: true; spacing: root.sp2
                                        Label { text: (index + 1) + "."; font.weight: Font.DemiBold; Layout.alignment: Qt.AlignTop }
                                        Label { text: modelData.text + (modelData.value !== "0" ? ", sending " + Logic.group(modelData.value) + " wei" : ""); Layout.fillWidth: true }
                                    }
                                }
                                KV { k: "Vault"; v: root.depRequest ? root.depRequest.vault : ""; monoValue: true; keyWidth: 90 }
                                KV { k: "Commitment"; v: root.depRequest ? root.depRequest.commitment : ""; monoValue: true; keyWidth: 90 }
                                RowLayout {
                                    spacing: root.sp2; Layout.topMargin: root.sp1
                                    Btn { text: "Copy request"; primary: true; onClicked: { exportText.selectAll(); exportText.copy(); exportText.deselect() } }
                                    Btn { text: root.showCalldata ? "Hide call data" : "Show call data"; onClicked: root.showCalldata = !root.showCalldata }
                                }
                            }
                        }
                        Repeater {
                            model: root.showCalldata && root.depRequest ? root.depRequest.steps : []
                            delegate: ColumnLayout {
                                Layout.fillWidth: true; spacing: 4; Layout.topMargin: root.sp2
                                Small { text: (index + 1) + ". " + modelData.text + ", to " + modelData.to; color: root.pal.text; font.weight: Font.DemiBold; Layout.fillWidth: true }
                                Rectangle {
                                    Layout.fillWidth: true; implicitHeight: cd.implicitHeight + 2 * root.sp3; radius: 6; color: root.cInset
                                    Mono { id: cd; anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; anchors.margins: root.sp3; text: Logic.calldataLines(modelData.data); color: root.pal.textSecondary; lineHeight: 1.3 }
                                }
                            }
                        }
                        Small { Layout.fillWidth: true; Layout.topMargin: root.sp2; text: "The commitment does not reveal which LEZ account receives this deposit. Only this computer holds the secret that claims it, so keep the bridge's data folder backed up until the wallet derives secrets from its seed." }
                        RowLayout {
                            Layout.fillWidth: true
                            Small { Layout.fillWidth: true; text: root.depRequest ? root.depRequest.send_in_basecamp.reason : "" }
                            Btn { text: "Send from Basecamp"; enabledState: false; why: root.depRequest ? root.depRequest.send_in_basecamp.reason : "" }
                        }
                        TextEdit { id: exportText; visible: false; text: root.depRequest ? root.depRequest.export_text : "" }
                    }
                }

                // ── Deposits & claims ──
                ColumnLayout {
                    visible: root.section === "pending"; Layout.fillWidth: true; spacing: root.sp4
                    RowLayout { Layout.fillWidth: true; Heading { text: "Deposits and claims"; Layout.fillWidth: true } Btn { text: "Claim from a claim file"; onClicked: root.openClaimFile() } Btn { text: "Refresh"; onClicked: root.loadDeposits() } }
                    Muted { Layout.fillWidth: true; text: "A deposit becomes claimable once its Ethereum block is final and the attestation on LEZ has an anchor past it. Waiting while more deposits of the same amount arrive makes the claim harder to match." }
                    StateBox { visible: !!(root.deposits === null && root.depositsError === ""); kind: "loading"; title: "Reading your deposits"; message: "Matching this computer's deposits with the vault's events." }
                    StateBox { visible: root.depositsError !== ""; kind: "error"; title: "Could not read the vault's deposits"; message: root.depositsError; action: "Try again"; onActed: root.loadDeposits() }
                    StateBox { visible: !!(root.deposits !== null && root.deposits.deposits.length === 0); kind: "empty"; title: "No deposits on this computer yet"; message: "Plan a deposit, send the request from an Ethereum wallet, and it shows here."; action: "Plan a deposit"; onActed: root.go("deposit") }
                    Repeater {
                        model: root.deposits ? root.deposits.deposits : []
                        delegate: Card {
                            id: depCard
                            property var d: modelData
                            RowLayout {
                                Layout.fillWidth: true; spacing: root.sp2
                                SubHeading { text: depCard.d.denom_text + " " + depCard.d.symbol; Layout.fillWidth: true }
                                Chip { text: Logic.stageLook(depCard.d.stage).word; tone: Logic.stageLook(depCard.d.stage).tone }
                            }
                            Small {
                                Layout.fillWidth: true
                                text: "To " + Logic.shortId(depCard.d.account) + ", a " + depCard.d.account_kind + " account." + (depCard.d.leaf_index !== undefined ? " " + Logic.capitalized(Logic.plural(depCard.d.anonymity_set, "deposit", "deposits")) + " of this amount under the anchor, yours included." : "")
                            }
                            // Timeline: done steps solid ink with a check, the next one orange, later ones outlined.
                            ColumnLayout {
                                Layout.fillWidth: true; Layout.topMargin: root.sp2; spacing: 0
                                Repeater {
                                    model: depCard.d.timeline || []
                                    delegate: Item {
                                        id: step
                                        Layout.fillWidth: true
                                        readonly property var tl: depCard.d.timeline
                                        readonly property bool last: index === tl.length - 1
                                        readonly property bool next: !modelData.done && depCard.d.stage !== "claimed" && (index === 0 || tl[index - 1].done)
                                        implicitHeight: stepText.implicitHeight + (last ? 0 : root.sp3)
                                        Rectangle {
                                            id: dot; x: 0; y: 2; width: 16; height: 16; radius: 8
                                            color: modelData.done ? root.pal.text : step.next ? root.pal.primary : "transparent"
                                            border.color: modelData.done ? root.pal.text : step.next ? root.pal.primary : root.pal.borderStrong; border.width: 1.5
                                            Text { anchors.centerIn: parent; text: "✓"; visible: modelData.done; color: root.pal.background; font.pixelSize: 10; font.bold: true }
                                        }
                                        Rectangle {
                                            visible: !step.last; x: 7; width: 2; anchors.top: dot.bottom; anchors.bottom: parent.bottom; anchors.bottomMargin: -2
                                            color: !step.last && modelData.done && step.tl[index + 1].done ? root.pal.text : root.pal.border
                                        }
                                        ColumnLayout {
                                            id: stepText; x: 28; width: parent.width - 28; spacing: 0
                                            RowLayout {
                                                Layout.fillWidth: true; spacing: root.sp2
                                                Label { text: modelData.label; color: modelData.done || step.next ? root.pal.text : root.pal.textSecondary; font.weight: modelData.done || step.next ? Font.DemiBold : Font.Normal }
                                                Small { text: modelData.at ? Logic.dateText(modelData.at) : ""; Layout.fillWidth: true }
                                            }
                                            Small { text: modelData.detail ? Logic.capitalized(modelData.detail) : ""; visible: text !== ""; Layout.fillWidth: true }
                                        }
                                    }
                                }
                            }
                            RowLayout {
                                Layout.fillWidth: true
                                Item { Layout.fillWidth: true }
                                Btn {
                                    text: depCard.d.stage === "claimed" ? "Claimed" : depCard.d.stage === "requested" ? "Waiting for the deposit" : "Plan claim"
                                    primary: depCard.d.stage === "claimable"
                                    enabledState: depCard.d.stage === "claimable" || depCard.d.stage === "anchored"
                                    why: depCard.d.stage === "claimed" ? "already claimed" : depCard.d.stage === "requested" ? "send the deposit on Ethereum first" : "not final enough yet"
                                    onClicked: root.openClaim(depCard.d)
                                }
                            }
                        }
                    }
                }

                // ── Claim ──
                ColumnLayout {
                    visible: root.section === "claim"; Layout.fillWidth: true; spacing: root.sp4
                    RowLayout { Layout.fillWidth: true; Btn { text: "Back to deposits"; onClicked: root.go("pending") } Heading { text: root.claimFor ? "Claim " + root.claimFor.denom_text + " " + root.claimFor.symbol : "Claim"; Layout.fillWidth: true; Layout.leftMargin: root.sp3 } }
                    PrivacyRail { report: root.claimPrivacy; visible: !!(root.claimPlan === null || root.claimPlan.ok) }
                    StateBox { visible: !!(root.claimBusy && root.claimPlan === null); kind: "loading"; title: "Checking the claim"; message: "Building the deposit list and running the claim on this computer as a dry run. Nothing is proven or sent." }
                    ErrorNote { visible: root.claimError !== ""; code: "cli_error"; message: root.claimError; claimContext: true }
                    ErrorNote { visible: !!(root.claimPlan !== null && !root.claimPlan.ok); code: root.claimPlan && root.claimPlan.error ? root.claimPlan.error.code : ""; message: root.claimPlan && root.claimPlan.error ? root.claimPlan.error.message : ""; claimContext: true }
                    Card {
                        visible: !!(root.claimPlan !== null && root.claimPlan.ok)
                        SubHeading { text: "Where the tokens arrive" }
                        Option {
                            selected: !root.claimReveal
                            title: "A one-time address (recommended)"
                            badge: "Private"; badgeTone: "ink"; badgeGlyph: "shield"
                            detail: "The mint goes to " + (root.claimPlan ? Logic.shortId(root.claimPlan.recipient) : "") + ", used once and never again. Your account " + (root.claimFor ? Logic.shortId(root.claimFor.account) : "") + " receives the tokens inside the proof without being named."
                            onChosen: { root.claimReveal = false; root.claimConfirm = false; root.claimChoicesChanged() }
                        }
                        Option {
                            selected: root.claimReveal
                            title: "Mint to my private account by name"
                            badge: "Links you"; badgeTone: "bad"
                            detail: "Publishes your account as the receiver of this mint, and the bridge does not pay the fee."
                            onChosen: { root.claimReveal = true; root.claimConfirm = false; root.claimChoicesChanged() }
                        }
                        SubHeading { text: "When"; Layout.topMargin: root.sp3 }
                        Option {
                            visible: root.claimIsEarly; selected: !root.claimNow
                            title: "Wait until " + (root.claimPlan ? Logic.dateText(root.claimPlan.deposit.claim_after) : "") + " (recommended)"
                            badge: "Private"; badgeTone: "ink"; badgeGlyph: "shield"
                            detail: (root.claimPlan ? Logic.capitalized(Logic.untilText(root.claimPlan.deposit.claim_after, root.now / 1000)) + ". " : "") + "A day after the deposit, with more deposits of the same amount arriving meanwhile, the claim is hard to pair with it."
                            onChosen: { root.claimNow = false; root.claimConfirm = false; root.claimChoicesChanged() }
                        }
                        Option {
                            visible: root.claimIsEarly; selected: root.claimNow
                            title: "Claim now"
                            badge: "Less private"; badgeTone: "warn"
                            detail: "A claim soon after its deposit can be paired with it by timing alone."
                            onChosen: { root.claimNow = true; root.claimConfirm = false; root.claimChoicesChanged() }
                        }
                        Label { visible: !root.claimIsEarly; Layout.fillWidth: true; text: "The suggested waiting time has passed: claiming now is private." }
                        Small { Layout.fillWidth: true; text: root.claimPlan ? Logic.capitalized(Logic.plural(root.claimPlan.deposit.anonymity_set, "deposit", "deposits")) + " of this amount are under the anchor, yours included; " + root.claimPlan.deposit.deposits_after + " arrived after yours." : "" }
                        SubHeading { text: "Fee"; Layout.topMargin: root.sp3 }
                        Label {
                            Layout.fillWidth: true
                            readonly property var dry: root.claimPlan && root.claimPlan.dry_run ? root.claimPlan.dry_run : ({})
                            text: root.claimPlan && root.claimPlan.wallet === "names"
                                  ? (root.claimPlan.sponsored && !root.claimReveal ? "Paid by the bridge's fee float. It sends " + Logic.group(root.claimPlan.sponsor) + " LEZ to the one-time address, and the claim's fee comes out of that, so nothing has to fund your account first. Your Names app shows the fee before you approve." : "Not sponsored: your account pays the fee. Your Names app shows it before you approve.")
                                  : root.claimPlan && root.claimPlan.sponsored && !root.claimReveal ? "Paid by the bridge's fee float. It sends " + Logic.group(dry.sponsor) + " LEZ to the one-time address, and the claim's fee (about " + Logic.group(dry.fee_at_head) + " LEZ) comes out of that, so nothing has to fund your account first." : "Not sponsored: your account pays the fee, about " + Logic.group(dry.fee_at_head || "") + " LEZ, so it must hold that much already."
                        }
                    }
                    Card {
                        visible: !!(root.claimPlan !== null && root.claimPlan.ok)
                        SubHeading { text: root.namesWallet ? "Approve in your Names Wallet" : "Proving on this computer" }
                        Label { visible: root.namesWallet; Layout.fillWidth: true; text: "Your Names app shows what this claim does, worked out by your wallet's own simulation, and asks you to approve it. After you approve, your wallet proves it on this computer and sends it; this app never proves or holds your keys." }
                        Label { Layout.fillWidth: true; text: (root.namesWallet ? "Proving takes time. " : "The claim is proved here before it is sent. ") + "Measured on a 16 GB Apple M4 desktop: about 11 minutes and 9.4 GB of memory at peak. Smaller computers are untested; close other large apps first, and keep Basecamp open until it finishes." }
                        Rectangle {
                            visible: !!(root.claimPlan && !root.claimPlan.sends_enabled)
                            Layout.fillWidth: true; implicitHeight: dryNote.implicitHeight + 2 * root.sp3; radius: 6; color: root.cInset
                            Label { id: dryNote; anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; anchors.margins: root.sp3; text: "This build runs claims as dry runs: Start simulates the claim on this computer and shows every step, and nothing is proven or sent." }
                        }
                        Check {
                            visible: !!(root.claimPrivacy !== null && root.claimPrivacy.requires_confirmation)
                            checked: root.claimConfirm
                            text: Logic.acceptText(root.claimPrivacy, "this claim")
                            tone: root.claimPrivacy && root.claimPrivacy.level === "exposed" ? "bad" : "warn"
                            onToggled: { root.claimConfirm = !root.claimConfirm; root.claimChoicesChanged() }
                        }
                        RowLayout {
                            Layout.fillWidth: true
                            Small { text: root.claimPlan ? (root.claimPlan.wallet === "names" ? "Checked on this computer: deposit, root, nullifier and float. Your wallet simulates it again before you approve." : "The dry run passed: recipient, float and deposit checked.") : ""; Layout.fillWidth: true }
                            Btn {
                                text: root.claimBusy ? "Starting…" : root.claimIsEarly && !root.claimNow ? "Claim after " + (root.claimPlan ? Logic.dateText(root.claimPlan.deposit.claim_after) : "") : root.namesWallet ? "Send to my Names Wallet" : "Start claim"
                                primary: true
                                enabledState: !!(!root.claimBusy && root.activeJob === null && !(root.claimIsEarly && !root.claimNow) && (!root.claimPrivacy || !root.claimPrivacy.requires_confirmation || root.claimConfirm))
                                why: root.activeJob !== null ? "another job is running" : root.claimIsEarly && !root.claimNow ? "you chose to wait; come back then, or choose Claim now" : "tick the confirmation first"
                                onClicked: root.startClaim()
                            }
                        }
                    }
                }

                // ── Claim a deposit (claim file → public account; the relay pays) ──
                ColumnLayout {
                    id: rcScreen
                    visible: root.section === "claimfile"; Layout.fillWidth: true; spacing: root.sp4
                    readonly property string net: Logic.networkLabel(root.rcCheck && root.rcCheck.network ? root.rcCheck.network : (root.cfg && root.cfg.deployment ? root.cfg.deployment.network : ""))
                    readonly property var err: root.rcCheck && !root.rcCheck.ok && root.rcCheck.error ? root.rcCheck.error : null
                    readonly property bool checked: !!(root.rcCheck && root.rcCheck.ok)
                    RowLayout {
                        Layout.fillWidth: true
                        Heading { text: root.rcCheck && root.rcCheck.ok ? "Claim " + root.rcCheck.deposit.amount_text : "Claim a deposit"; Layout.fillWidth: true }
                        Chip { text: rcScreen.net; tone: "ink" }
                    }
                    Muted { Layout.fillWidth: true; text: Logic.RELAY_WORDING + " Your account needs no LEZ, and your wallet sends nothing and pays nothing." }

                    // 1. The claim file
                    Card {
                        visible: !rcScreen.checked && root.rcJob === null && !root.rcBusy
                        SubHeading { text: "Your claim file" }
                        Label { Layout.fillWidth: true; text: "The deposit page saved it when you deposited (logos-bridge-claim-….json). It holds the secret that claims the deposit: this app keeps it in memory only, writes it nowhere, and forgets it once the claim is done." }
                        RowLayout {
                            Layout.fillWidth: true; spacing: root.sp3
                            Btn { text: "Choose file…"; onClicked: root.pickClaimFile() }
                            Small { text: "or paste the file's text (or its full path) below"; Layout.fillWidth: true }
                        }
                        Small { visible: root.rcPickerProblem !== ""; text: root.rcPickerProblem; color: root.pal.error; Layout.fillWidth: true }
                        Rectangle {
                            Layout.fillWidth: true; implicitHeight: Math.max(96, pasteBox.implicitHeight + 2 * root.sp3)
                            radius: 6; color: root.cInset
                            border.color: Logic.claimSource(root.rcSource).problem !== "" ? root.pal.error : pasteBox.activeFocus ? root.pal.focus : root.pal.border
                            border.width: pasteBox.activeFocus || Logic.claimSource(root.rcSource).problem !== "" ? 2 : 1
                            TextEdit {
                                id: pasteBox
                                anchors.fill: parent; anchors.margins: root.sp3
                                text: root.rcSource; onTextChanged: if (text !== root.rcSource) root.rcSource = text
                                color: root.pal.text; font.family: root.mono; font.pixelSize: root.fsSmall
                                wrapMode: TextEdit.WrapAnywhere; selectByMouse: true; activeFocusOnTab: true
                                selectionColor: root.pal.primary; selectedTextColor: root.pal.onAccent
                                Accessible.role: Accessible.EditableText; Accessible.name: "Claim file text or path"
                            }
                            Text {
                                visible: root.rcSource === "" && !pasteBox.activeFocus; anchors.left: parent.left; anchors.top: parent.top; anchors.margins: root.sp3
                                text: "{ \"format\": \"logos-bridge-claim\", … }"; color: root.pal.textPlaceholder; font.family: root.mono; font.pixelSize: root.fsSmall
                            }
                        }
                        Small { visible: Logic.claimSource(root.rcSource).problem !== ""; text: Logic.claimSource(root.rcSource).problem; color: root.pal.error; Layout.fillWidth: true }
                        RowLayout {
                            Layout.fillWidth: true
                            Small { text: "Checking reads Sepolia and " + rcScreen.net + ". Nothing is signed or sent."; Layout.fillWidth: true }
                            Btn {
                                text: "Check the claim"; primary: true
                                enabledState: Logic.claimSource(root.rcSource).kind !== "empty" && Logic.claimSource(root.rcSource).problem === ""
                                why: "give the claim file first"
                                onClicked: root.checkClaimFile()
                            }
                        }
                    }

                    StateBox {
                        visible: root.rcBusy && root.rcJob === null
                        kind: "loading"; title: rcScreen.checked ? "Asking your Names Wallet" : "Checking the claim"
                        message: rcScreen.checked ? "Your Names Wallet builds the call itself before it shows it to you." : "Finding the deposit on Sepolia and the bridge's records on " + rcScreen.net + ". Nothing is signed or sent."
                    }
                    Card {
                        visible: root.rcError !== ""
                        tone: "bad"
                        Accessible.role: Accessible.AlertMessage
                        Accessible.name: Logic.walletProblemTitle(root.rcError) + ". " + root.rcError
                        Label { text: Logic.walletProblemTitle(root.rcError); font.weight: Font.Bold; color: root.pal.error; Layout.fillWidth: true }
                        Label { text: root.rcError; Layout.fillWidth: true }
                        Small { visible: !!(root.rcCheck && root.rcCheck.ok); Layout.fillWidth: true; text: "Nothing was signed or sent. The checked claim is still here: try again." }
                        RowLayout {
                            spacing: root.sp2
                            // Safe after a timeout too: the module looks for this claim's job and
                            // for a proposal that got no answer before it proposes anything.
                            Btn { visible: !!(root.rcCheck && root.rcCheck.ok && root.rcJob === null); text: "Try again"; primary: true; onClicked: root.startRelayedClaim() }
                            Btn { text: "Start over"; onClicked: root.restartClaimFile() }
                        }
                    }
                    // A proposal (or the programs' declaration) got no answer at all: it may be
                    // waiting in the Names app, and this app can't follow it. No second proposal
                    // until the person looked.
                    Card {
                        visible: root.rcUncertain !== null && root.rcJob === null
                        Accessible.role: Accessible.AlertMessage
                        Accessible.name: root.rcUncertain ? Logic.needsTitle("uncertain") + ". " + root.rcUncertain.message : ""
                        SubHeading { text: Logic.needsTitle("uncertain"); Layout.fillWidth: true }
                        Label { text: root.rcUncertain ? root.rcUncertain.message : ""; Layout.fillWidth: true }
                        RowLayout {
                            spacing: root.sp2
                            Btn {
                                text: root.rcUncertain && root.rcUncertain.uncertain === "declare" ? "Continue" : "Propose again"; primary: true
                                enabledState: !root.rcBusy
                                onClicked: root.startRelayedClaim(true)
                            }
                            Btn { text: "Start over"; onClicked: root.restartClaimFile() }
                        }
                    }

                    // A refusal from the check: specific, with what to do.
                    Card {
                        visible: rcScreen.err !== null
                        tone: "bad"
                        Accessible.role: Accessible.AlertMessage
                        Accessible.name: rcScreen.err ? Logic.relayErrorTitle(rcScreen.err.code) + ". " + rcScreen.err.message : ""
                        Label { text: rcScreen.err ? Logic.relayErrorTitle(rcScreen.err.code) : ""; font.weight: Font.Bold; color: root.pal.error; Layout.fillWidth: true }
                        Label { text: rcScreen.err ? rcScreen.err.message : ""; Layout.fillWidth: true }
                        Small { Layout.fillWidth: true; text: "Nothing was signed or sent. The claim file was read here and is not kept." }
                        RowLayout {
                            spacing: root.sp2
                            Btn { text: "Choose another file"; onClicked: root.restartClaimFile() }
                        }
                    }

                    // 2. Checked: what will happen, and who pays.
                    Card {
                        visible: rcScreen.checked && root.rcJob === null
                        SubHeading { text: "What this claim does" }
                        KV { k: "Amount"; v: root.rcCheck && root.rcCheck.ok ? root.rcCheck.deposit.amount_text + " (wrapped, on " + rcScreen.net + ")" : "" }
                        KV { k: "Into"; v: root.rcCheck && root.rcCheck.ok ? root.rcCheck.deposit.account + ", your public account" : ""; monoValue: false }
                        KV { k: "From"; v: root.rcCheck && root.rcCheck.ok ? "Sepolia deposit, leaf " + root.rcCheck.deposit.leaf_index + " of " + root.rcCheck.deposit.leaves + " (final)" : "" }
                        KV { k: "Signs"; v: "Your Names Wallet, after you approve its sheet. It sends nothing and pays nothing." }
                        KV { k: "Sends and pays"; v: root.rcCheck && root.rcCheck.ok ? "The bridge's relay, " + Logic.shortId(root.rcCheck.relay.payer.replace("Public/", "")) + ": at most " + Logic.group(root.rcCheck.relay.max_fee) + " LEZ of its own. Your account needs no LEZ." : "" }
                        SubHeading { text: "Checked"; Layout.topMargin: root.sp3 }
                        Repeater {
                            model: root.rcCheck && root.rcCheck.ok ? root.rcCheck.checks : []
                            delegate: RowLayout {
                                Layout.fillWidth: true; spacing: root.sp2
                                Rectangle { width: 16; height: 16; radius: 8; color: root.pal.text; Layout.alignment: Qt.AlignTop; Layout.topMargin: 2; Text { anchors.centerIn: parent; text: "✓"; color: root.pal.background; font.pixelSize: 10; font.bold: true } }
                                Label { text: modelData.text; Layout.fillWidth: true }
                            }
                        }
                    }
                    // The privacy cost, in the wallet's own words: a public claim is not private.
                    Rectangle {
                        visible: rcScreen.checked && root.rcJob === null
                        Layout.fillWidth: true; implicitHeight: privCol.implicitHeight + 2 * root.sp3
                        radius: root.radius; color: root.soft("bad"); border.color: root.edge("bad"); border.width: 1.5
                        Accessible.role: Accessible.StaticText
                        Accessible.name: "Privacy: Not private. " + (root.rcCheck && root.rcCheck.privacy ? root.rcCheck.privacy : "")
                        RowLayout {
                            id: privCol; anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; anchors.margins: root.sp3; spacing: root.sp3
                            Shield { kind: "shield-open"; color: root.pal.error; Layout.alignment: Qt.AlignTop }
                            Label { text: "Not private"; font.weight: Font.Bold; color: root.pal.error; Layout.alignment: Qt.AlignTop; wrapMode: Text.NoWrap }
                            Label { text: (root.rcCheck && root.rcCheck.privacy ? root.rcCheck.privacy : "") + " A deposit made for a private account is claimed privately instead (Deposits and claims)."; Layout.fillWidth: true }
                        }
                    }
                    Card {
                        visible: rcScreen.checked && root.rcJob === null
                        SubHeading { text: "Approve in your Names Wallet" }
                        Label { Layout.fillWidth: true; text: root.rcCheck && root.rcCheck.ok ? root.rcCheck.approval : "" }
                        Note {
                            visible: !!(root.rcCheck && root.rcCheck.ok && root.rcCheck.wallet && root.rcCheck.wallet.needs)
                            Label {
                                Layout.fillWidth: true
                                text: !root.rcCheck || !root.rcCheck.ok || !root.rcCheck.wallet ? "" :
                                      Logic.walletCheckNote(root.rcCheck.wallet.needs) !== "" ? Logic.walletCheckNote(root.rcCheck.wallet.needs) :
                                      root.rcCheck.wallet.needs === "connect" ? "First your Names app asks you to connect this app and give it " + root.rcCheck.deposit.account + "." :
                                      root.rcCheck.wallet.needs === "public_account" ? "This app hasn't been given " + root.rcCheck.deposit.account + " yet: your Names app asks for it first. If that account isn't in your wallet, this claim file is for someone else." :
                                      root.rcCheck.wallet.needs === "network" ? "Your Names Wallet is on LEZ 0.3: switch it to LEZ 0.4 (the bridge test network) in the Names app first." : ""
                            }
                        }
                        RowLayout {
                            Layout.fillWidth: true
                            Btn { text: "Start over"; onClicked: root.restartClaimFile() }
                            Item { Layout.fillWidth: true }
                            Btn {
                                text: root.rcBusy ? "Asking…" : "Send to my Names Wallet"; primary: true
                                enabledState: !root.rcBusy && !!(root.rcCheck && root.rcCheck.ok && !(root.rcCheck.wallet && root.rcCheck.wallet.needs === "wallet"))
                                why: "the Names Wallet is not answering"
                                onClicked: root.startRelayedClaim()
                            }
                        }
                    }

                    // 3. The claim on its way.
                    Card {
                        visible: root.rcJob !== null
                        RowLayout {
                            Layout.fillWidth: true
                            Chip { text: root.rcJob ? Logic.jobLook(root.rcJob.state).word : ""; tone: root.rcJob ? Logic.jobLook(root.rcJob.state).tone : "muted" }
                            Label { text: root.rcJob ? (root.rcJob.amount_text || "") + " into " + Logic.shortId(String(root.rcJob.account || "").replace("Public/", "")) : ""; Layout.fillWidth: true; font.weight: Font.DemiBold }
                            Label { text: root.rcJob ? Logic.clockText(root.rcJob.elapsed_s) : ""; font.pixelSize: 22; font.weight: Font.DemiBold; wrapMode: Text.NoWrap; Accessible.name: root.rcJob ? "Elapsed " + Logic.durationText(root.rcJob.elapsed_s) : "" }
                        }
                        RowLayout {
                            Layout.fillWidth: true; spacing: 0; Layout.topMargin: root.sp2
                            Repeater {
                                model: Logic.RELAY_PHASES
                                delegate: ColumnLayout {
                                    Layout.fillWidth: true; Layout.preferredWidth: 1; spacing: 4; Layout.alignment: Qt.AlignTop
                                    readonly property bool ended: root.rcJob !== null && root.rcJob.state !== "running"
                                    readonly property int at: { var p = root.rcJob ? root.rcJob.phase : ""; for (var i = 0; i < Logic.RELAY_PHASES.length; i++) if (Logic.RELAY_PHASES[i].key === p) return i; return p === "done" ? 3 : 0 }
                                    readonly property bool current: !ended && at === index
                                    readonly property bool done: index < at || (root.rcJob !== null && root.rcJob.state === "done")
                                    Accessible.role: Accessible.StaticText
                                    Accessible.name: modelData.word + (done ? ", done" : current ? ", in progress" : ", not reached")
                                    Rectangle { Layout.fillWidth: true; height: 6; radius: 3; Layout.rightMargin: 4; color: parent.done ? root.pal.text : parent.current ? root.pal.primary : root.cTrack }
                                    Label { text: modelData.word; font.weight: parent.done || parent.current ? Font.DemiBold : Font.Normal; color: parent.done || parent.current ? root.pal.text : root.pal.textTertiary }
                                    Small { text: modelData.hint; Layout.fillWidth: true; Layout.rightMargin: root.sp2 }
                                }
                            }
                        }
                        Note { visible: !!(root.rcJob && root.rcJob.note); Label { Layout.fillWidth: true; text: root.rcJob && root.rcJob.note ? root.rcJob.note : "" } }
                        Card {
                            visible: !!(root.rcJob && root.rcJob.error)
                            tone: "bad"
                            Accessible.role: Accessible.AlertMessage
                            Label { text: root.rcJob && root.rcJob.error ? Logic.relayErrorTitle(root.rcJob.error.code) : ""; font.weight: Font.Bold; color: root.pal.error; Layout.fillWidth: true }
                            Label { text: root.rcJob && root.rcJob.error ? root.rcJob.error.message : ""; Layout.fillWidth: true }
                        }
                        Card {
                            visible: !!(root.rcJob && root.rcJob.state === "done" && root.rcJob.result)
                            tone: "ink"
                            Label { Layout.fillWidth: true; font.weight: Font.DemiBold; text: root.rcJob && root.rcJob.result ? "✓  Claimed in block " + root.rcJob.result.block + " on " + rcScreen.net : "" }
                            KV { k: "Transaction"; v: root.rcJob && root.rcJob.result ? root.rcJob.result.tx : ""; monoValue: true }
                            KV { k: "Now in the account"; v: root.rcJob && root.rcJob.result ? root.rcJob.result.balance_text + " (wrapped); native LEZ " + Logic.group(root.rcJob.result.native) : "" }
                            Small { Layout.fillWidth: true; text: root.rcJob && root.rcJob.result ? root.rcJob.result.wallet_note : "" }
                        }
                        RowLayout {
                            spacing: root.sp2
                            Btn { text: "Cancel"; visible: !!(root.rcJob && root.rcJob.can_cancel); onClicked: root.cancelRelayed() }
                            Btn { text: "Check now"; visible: !!(root.rcJob && root.rcJob.state === "running"); onClicked: root.pollRelayed() }
                            Btn { text: "Claim another deposit"; visible: !!(root.rcJob && root.rcJob.state !== "running"); onClicked: root.restartClaimFile() }
                        }
                    }
                }

                // ── Jobs / proving ──
                ColumnLayout {
                    visible: root.section === "proving"; Layout.fillWidth: true; spacing: root.sp4
                    Heading { text: root.job ? (root.job.kind === "claim" ? "Claim" : root.job.kind === "redeem" ? "Redemption" : "Job") + (root.job.dry_run ? " (dry run)" : "") : "Jobs" }
                    StateBox { visible: !!(root.job === null && root.jobsList.length === 0); kind: "empty"; title: "No jobs yet"; message: "Claims, redemptions and admin actions run here as jobs you can follow, pause and cancel." }
                    Card {
                        visible: root.job !== null
                        RowLayout {
                            Layout.fillWidth: true
                            Chip { text: root.job ? Logic.jobLook(root.job.state).word : ""; tone: root.job ? Logic.jobLook(root.job.state).tone : "muted" }
                            Mono { text: root.job ? root.job.id : ""; color: root.pal.textTertiary; Layout.fillWidth: true }
                            Label {
                                visible: !!(root.job && !(root.job.dry_run && root.job.state === "done"))
                                text: root.job ? Logic.clockText(root.job.elapsed_s) : ""; font.pixelSize: 26; font.weight: Font.DemiBold; wrapMode: Text.NoWrap
                                Accessible.name: root.job ? "Elapsed " + Logic.durationText(root.job.elapsed_s) : ""
                            }
                        }
                        // Phase stepper
                        RowLayout {
                            Layout.fillWidth: true; spacing: 0; Layout.topMargin: root.sp2
                            Repeater {
                                model: root.job && root.job.wallet === "names" ? Logic.WALLET_PHASES : Logic.PHASES
                                delegate: ColumnLayout {
                                    Layout.fillWidth: true; Layout.preferredWidth: 1; spacing: 4; Layout.alignment: Qt.AlignTop
                                    readonly property bool ended: root.job !== null && !(root.job.state === "running" || root.job.state === "paused")
                                    readonly property bool reached: { var l = root.job && root.job.phases ? root.job.phases : []; for (var i = 0; i < l.length; i++) if (l[i].phase === modelData.key) return true; return false }
                                    readonly property bool current: root.job !== null && !ended && root.job.phase === modelData.key
                                    readonly property bool done: reached && !current
                                    readonly property bool skipped: ended && !reached && root.job.state === "done"
                                    Accessible.role: Accessible.StaticText
                                    Accessible.name: modelData.word + (done ? ", done" : current ? ", in progress" : skipped ? ", skipped in a dry run" : ", not reached")
                                    Rectangle {
                                        Layout.fillWidth: true; height: 6; radius: 3; Layout.rightMargin: 4
                                        color: parent.done ? root.pal.text : parent.current ? root.pal.primary : parent.skipped ? "transparent" : root.cTrack
                                        border.color: parent.skipped ? root.pal.border : "transparent"; border.width: 1
                                    }
                                    Label { text: modelData.word + (parent.skipped ? ", skipped" : ""); font.weight: parent.done || parent.current ? Font.DemiBold : Font.Normal; color: parent.done || parent.current ? root.pal.text : root.pal.textTertiary }
                                    Small { text: modelData.hint; Layout.fillWidth: true; Layout.rightMargin: root.sp2 }
                                }
                            }
                        }
                        ColumnLayout {
                            visible: !!(root.job !== null && root.job.phase === "prove" && root.job.prove_estimate_s > 0)
                            Layout.fillWidth: true; spacing: 4; Layout.topMargin: root.sp2
                            RowLayout {
                                Layout.fillWidth: true
                                Label { text: "Proof: " + (root.job ? Logic.durationText(root.job.phase_elapsed_s) : "") + " of about " + (root.job ? Logic.durationText(root.job.prove_estimate_s) : ""); Layout.fillWidth: true }
                                Small { text: "estimate, not progress" }
                            }
                            Rectangle {
                                Layout.fillWidth: true; height: 8; radius: 4; color: root.cTrack
                                Rectangle { height: parent.height; radius: 4; color: root.job && root.job.state === "paused" ? root.pal.textSecondary : root.pal.primary; width: parent.width * (root.job ? Logic.estimateFraction(root.job.phase_elapsed_s, root.job.prove_estimate_s) : 0) }
                            }
                        }
                        GridLayout {
                            visible: !!(root.job !== null && root.job.prove_estimate_s > 0 && (root.job.phase === "prove" || root.job.peak_rss_mb > 500))
                            Layout.fillWidth: true; columns: 3; columnSpacing: root.sp3; Layout.topMargin: root.sp2
                            Repeater {
                                model: root.job ? [
                                    { k: "Memory (resident)", v: root.job.peak_rss_mb ? Logic.gb(root.job.peak_rss_mb) + " peak so far" : "—" },
                                    { k: "Expected peak", v: Logic.gb(root.job.peak_estimate_mb) + " footprint" },
                                    { k: "Threads", v: "4 (CPU prover)" }
                                ] : []
                                delegate: ColumnLayout { spacing: 2; Layout.fillWidth: true; Small { text: modelData.k } Label { text: modelData.v; font.weight: Font.DemiBold } }
                            }
                        }
                        Note {
                            visible: !!(root.job !== null && (root.job.state === "running" || root.job.state === "paused") && root.job.prove_estimate_s > 0)
                            Label { Layout.fillWidth: true; text: "Keep Basecamp open until this finishes: the app follows the proof only while Basecamp runs. If it closes, reopen the app before trying again: the job then shows how it ended, or that its outcome is unknown. A private proof peaked at 9.4 GB of memory on a 16 GB Apple M4 desktop; close other large apps first." }
                        }
                        Note { visible: !!(root.job !== null && root.job.dry_run_forced !== undefined); Label { Layout.fillWidth: true; text: root.job && root.job.dry_run_forced ? root.job.dry_run_forced : "" } }
                        Note { visible: !!(root.job !== null && root.job.note !== undefined && root.job.note !== null); Label { Layout.fillWidth: true; text: root.job && root.job.note ? root.job.note : "" } }
                        ErrorNote { visible: !!(root.job !== null && root.job.error !== null && root.job.error !== undefined); code: root.job && root.job.error ? root.job.error.code : ""; message: root.job && root.job.error ? root.job.error.message : ""; claimContext: root.job !== null && root.job.kind === "claim" && root.job.wallet !== "names" }
                        Card {
                            visible: !!(root.job !== null && root.job.state === "done" && root.job.result !== null)
                            tone: "ink"
                            Label { Layout.fillWidth: true; font.weight: Font.DemiBold; text: root.job && root.job.result ? "✓  " + (root.job.result.block ? "Applied in LEZ block " + root.job.result.block : root.job.result.dry_run ? "Dry run passed: simulated in " + root.job.result.simulated_in + ". Nothing was proven or sent." : root.job.result.tx ? "Included: transaction " + Logic.shortId(root.job.result.tx) : "Finished") : "" }
                            Small { Layout.fillWidth: true; text: root.job && root.job.result && root.job.result.recipient ? "Recipient " + root.job.result.recipient + (root.job.result.recipient_kind === "one_time" ? " (one-time address)" : "") : "" }
                        }
                        RowLayout {
                            spacing: root.sp2
                            Btn { text: "Pause"; visible: !!(root.job && root.job.state === "running" && root.job.wallet !== "names"); enabledState: !!(root.job && root.job.can_pause); why: "a proof under the memory guard is paused by the guard itself"; onClicked: root.jobAction("pause_job") }
                            Btn { text: "Resume"; primary: true; visible: !!(root.job && root.job.state === "paused"); onClicked: root.jobAction("resume_job") }
                            Btn { text: "Cancel"; visible: !!(root.job && root.job.can_cancel); onClicked: root.jobAction("cancel_job") }
                        }
                        Small { visible: !!(root.job && root.job.state === "running" && !root.job.can_pause); Layout.fillWidth: true; text: root.job && root.job.wallet === "names" ? "Your Names Wallet runs this: pausing it is up to the Names app. You can cancel it here until it is being sent." : "Pause is not offered while the memory guard watches this proof: the guard pauses it by itself when memory runs low." }
                        Small { visible: root.jobError !== ""; text: root.jobError; color: root.pal.error; Layout.fillWidth: true }
                        Rectangle {
                            Layout.fillWidth: true; implicitHeight: logText.implicitHeight + 2 * root.sp3; radius: 6; color: root.cInset
                            visible: !!(root.job !== null && root.job.log_tail !== "")
                            Mono { id: logText; anchors.fill: parent; anchors.margins: root.sp3; text: root.job ? root.job.log_tail : ""; color: root.pal.textSecondary }
                        }
                    }
                    SubHeading { visible: root.jobsList.length > 0; text: "All jobs" }
                    Repeater {
                        model: root.jobsList
                        delegate: Rectangle {
                            Layout.fillWidth: true; implicitHeight: 40; radius: 6; color: root.pal.backgroundSecondary; border.color: root.cLine
                            RowLayout {
                                anchors.fill: parent; anchors.leftMargin: root.sp3; anchors.rightMargin: root.sp2; spacing: root.sp3
                                Chip { text: Logic.jobLook(modelData.state).word; tone: Logic.jobLook(modelData.state).tone }
                                Label { text: Logic.capitalized(modelData.kind) + (modelData.dry_run ? " (dry run)" : "") + ", " + modelData.subject; Layout.fillWidth: true; elide: Text.ElideRight; wrapMode: Text.NoWrap }
                                Small { text: Logic.durationText(modelData.elapsed_s) }
                                Btn { text: "Open"; onClicked: root.showJob(modelData) }
                            }
                        }
                    }
                }

                // ── Holdings ──
                ColumnLayout {
                    visible: root.section === "holdings"; Layout.fillWidth: true; spacing: root.sp4
                    RowLayout { Layout.fillWidth: true; Heading { text: "Holdings"; Layout.fillWidth: true } Small { text: root.holdings && root.holdings.as_of ? (root.holdings.stale ? "Updating… " : "") + "read " + Logic.durationText((root.now - root.holdings.as_of) / 1000) + " ago" : "" } Btn { text: "Refresh"; onClicked: root.loadHoldings() } }
                    StateBox { visible: !!((root.holdings === null || root.holdings.reading) && root.holdingsError === ""); kind: "loading"; title: "Reading your wrapped tokens"; message: root.holdings && root.holdings.message ? root.holdings.message : "The wallet catches up with the chain first." }
                    StateBox { visible: root.holdingsError !== ""; kind: "error"; title: "Could not read your holdings"; message: root.holdingsError; action: "Try again"; onActed: root.loadHoldings() }
                    GridLayout {
                        visible: !!(root.holdings !== null && !root.holdings.reading); Layout.fillWidth: true; columns: 2; columnSpacing: root.sp3
                        Repeater {
                            model: root.holdings && root.holdings.totals ? root.holdings.totals : []
                            delegate: Rectangle {
                                Layout.fillWidth: true; Layout.preferredWidth: 1; implicitHeight: 84; radius: root.radius; color: root.pal.backgroundSecondary; border.color: root.cLine
                                ColumnLayout { anchors.fill: parent; anchors.margins: root.sp4; spacing: 2; Small { text: "Wrapped " + modelData.symbol + " on LEZ" } Label { text: modelData.balance_text; font.pixelSize: 26; font.weight: Font.Bold } }
                            }
                        }
                    }
                    Small { visible: !!(root.holdings && root.holdings.note); text: root.holdings && root.holdings.note ? root.holdings.note : ""; Layout.fillWidth: true }
                    Repeater {
                        model: root.holdings && root.holdings.accounts ? root.holdings.accounts : []
                        delegate: Card {
                            RowLayout {
                                Layout.fillWidth: true; spacing: root.sp2
                                Chip { text: modelData.kind === "private" ? "Private" : "Public"; tone: modelData.kind === "private" ? "ink" : "muted"; glyph: modelData.kind === "private" ? "shield" : "" }
                                Mono { text: modelData.account; Layout.fillWidth: true }
                            }
                            Repeater { model: modelData.tokens; delegate: KV { k: modelData.symbol; v: modelData.balance_text } }
                            Small { visible: modelData.kind === "public"; text: "Anyone can see this account's balance. Burning from it shows which account redeemed."; Layout.fillWidth: true }
                        }
                    }
                    StateBox { visible: !!(root.holdings && root.holdings.accounts && root.holdings.accounts.length === 0); kind: "empty"; title: "No wrapped tokens in this wallet"; message: "Claimed deposits show here."; action: "Deposits and claims"; onActed: root.go("pending") }
                }

                // ── Redeem ──
                ColumnLayout {
                    visible: root.section === "redeem"; Layout.fillWidth: true; spacing: root.sp4
                    Heading { text: "Redeem to Ethereum" }
                    PrivacyRail { report: root.rdmPlan ? root.rdmPlan.privacy : null }
                    Note {
                        visible: !!(root.registry && root.registry.vault && root.registry.vault.release_kind === "operator")
                        Label { Layout.fillWidth: true; font.weight: Font.DemiBold; text: "Releases on this vault are signed by the bridge operator" }
                        Label { Layout.fillWidth: true; text: "An operator-attested demo vault: the operator's attestor key signs each release (no zk proof is checked on Ethereum), within the vault's caps and after its delay. Not trustless. No signing service runs yet, so a burn waits until the operator releases it." }
                    }
                    Note {
                        visible: !!(root.registry && root.registry.vault && !root.registry.vault.exits_possible)
                        Label { Layout.fillWidth: true; font.weight: Font.DemiBold; text: "This deployment cannot release on Ethereum" }
                        Label { Layout.fillWidth: true; text: (root.registry && root.registry.vault && root.registry.vault.release_note ? root.registry.vault.release_note + " " : "The outbound proof (G2) that tells the vault about LEZ burns is not built, and this demo vault's outbound image is a placeholder. ") + (root.namesWallet ? "You can plan a redemption and see the burn your wallet would be asked to approve; it is not proposed, and nothing is burned." : "You can plan a redemption and run it as a dry run; nothing is burned.") }
                        Small { Layout.fillWidth: true; visible: text !== ""; text: root.registry && root.registry.vault && root.registry.vault.release_detail ? root.registry.vault.release_detail : "" }
                    }
                    Card {
                        Small { text: "Token"; font.weight: Font.DemiBold }
                        RowLayout {
                            spacing: root.sp2
                            Repeater { model: root.tokens; delegate: Option { Layout.fillWidth: false; Layout.preferredWidth: 160; title: modelData.symbol; selected: root.rdmToken === index; onChosen: { root.rdmToken = index; root.rdmPlan = null } } }
                        }
                        Small { text: "Amount"; font.weight: Font.DemiBold; Layout.topMargin: root.sp2 }
                        Field { id: rdmAmountField; label: "Amount to redeem"; text: root.rdmAmount; onTextChanged: root.rdmAmount = text; problem: root.tokens.length ? Logic.amountProblem(text, root.tokens[root.rdmToken].decimals) : "" }
                        Small { text: "Ethereum recipient"; font.weight: Font.DemiBold; Layout.topMargin: root.sp2 }
                        Field { id: rdmToField; label: "Ethereum recipient address"; placeholder: "0x…"; monoFont: true; text: root.rdmTo; onTextChanged: root.rdmTo = text; problem: Logic.ethAddressProblem(text) }
                        Small { visible: rdmToField.problem !== ""; text: rdmToField.problem; color: root.pal.error }
                        Small { Layout.fillWidth: true; text: "Fixed when you burn and public on Ethereum. Use a fresh address that isn't linked to you. A contract that can't receive this token makes the release fail." }
                        Small { text: "Who submits the release"; font.weight: Font.DemiBold; Layout.topMargin: root.sp2 }
                        Option {
                            selected: root.rdmRelay > 0; title: "A relayer, paid from the amount (recommended)"; badge: "Private"; badgeTone: "ink"; badgeGlyph: "shield"
                            detail: root.tokens.length && root.tokens[root.rdmToken].denoms.length ? "Fee from the vault's fixed menu: " + root.tokens[root.rdmToken].denoms[0].relay_fee_menu.map(function(m, i) { return (i + 1) + ") " + m.pct }).join("  ") + ". The same menu for everyone, so the fee doesn't single you out." : ""
                            onChosen: root.rdmRelay = 1
                        }
                        RowLayout {
                            visible: !!(root.rdmRelay > 0 && root.tokens.length > 0); spacing: root.sp2; Layout.leftMargin: root.sp5
                            Repeater {
                                model: root.tokens.length && root.tokens[root.rdmToken].denoms.length ? root.tokens[root.rdmToken].denoms[0].relay_fee_menu : []
                                delegate: Option { Layout.fillWidth: false; Layout.preferredWidth: 150; title: modelData.pct + " fee"; selected: root.rdmRelay === index + 1; onChosen: root.rdmRelay = index + 1 }
                            }
                        }
                        Option {
                            selected: root.rdmRelay === 0; title: "No relay fee: I or a volunteer submit it"; badge: "Can link you"; badgeTone: "warn"
                            detail: "Whoever sends the release pays its gas from an Ethereum address, and that address's history can identify them."
                            onChosen: root.rdmRelay = 0
                        }
                        RowLayout {
                            Layout.fillWidth: true; Layout.topMargin: root.sp2
                            Item { Layout.fillWidth: true }
                            Btn { text: root.rdmBusy ? "Planning…" : "Plan redemption"; primary: true; enabledState: !!(!root.rdmBusy && rdmToField.problem === "" && root.rdmTo !== "" && rdmAmountField.problem === ""); why: "enter an amount and a recipient"; onClicked: root.planRedeem() }
                        }
                    }
                    ErrorNote { visible: root.rdmError !== ""; code: "cli_error"; message: root.rdmError }
                    Card {
                        visible: root.rdmBuilt !== null
                        SubHeading { text: "The burn your wallet would be asked to approve" }
                        KV { k: "Programs"; v: root.rdmBuilt ? root.rdmBuilt.call.programs.map(function(p) { return Logic.shortId(p) }).join(", ") : "" }
                        KV { k: "It takes"; v: root.rdmBuilt && root.rdmBuilt.call.declared.debits ? Logic.group(root.rdmBuilt.call.declared.debits[0].amount) + " base units of the token, nothing else" : "" }
                        KV { k: "Fee at most"; v: root.rdmBuilt ? Logic.group(root.rdmBuilt.call.declared.max_fee) + " LEZ" : "" }
                        KV { k: "Message"; v: root.rdmBuilt ? root.rdmBuilt.call.message_bytes + " bytes" : "" }
                        Small { Layout.fillWidth: true; text: root.rdmBuilt ? root.rdmBuilt.blocked : "" }
                    }
                    Card {
                        visible: root.rdmPlan !== null
                        SubHeading { text: root.rdmPlan ? "Burn " + root.rdmPlan.amount_text + " " + root.rdmPlan.symbol + ", receive " + root.rdmPlan.receive_text + " on Ethereum" : "" }
                        Repeater {
                            model: root.rdmPlan ? root.rdmPlan.pieces : []
                            delegate: RowLayout {
                                Layout.fillWidth: true; spacing: root.sp3
                                Label { text: modelData.denom_text + " " + root.rdmPlan.symbol; font.weight: Font.DemiBold; Layout.preferredWidth: 120 }
                                Small { text: "from " + Logic.shortId(modelData.from) + " (" + modelData.from_kind + ") · relay fee " + modelData.relay_fee_text + " · you receive " + modelData.receive_text; Layout.fillWidth: true }
                            }
                        }
                        KV { k: "Recipient"; v: root.rdmPlan ? root.rdmPlan.eth_to : ""; monoValue: true }
                        Repeater { model: root.rdmPlan ? root.rdmPlan.warnings : []; delegate: Note { Label { text: modelData; Layout.fillWidth: true } } }
                        Card { visible: !!(root.rdmPlan && root.rdmPlan.blocked); tone: "bad"; Label { text: root.rdmPlan && root.rdmPlan.blocked ? root.rdmPlan.blocked : ""; Layout.fillWidth: true } }
                        Check {
                            visible: !!(root.rdmPlan && root.rdmPlan.privacy.requires_confirmation); checked: root.rdmConfirm
                            text: Logic.acceptText(root.rdmPlan ? root.rdmPlan.privacy : null, "this redemption")
                            tone: root.rdmPlan && root.rdmPlan.privacy.level === "exposed" ? "bad" : "warn"
                            onToggled: root.rdmConfirm = !root.rdmConfirm
                        }
                        RowLayout {
                            Layout.fillWidth: true
                            Item { Layout.fillWidth: true }
                            Btn { text: !root.namesWallet ? "Burn (dry run)" : root.rdmPlan && root.rdmPlan.exits_possible ? "Send the next piece to my Names Wallet" : "Build the burn (not proposed)"; primary: true; enabledState: !!(root.rdmPlan && (!root.rdmPlan.privacy.requires_confirmation || root.rdmConfirm) && root.activeJob === null); why: root.activeJob !== null ? "another job is running" : "tick the confirmation first"; onClicked: root.startRedeem() }
                        }
                    }
                }

                // ── Exits ──
                ColumnLayout {
                    visible: root.section === "exits"; Layout.fillWidth: true; spacing: root.sp4
                    Heading { text: "Exits and release" }
                    Note {
                        id: exitsNote
                        readonly property bool operator: !!(root.registry && root.registry.vault && root.registry.vault.release_kind === "operator")
                        readonly property string note: root.registry && root.registry.vault && root.registry.vault.release_note ? root.registry.vault.release_note : ""
                        readonly property string detail: root.registry && root.registry.vault && root.registry.vault.release_detail ? root.registry.vault.release_detail : ""
                        Label { Layout.fillWidth: true; font.weight: Font.DemiBold; text: exitsNote.operator ? "Releases on this vault are signed by the bridge operator" : exitsNote.note ? "This deployment cannot release on Ethereum" : "Releasing on Ethereum is not possible yet" }
                        Label { Layout.fillWidth: true; text: exitsNote.operator
                            ? "An operator-attested demo vault: the operator's attestor key signs each release (no zk proof is checked on Ethereum), within the vault's caps and after its delay. Not trustless. This app can't release by itself and no signing service runs yet: ask the operator. Then anyone (you, or a relayer paid the fee you chose) can release your exit to the recipient you fixed when burning."
                            : exitsNote.note ? exitsNote.note
                            : "A release needs the outbound proof (G2), which proves LEZ burns to the vault. It is not built, so no exit can be released today. When it exists: the vault records exit roots; after the release delay you, or a relayer paid the fee you chose, prove your exit, and the vault pays the recipient you fixed when burning." }
                        Small { Layout.fillWidth: true; visible: exitsNote.detail !== ""; text: exitsNote.detail }
                    }
                    StateBox { visible: root.exitsList.length === 0; kind: "empty"; title: "No exits on this computer"; message: "Redemptions you plan and burn show here, with what each waits for."; action: "Redeem"; onActed: root.go("redeem") }
                    Repeater {
                        model: root.exitsList
                        delegate: Card {
                            RowLayout {
                                Layout.fillWidth: true
                                SubHeading { text: modelData.denom_text + " " + modelData.symbol + " → " + Logic.shortId(modelData.eth_to); Layout.fillWidth: true }
                                Chip { text: modelData.stage === "burned" ? "Burned" : modelData.stage === "simulated" ? "Dry run only" : modelData.stage === "proposed" ? "Waiting in your wallet" : "Planned"; tone: modelData.stage === "burned" ? "solid" : modelData.stage === "proposed" ? "accent" : "muted" }
                            }
                            KV { k: "You receive"; v: modelData.receive_text + " " + modelData.symbol + " (relay fee " + modelData.relay_fee_text + ")" }
                            KV { k: "Exit commitment"; v: modelData.exit_commitment; monoValue: true }
                            RowLayout {
                                Layout.fillWidth: true
                                Small { text: modelData.release.vault_note ? modelData.release.vault_note : modelData.release.reason; Layout.fillWidth: true }
                                Btn { text: "Release"; enabledState: false; why: modelData.release.reason }
                            }
                        }
                    }
                }

                // ── Recovery ──
                ColumnLayout {
                    visible: root.section === "recover"; Layout.fillWidth: true; spacing: root.sp4
                    Heading { text: "Recovery" }
                    Muted { Layout.fillWidth: true; text: "Find your deposits and exits again: unclaimed deposits by matching the vault's deposits against your secrets, unreleased exits by your burns." }
                    Note {
                        visible: !root.namesWallet
                        Label { Layout.fillWidth: true; font.weight: Font.DemiBold; text: "Recovery from your wallet's seed needs the Names Wallet" }
                        Label { Layout.fillWidth: true; text: "With the lez-bridge program's own wallet folder, this finds only positions whose secrets are on this computer (or run `lez-bridge recover` there). Keep the bridge's data folder backed up." }
                    }
                    Note {
                        visible: root.namesWallet
                        Label { Layout.fillWidth: true; font.weight: Font.DemiBold; text: "From your Names Wallet" }
                        Label { Layout.fillWidth: true; text: "Your wallet makes this app's deposit secrets from your account, so they come back when you restore it from its recovery phrase. The scan asks it for them in batches and matches them against the vault's public deposits; nothing leaves this computer but the vault's event reads." }
                    }
                    Card {
                        Small { text: "LEZ account"; font.weight: Font.DemiBold }
                        Field { label: "LEZ account to recover"; monoFont: true; text: root.recAccount; onTextChanged: root.recAccount = text }
                        Small { text: "Gap limit (deposits past the last one found)"; font.weight: Font.DemiBold }
                        RowLayout {
                            Field { label: "Gap limit"; text: root.recGap; onTextChanged: root.recGap = text; Layout.preferredWidth: 120; Layout.fillWidth: false }
                            Item { Layout.fillWidth: true }
                            Btn { text: "Scan"; primary: true; enabledState: root.recAccount !== ""; onClicked: root.recover() }
                        }
                    }
                    ErrorNote { visible: root.recError !== ""; code: "cli_error"; message: root.recError }
                    Card {
                        id: recCard
                        visible: root.recResult !== null
                        readonly property var found: root.recResult ? (root.recResult.found || root.recResult.found_on_this_computer || []) : []
                        SubHeading { text: root.recResult ? Logic.plural(recCard.found.length, "deposit", "deposits") + " found" : "" }
                        Repeater {
                            model: recCard.found
                            delegate: RowLayout { Layout.fillWidth: true; Label { text: modelData.denom_text + " " + modelData.symbol; font.weight: Font.DemiBold; Layout.preferredWidth: 120 } Small { text: Logic.stageLook(modelData.stage).word + (modelData.leaf_index !== undefined ? ", leaf " + modelData.leaf_index + " in the vault" : ""); Layout.fillWidth: true } }
                        }
                        Small { text: !root.recResult ? "" : Array.isArray(root.recResult.scanned) ? "Read every amount of every token, up to index " + root.recResult.scanned.reduce(function(m, x) { return Math.max(m, x.to_index) }, 0) + "." : "Scanned " + root.recResult.scanned + "."; Layout.fillWidth: true }
                        Small { visible: !!(root.recResult && root.recResult.exits); text: root.recResult && root.recResult.exits ? root.recResult.exits.reason : ""; Layout.fillWidth: true }
                    }
                }

                // ── Settings ──
                ColumnLayout {
                    visible: root.section === "settings"; Layout.fillWidth: true; spacing: root.sp4
                    Heading { text: "Settings" }
                    Card {
                        SubHeading { text: "Ethereum" }
                        Small { text: "RPC for reads (chain " + (root.cfg ? root.cfg.deployment.chain_id : "") + ")"; font.weight: Font.DemiBold }
                        Field { label: "Ethereum RPC address"; text: root.setRpc; onTextChanged: root.setRpc = text; monoFont: true }
                        Small { text: "RPC for storage proofs (must serve old blocks)"; font.weight: Font.DemiBold }
                        Field { label: "Ethereum RPC for storage proofs"; text: root.setProofRpc; onTextChanged: root.setProofRpc = text; monoFont: true }
                        Small { Layout.fillWidth: true; text: "The RPC sees your network address and which blocks you ask about. Use one you trust, or your own node." }
                        Small { text: "Source"; font.weight: Font.DemiBold; Layout.topMargin: root.sp2 }
                        Option { selected: !root.cfg || root.cfg.eth_source === "rpc"; title: "JSON-RPC from this module"; detail: "Reads go straight to the RPC above."; onChosen: root.call("set_eth_source", ["rpc"], function() { root.loadConfig() }) }
                        Option { selected: root.cfg && root.cfg.eth_source === "logos_module"; title: "Logos Ethereum module (verified proxy)"; badge: "Not wired yet"; badgeTone: "muted"; detail: "The spec's way to reach Ethereum. Its integration is being evaluated; until then this choice reads nothing."; onChosen: root.call("set_eth_source", ["logos_module"], function() { root.loadConfig() }) }
                    }
                    Card {
                        SubHeading { text: "LEZ" }
                        KV { visible: !!(root.cfg && root.cfg.deployment && root.cfg.deployment.lez_rpc); k: "In use"; v: root.cfg && root.cfg.deployment ? Logic.networkLabel(root.cfg.deployment.network) + " (" + root.cfg.deployment.label + ")" : "" }
                        KV { visible: !!(root.cfg && root.cfg.deployment && root.cfg.deployment.lez_rpc); k: "Endpoint"; v: root.cfg && root.cfg.deployment ? (root.cfg.deployment.lez_rpc || "") : ""; monoValue: true }
                        Small { visible: !!(root.cfg && root.cfg.deployment && root.cfg.deployment.lez_rpc); Layout.fillWidth: true; text: "The Overview, the registry and Claim a deposit read this endpoint. It comes with the bridge deployment; it isn't a setting." }
                        Small { text: "Developer: the lez-bridge CLI's sequencer (CLI wallet mode only)"; font.weight: Font.DemiBold; Layout.topMargin: root.sp2 }
                        Field { label: "lez-bridge CLI sequencer address"; text: root.setSeq; onTextChanged: root.setSeq = text; monoFont: true }
                        Small { Layout.fillWidth: true; text: "Used only by the developer CLI flows (private claims and burns through the lez-bridge program), which talk to a sequencer on this computer. The Names Wallet's claims don't use it." }
                        RowLayout { Layout.fillWidth: true; Item { Layout.fillWidth: true } Btn { text: "Save network settings"; primary: true; onClicked: root.saveSettings() } }
                    }
                    Card {
                        SubHeading { text: "Wallet" }
                        Option {
                            selected: root.namesWallet; title: "Names Wallet (recommended)"; badge: "Private"; badgeTone: "ink"; badgeGlyph: "shield"
                            detail: "Your accounts and keys stay in the Names Wallet. It makes this app's deposit secrets, so they come back with your recovery phrase, and you approve every claim and burn in the Names app, which proves it on this computer."
                            onChosen: root.call("set_wallet_mode", ["names"], function() { root.loadConfig(); root.loadWallet(); root.loadAccounts() })
                        }
                        Option {
                            selected: !!(root.cfg && root.cfg.wallet === "cli"); title: "The lez-bridge program's own wallet folder"; badge: "Developers"; badgeTone: "muted"
                            detail: "For test setups: secrets are made on this computer and kept in the bridge's data folder, and sends run as dry runs."
                            onChosen: root.call("set_wallet_mode", ["cli"], function() { root.loadConfig(); root.loadAccounts() })
                        }
                        ColumnLayout {
                            visible: root.namesWallet; Layout.fillWidth: true; spacing: root.sp2; Layout.topMargin: root.sp2
                            readonly property var priv: { var l = root.walletState && root.walletState.accounts ? root.walletState.accounts : []; for (var i = 0; i < l.length; i++) if (l[i].kind === "private") return l[i].account; return "" }
                            KV { k: "Connected"; v: root.walletState ? (root.walletState.problem ? "Can't ask the Names Wallet" : root.walletState.connected ? "Yes" : "No") : "Asking…" }
                            KV { k: "Private account"; v: parent.priv !== "" ? Logic.shortId(parent.priv) : "None given yet" }
                            KV { k: "Bridged tokens"; v: root.walletState && root.walletState.token_source && root.walletState.token_source.accepted ? "Listed in your wallet" : "Waiting for your approval in the Names app" }
                            Small { visible: !!(root.walletState && root.walletState.problem); text: root.walletState && root.walletState.problem ? root.walletState.problem : ""; color: root.pal.error; Layout.fillWidth: true }
                            RowLayout {
                                Layout.fillWidth: true
                                Small { text: "Every request shows in the Names app, and nothing is signed or sent until you approve it there."; Layout.fillWidth: true }
                                Btn { text: root.walletState && root.walletState.connected ? "Connected" : "Connect"; primary: !(root.walletState && root.walletState.connected); enabledState: !(root.walletState && root.walletState.connected); why: "already connected"; onClicked: root.connectWallet() }
                            }
                        }
                    }
                    Card {
                        SubHeading { text: root.namesWallet ? "Bridge program" : "Bridge program and wallet" }
                        Small { text: "lez-bridge program"; font.weight: Font.DemiBold }
                        Field { label: "Path to the lez-bridge program"; text: root.setCli; onTextChanged: root.setCli = text; monoFont: true }
                        Small { visible: root.namesWallet; Layout.fillWidth: true; text: "It reads the chain and builds each claim and burn for your wallet to check; with the Names Wallet it uses a wallet folder of its own that holds nothing, unless you set one below." }
                        Small { text: "Wallet folder"; font.weight: Font.DemiBold }
                        Field { label: "Wallet folder"; text: root.setWallet; onTextChanged: root.setWallet = text; monoFont: true }
                        RowLayout {
                            Layout.fillWidth: true
                            Chip { text: root.cfg && root.cfg.ready.cli ? "Ready" : "Not ready"; tone: root.cfg && root.cfg.ready.cli ? "ok" : "bad" }
                            Small { text: root.cfg && root.cfg.ready.why ? root.cfg.ready.why : ""; Layout.fillWidth: true }
                            Btn { text: "Save"; onClicked: root.saveCli() }
                        }
                    }
                    Card {
                        SubHeading { text: "Deployment" }
                        KV { k: "Name"; v: root.cfg ? root.cfg.deployment.label : "" }
                        KV { k: "Vault"; v: root.cfg ? root.cfg.deployment.vault : ""; monoValue: true }
                        KV { k: "Bridge program"; v: root.cfg ? root.cfg.deployment.bridge_program : ""; monoValue: true }
                        KV { k: "Deployment id (cfgId)"; v: root.cfg ? root.cfg.deployment.cfg_id : ""; monoValue: true }
                        KV { k: "Sends and proofs"; v: root.namesWallet ? "By your Names Wallet, after you approve each one" : root.cfg && root.cfg.proving.enabled ? "On" : "Off in this build: every send runs as a dry run" }
                        KV { k: "Module"; v: root.cfg ? "lez_bridge " + root.cfg.module_version + ", data in " + root.cfg.data_dir : "" }
                    }
                    Card { visible: root.setNotice !== ""; tone: root.setNoticeBad ? "bad" : "ok"; Label { text: root.setNotice; Layout.fillWidth: true } }
                }

                // ── Admin ──
                ColumnLayout {
                    visible: root.section === "admin"; Layout.fillWidth: true; spacing: root.sp4
                    Heading { text: "Admin" }
                    Muted { Layout.fillWidth: true; text: "The controls a deploying entity holds: registry, fees, caps, the relay-fee menu, freezes, the release delay and the fee float. Freezes act per token or for everything, never per person: the bridge does not know whose funds are whose." }
                    StateBox { visible: !!(root.admin === null && root.adminError === ""); kind: "loading"; title: "Reading roles and settings"; message: "" }
                    StateBox { visible: root.adminError !== ""; kind: "error"; title: "Could not read the admin state"; message: root.adminError; action: "Try again"; onActed: root.loadAdmin() }
                    Card {
                        visible: root.admin !== null
                        SubHeading { text: "Roles" }
                        KV { k: "LEZ admin"; v: root.admin ? root.admin.lez.admin + (root.admin.lez.this_wallet_is_admin ? "  (this wallet)" : "") : ""; monoValue: true }
                        KV { k: "LEZ freezer"; v: root.admin ? root.admin.lez.freezer + (root.admin.lez.this_wallet_is_freezer ? "  (this wallet)" : "") : ""; monoValue: true }
                        KV { k: "Ethereum admin"; v: root.admin ? root.admin.ethereum.admin : ""; monoValue: true }
                        KV { k: "Ethereum freezer"; v: root.admin ? root.admin.ethereum.freezer : ""; monoValue: true }
                        KV { k: "Fee recipient"; v: root.admin ? root.admin.ethereum.fee_recipient : ""; monoValue: true }
                    }
                    Card {
                        visible: root.admin !== null
                        SubHeading { text: "Freeze on LEZ" }
                        RowLayout {
                            spacing: root.sp4
                            Check { Layout.fillWidth: false; Layout.preferredWidth: 150; text: "Claims (mint)"; checked: root.admMint; tone: "accent"; onToggled: root.admMint = !root.admMint }
                            Check { Layout.fillWidth: false; Layout.preferredWidth: 150; text: "Burns"; checked: root.admBurn; tone: "accent"; onToggled: root.admBurn = !root.admBurn }
                        }
                        RowLayout {
                            spacing: root.sp2
                            Option { Layout.fillWidth: false; Layout.preferredWidth: 140; title: "All tokens"; selected: root.admToken === ""; onChosen: root.admToken = "" }
                            Repeater { model: root.tokens; delegate: Option { Layout.fillWidth: false; Layout.preferredWidth: 120; title: modelData.symbol; selected: root.admToken === modelData.symbol; onChosen: root.admToken = modelData.symbol } }
                        }
                        RowLayout {
                            spacing: root.sp2
                            Btn { text: "Freeze (dry run)"; enabledState: !!(root.admin && (root.admin.lez.this_wallet_is_freezer || root.admin.lez.this_wallet_is_admin)); why: "this wallet does not hold the freezer key"; onClicked: root.adminDo({ op: "freeze", mint: root.admMint, burn: root.admBurn, token: root.admToken }) }
                            Btn { text: "Unfreeze (dry run)"; enabledState: !!(root.admin && root.admin.lez.this_wallet_is_admin); why: "only the admin unfreezes"; onClicked: root.adminDo({ op: "unfreeze", mint: root.admMint, burn: root.admBurn, token: root.admToken }) }
                        }
                        SubHeading { text: "Fee float"; Layout.topMargin: root.sp2 }
                        Label { Layout.fillWidth: true; text: root.admin ? "Float " + Logic.group(root.admin.lez.float) + " LEZ, sponsor " + Logic.group(root.admin.lez.sponsor_amount) + " per private claim: " + Logic.plural(root.admin.lez.claims_float_covers, "claim", "claims") + " left." : "" }
                        RowLayout {
                            spacing: root.sp2
                            Field { label: "Amount in LEZ units"; placeholder: "amount in LEZ units"; text: root.admAmount; onTextChanged: root.admAmount = text; Layout.preferredWidth: 220; Layout.fillWidth: false }
                            Btn { text: "Set sponsor (dry run)"; enabledState: !!(root.admin && root.admin.lez.this_wallet_is_admin && /^\d+$/.test(root.admAmount)); why: "enter a whole amount"; onClicked: root.adminDo({ op: "set_sponsor", amount: root.admAmount }) }
                        }
                        RowLayout {
                            spacing: root.sp2
                            readonly property string from: { for (var i = 0; i < root.accountsList.length; i++) if (root.accountsList[i].kind === "public") return root.accountsList[i].account; return "" }
                            Btn { text: "Top up the float with that amount (dry run)"; enabledState: !!(parent.from !== "" && /^\d+$/.test(root.admAmount)); why: parent.from === "" ? "this wallet has no public account" : "enter a whole amount"; onClicked: root.adminDo({ op: "fund_float", amount: root.admAmount, from: parent.from }) }
                            Small { text: parent.from !== "" ? "from " + Logic.shortId(parent.from) + " (public)" : "" }
                        }
                        Small { visible: root.admNotice !== ""; text: root.admNotice; color: root.pal.text; Layout.fillWidth: true }
                    }
                    Card {
                        visible: root.admin !== null
                        SubHeading { text: "On LEZ" }
                        Small { Layout.fillWidth: true; text: "Through the lez-bridge program with this wallet's keys. In this build each runs as a dry run." }
                        Repeater {
                            model: root.admin ? root.admin.operations.filter(function(o) { return o.side === "lez" }) : []
                            delegate: RowLayout {
                                Layout.fillWidth: true; spacing: root.sp3
                                Label { text: modelData.title; Layout.fillWidth: true }
                                Chip { text: !modelData.available ? "Needs a key this wallet lacks" : modelData.op === "register_token" ? "Module API only (no form yet)" : "Available"; tone: modelData.available && modelData.op !== "register_token" ? "ok" : "muted" }
                            }
                        }
                    }
                    Card {
                        visible: root.admin !== null
                        RowLayout {
                            Layout.fillWidth: true
                            SubHeading { text: "On Ethereum: the vault's settings"; Layout.fillWidth: true }
                            Chip { text: "Read-only in this build"; tone: "muted" }
                        }
                        Small { Layout.fillWidth: true; text: "Editing needs Ethereum transactions from the admin key, which go through the Logos Ethereum module. It is not wired into this build yet, so these values are shown, not edited." }
                        Repeater {
                            model: root.admin ? root.admin.tokens : []
                            delegate: ColumnLayout {
                                Layout.fillWidth: true; spacing: 4; Layout.topMargin: root.sp2
                                Label { text: modelData.symbol; font.weight: Font.Bold }
                                KV { k: "Registered"; v: modelData.registered ? "Yes, in " + modelData.denoms.map(function(d) { return d.text }).join(", ") : "No (delisted)" }
                                KV { k: "Fees (deposit / withdraw)"; v: modelData.denoms.every(function(d) { return d.deposit_fee_text === "none" && d.withdraw_fee_text === "none" }) ? "None on any amount" : modelData.denoms.map(function(d) { return d.text + ": " + d.deposit_fee_text + " / " + d.withdraw_fee_text }).join("; ") }
                                KV { k: "Caps per " + Logic.durationText(modelData.caps.window_s); v: "deposits " + modelData.caps.deposit.cap_text + ", releases " + modelData.caps.release.cap_text + " " + modelData.symbol }
                                KV { k: "Relay-fee menu"; v: modelData.denoms.map(function(d) { return d.text + ": " + d.relay_fee_menu.map(function(m) { return m.pct }).join(", ") }).join("; ") }
                                KV { k: "Release delay"; v: Logic.durationText(modelData.release_delay_s) }
                                KV { k: "Freeze"; v: "deposits " + Logic.freezeWord(modelData.freeze.eth_deposits).toLowerCase() + ", releases " + Logic.freezeWord(modelData.freeze.eth_releases).toLowerCase() }
                            }
                        }
                        RowLayout {
                            spacing: root.sp2; Layout.topMargin: root.sp2
                            Repeater {
                                model: ["Edit registry", "Edit fees", "Edit caps", "Edit relay menu", "Edit delay", "Freeze"]
                                delegate: Btn { text: modelData; enabledState: false; why: "Ethereum transactions come with the Logos Ethereum module" }
                            }
                        }
                    }
                    Card {
                        visible: root.admin !== null
                        SubHeading { text: "Attestation" }
                        KV { k: "Instance"; v: root.admin ? root.admin.attestation.instance : ""; monoValue: true }
                        KV { k: "Latest anchor"; v: root.admin ? "block " + root.admin.attestation.anchor_block + ", " + root.admin.attestation.anchor_age_text + " ago" : "" }
                        KV { k: "Updates so far"; v: root.admin ? String(root.admin.attestation.updates) : "" }
                        KV { k: "Updater"; v: root.admin ? root.admin.updater.reason : "" }
                    }
                }
            }
        }
    }
}
