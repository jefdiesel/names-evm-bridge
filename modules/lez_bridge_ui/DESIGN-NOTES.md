# LEZ Bridge mini-app: design notes

## Direction

The brand, as on the live site: cream paper, near-black ink and ONE accent, orange. The mark is
a white η on an orange rounded tile. The frame is the Names app's (left sidebar, one centred
column at most 760 px wide, cards with a 1 px line and 8 px radius, Public Sans), so the app sits
at home in Basecamp.

Two things are the bridge's own:
- **The privacy indicator** at the top of every flow, in the same place, saying in words what
  makes a choice less private and what would make it private.
- **Jobs as first-class objects**: a claim proves for about 11 minutes, so it gets a phase
  stepper, a clock and its own screen, not a spinner.

The boldness is spent in one place, the orange: the current step, the one primary action, the
selection and the mark. Everything else is ink on paper.

**No status hues.** There is no green and no blue anywhere. State is carried by ink weight,
fills of ink or dim, shape, glyphs and words:
- **Done:** a solid ink circle or bar, with a check.
- **Current:** orange.
- **Pending:** an outline in the line colour.
- **Skipped (dry run):** an outlined, empty bar, and the word "skipped".
- **Facts the reader should know** (demo vault, dry-run builds, "not built yet"): quiet notes on
  inset paper, with no hue.

## Palette

| Role | Light | Dark | Use |
|---|---|---|---|
| paper | #f4f2ec | #11110f | page |
| card | #fffefb | #201f1b | cards, selected sidebar row |
| sidebar | #ece9e1 | #181815 | the frame |
| ink | #1b1a17 | #edeae2 | text; done steps; solid tags; the "live" dot |
| dim | #6d6960 | #a29d91 | secondary text; utilisation bars below 70% |
| line | #d6d1c4 | #3a3832 | borders; pending steps |
| track | #e7e3d9 | #2e2d28 | empty bars |
| orange (accent) | #e8662f | #f0794a | primary button, current step, selection, focus ring, the mark |
| orange as text | #b4481c | #f0794a | small links on paper ("What is public?") |
| ochre | #7f5d0c on #f2e7c9 | #d9b45f on #2c2617 | **only** "Less private", and bars at 70–90% of a cap |
| red | #a8322a on #f6e3df | #e8806f on #2f1c18 | errors, "Not private", "Links you", caps at 90% or more |

**Contrast** (WCAG 2 ratios, computed from the hex values):
- Ink on paper: 15.5:1 light, 15.7:1 dark.
- Dim on card: 5.4:1 light, 6.1:1 dark.
- Tertiary text on card: 4.9:1 light, 4.9:1 dark.
- Orange as text on card: 5.4:1 light, 5.9:1 dark.
- Ink on the orange button: 5.3:1 light, 6.8:1 dark.
- Ochre on its fill: 4.9:1 light, 7.6:1 dark.
- Red on its fill: 5.4:1 light, 6.0:1 dark.
- The mark's white η on orange is 3.3:1, which passes for its large glyph only.

The ochre is a yellow-brown and the accent a red-orange, so they do not read as one colour; the
caution state also has its own glyph (half shield) and word.

## Basecamp tokens: what is used and what is avoided

- **Used from the host:**
  - light or dark (`Theme.isDark`);
  - the typeface (`Theme.typography.publicSans`) and its size scale, which follows the user's
    text-size setting;
  - the monospace family, for hashes only.
- **Avoided on purpose:**
  - `success`, `successHover`, `successPressed`, `successBorder`: green; state is ink here.
  - `info`: blue; notes are neutral.
  - `warning`, `warningHover`, `warningBorder`: a bright yellow that competes with the accent.
    The bridge's ochre is used only for "Less private".
  - `primary` and its states: Basecamp's light-theme orange is #C8460F. The brand's is #e8662f,
    used for fills, with #b4481c for orange text.
  - `backgroundInset`, `backgroundTertiary`, `borderSecondary`: not every Basecamp 0.3 theme has
    them.
- **Brand values for the neutrals instead:** paper, card, ink, dim and line sit within a few
  steps of Basecamp's own light theme (#FBFAF7 / #1B1A17), so the app still reads as part of it.
- **The render harness** sets `forcedMode` (0 light, 1 dark), never `Theme.setTheme`, so the
  theme file every Logos process reads is untouched.

## Type scale

From `Theme.typography`, so the user's text-size setting scales everything:

| Use | Size |
|---|---|
| Screen title | `panelTitleText` (24), bold |
| Section title | `subtitleText` (16), demi-bold |
| Body | `primaryText` (14), line height 1.25 |
| Captions, table heads, hints | `secondaryText` (12), `textSecondary` |
| Hashes, addresses, calldata, logs | the platform monospace at 12, wrapped anywhere |

## Spacing

The scale is 4 / 8 / 12 / 16 / 24 / 40:
- 16 inside cards;
- 16 between cards;
- 8 between rows in a card;
- 24 page margin.

Sidebar 232 px. Controls are 34 to 38 px tall.

## The privacy indicator (`PrivacyRail`)

**Closed:** one line. A shield glyph, the level word, a sentence that names the cause, and "What
is public?".

| Level | Word | Glyph | Look | Sentence |
|---|---|---|---|---|
| `private` | Private | filled shield | card, 1.5 px ink border, ink glyph | "Nothing published links the two sides. Amount, token and timing are visible." |
| `reduced` | Less private | half shield | ochre fill and border | "Because you claim before the suggested time. To make it private, choose Wait." |
| `exposed` | Not private | open shield with a cross | red fill and border | "Because you mint to your own account by name. To make it private, choose the one-time address." |

The sentence is built from the module's `weakened_by` list (`logic.mjs` `CAUSES`). So it always
names the actual choice, and the action that undoes it, in the words of the option the person
can click.

**Open** (click, Enter or Space), it lists each chosen override with what it leaks and to whom,
what is always public, what stays on this computer, and notes such as a small anonymity set.

**Behaviour:**
- Every private choice is pre-selected, including timing. A claim before the suggested time is
  an explicit option ("Claim now", tagged "Less private"). With "Wait" selected, the main button
  reads "Claim after <time>" and is disabled.
- A weakening choice recolours the rail at once and shows a tick right above the main action:
  "I accept that claiming before the suggested time makes this claim easier to link to me." The
  module refuses the call without `confirm_weaker` anyway.
- Without colour, the level is still carried by the glyph's shape and the word.

## States, per screen

| Screen | Loading | Empty | Error |
|---|---|---|---|
| Overview | "Reading the vault on Ethereum and the bridge on LEZ" | "Nothing waits on you" + where to start | Error card + Try again; a **partial** banner lists which side failed and still shows the other |
| Registry | same, with "Both chains are asked" | n/a | Error card with the RPC/CLI message + Try again |
| Deposit | tokens loading card | n/a (needs tokens) | Error note under the form; deposits frozen shows a blocking card |
| Deposits & claims | "Matching this computer's deposits with the vault's events" | "No deposits on this computer yet" + New deposit | Error card + Try again; a short log read is an error, never "no deposits" |
| Claim | "Checking the claim … nothing is proven or sent" | n/a | Error note with title by code, local diagnosis, and the note that on chain it only ever reads "claim refused" |
| Jobs / proving | n/a | "No jobs yet" | Error note with code; "Outcome unknown" after a restart says to check holdings first |
| Holdings | "The wallet catches up with the chain first" | "No wrapped tokens in this wallet" | Error card + Try again; stale data is labelled with its age |
| Redeem | "Planning…" on the button | n/a | Error note (checksum, zero address, vault address, not enough held) |
| Exits | n/a | "No exits on this computer" + Redeem | n/a |
| Recovery | n/a | "0 positions found" | Error note |
| Settings | n/a | n/a | Notice card: saved / RPC did not answer / other chain |
| Admin | "Reading roles and settings" | n/a | Error card + Try again |
| Claim a deposit | "Checking the claim … Nothing is signed or sent"; "Asking your Names Wallet" | the claim-file card (Choose file… or paste) | a red card titled by code (`logic.mjs` `relayErrorTitle`: not final on Sepolia yet, not recorded on the bridge test network yet, already claimed, relay limit reached, relay refused and why, private account, another vault or chain, damaged file) with the module's plain sentence, and "Nothing was signed or sent" |

## Claim a deposit (a claim file into a public account; the relay pays)

One screen, `claimfile` in the sidebar ("Claim a deposit") and a button on Deposits and claims:

1. **The claim file.** "Choose file…" (QtQuick.Dialogs, created at run time; where Basecamp has no
   file picker the screen says so and the paste box takes the file's text or its full path) or paste.
   The pasted text holds the claim key: it is cleared from the screen as soon as it is sent to the
   module, which keeps it in memory only.
2. **Checked.** What the claim does in plain rows: amount, into (your public account), from (the
   Sepolia leaf, final), who signs (your Names Wallet; it sends nothing and pays nothing), who sends
   and pays (the bridge's relay, at most N LEZ of its own). Then the module's checks as solid ink
   check-circles, then the privacy cost in the "Not private" look (open shield, red) with the
   wallet's own line: "A public claim publishes the claim secret and links the Sepolia deposit to
   this account." Then "Send to my Names Wallet".
3. **On its way.** A three-step stepper (Approve, Relay, Confirm), the module's note (waiting for you
   in the Names app; the relay is busy and your wallet signs the same claim again; sent, waiting for
   a block), a specific error card, or the result: block, transaction, the wrapped balance now in the
   account, native LEZ 0, and that the Names Wallet's LEZ 0.4 view shows it.

The network chip always reads the deployment's label ("LEZ 0.4 · bridge test network"; never
"official"), and every state repeats the agreed sentence: "The wallet signs the claim; the bridge's
relay sends it and pays the fee." No green: done is ink, current is orange.

## Keyboard and accessibility

- **Focus and activation:**
  - Every control takes focus with Tab, in reading order.
  - Enter and Space activate buttons, options, checkboxes, the sidebar items and the privacy
    rail.
  - Focus shows as a 2 px ring in the theme's `focus` colour.
- **Accessible names and roles:**
  - Buttons say what they do. Disabled ones add "Unavailable: <reason>".
  - Options are radio buttons with `checked`. Checkboxes are checkboxes.
  - Sidebar items are page tabs with `selected`.
  - Usage bars are progress bars named "label: used of cap (n%)".
  - Error notes are alert messages.
  - The QR code is a graphic named for what it encodes.
- **Text over shapes:**
  - The phase stepper names each phase as done, in progress, skipped (dry run) or not reached.
  - The clock is named "Elapsed …".
- **Wrapping:** long hashes wrap anywhere instead of being cut.

## Fixtures and renders

- **Fixtures:**
  - `modules/fixtures/v2-demo/*.json` are replies of the real module code against the real v2
    demo (`tools/record-fixtures.sh`): read-only calls and dry runs on a wallet copy, and
    read-only Sepolia RPC.
  - The one exception is `job__proving.json`, a proof in progress. This build never proves, so
    `tools/make-proving-fixture.py` takes its timing and memory from the real proved claim of LEZ
    block 4998 (`fixtures/source/`).
  - The relayed claim's replies (`relayed_claim*.json`) are hand-made in `relayed.rs`'s shapes;
    `tests_relayed.rs` (`the_render_fixtures_have_the_module_s_shapes`) checks their keys against
    the module's real replies. They drive the `claimfile*` renders.
  - The Names Wallet mode's replies (`*__names.json`, `wallet_*.json`, `job__wallet*.json`,
    `make_deposit_request__needs.json`) are hand-made in the module's shapes (`names_mode.rs`,
    `wallet.rs`, as `tests_names.rs` checks them), each marked with `_provenance`: no Names Wallet
    ran for them. They drive the `*-names`, `deposit-needs` and `proving-wallet*` renders.
- **Renders:** `tools/render.py` renders every screen in both themes from those fixtures into
  `modules/renders/` (not published).

## Known limits

- One QML file with inline components, as the Names app does, so packaging stays a single view.
- Every piece of a deposit plan can become a request. The schedule is advice: nothing sends later
  pieces automatically.
- The QR carries an EIP-681 link. Wallet support for `bytes32` parameters varies, so the steps are
  also shown as raw call data to paste.
