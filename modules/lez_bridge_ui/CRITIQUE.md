# Critique of the LEZ Bridge mini-app, round 1

Scope: all 40 renders in `modules/renders/` (not published), taken before this pass (20 screens, light and dark), and
`qml/Main.qml`. Sources: `DESIGN.md` §3 and §8, the RFP-022 Usability and
Privacy text, the module's own replies (`fixtures/v2-demo/`), and the brand, which is the live
site's values.

**Status of the artefact.** This is a mini-app for a bridge whose outbound half (G2) is not built,
shown against a demo vault and a local sequencer. The app does say so (Overview notes, Exits,
Redeem). Findings are ranked by the damage they would do if a user or reviewer saw the screen.

## Ranked findings

```
HIGH  Claim screen, privacy indicator
  claim   "Less private  The choices below make you easier to match by timing or pattern."
          shown while "A one-time address (recommended)" is selected
  source  claim_plan: claim_early = true. The report is weakened only by timing (claiming
          before the suggested time). The user made no choice; the app chose "claim now" silently.
  why     The recommended option is selected and the banner still says "Less private". It
          blames "the choices below", but no choice below causes it. A reviewer concludes that
          the indicator is noise, or that the recommended path is not private.
  fix     Make timing an explicit choice. "Wait until 14 Nov 02:28 UTC (recommended)" is
          pre-selected; "Claim now" must be chosen. The indicator names the cause: "Less private
          because you claim before 14 Nov 02:28 UTC. Choose Wait to make it private."
```

```
HIGH  Privacy indicator headline (every flow)
  claim   "Private  Only the amount, the token and the timing are public."
  source  DESIGN §8.2 lists more that a private claim publishes: the one-time address, the
          anchor, instance, vault and cfg_id, the nullifier, the fixed sponsor payment and the
          fee. The rail's own "Always public" list repeats them.
  why     The headline contradicts the list under it. A hostile reader quotes the headline next
          to the list.
  fix     "Private: nothing published links your deposit to your LEZ account. Amount, token and
          timing are visible."
```

```
HIGH  Claim screen, proving card
  claim   "It needs about 9.5 GB of memory for 11 minutes: 16 GB of RAM is the minimum"
  source  DESIGN P4: measured on ONE machine (Apple M4 (16 GB)). "laptop and
          minimum spec NO TEST".
  why     A minimum hardware spec stated as fact rests on one measurement.
  fix     "Measured on a 16 GB Apple M4 desktop: about 11 minutes, 9.4 GB peak. Smaller computers are
          untested; close other large apps first."
```

```
HIGH  Deposit screen lead
  claim   "The bridge moves fixed amounts only, so an amount can't identify you."
  source  The registry shows 3 deposits of 100 dUSDC, 1 of 1,000 and 0 of 10,000. DESIGN §3.1:
          the anonymity set is the earlier deposits of the same amount. With 1 deposit the
          amount identifies you completely.
  why     An absolute contradicted by the numbers on the same screen.
  fix     "The bridge moves fixed amounts only, so the amount hides you among everyone who moved
          the same amount. Larger amounts are split into pieces, best sent hours apart."
```

```
MEDIUM  Sidebar "Live" with a green dot; renders made from recorded replies
  claim   "Live" (green)
  source  The flag only means "not in fixture mode". No health check stands behind it. The
          renders show recorded replies and still say "Live".
  why     It reads as "connected and healthy" when nothing was checked, and it is false in the
          renders.
  fix     A small ink dot with "Reading the chains" when live; a hollow dot with "Recorded data"
          in fixture mode or a render.
```

```
MEDIUM  Overview, attestation tile
  claim   "11 h since the latest anchor", green dot
  source  registry: anchor 3,010 blocks behind finality, 0 updates, no updater. Deposits newer
          than the anchor cannot be claimed until someone proves an update (about 2 h of proving).
  why     Green says "fine" about the one stalled component that blocks new claims.
  fix     No status colour. Say it: "Covers deposits up to Ethereum block 12,123,027. Newer
          deposits wait for the next update; nobody runs updates automatically yet."
```

```
MEDIUM  Error notes (claim refused, already claimed)
  claim   "Already claimed / deposit already claimed: its nullifier is spent; nothing sent"
  source  The message is the CLI's text. "nullifier" is protocol vocabulary (Usability #8 asks
          for clear, actionable errors).
  why     It is not actionable, and its jargon makes the title look duplicated.
  fix     Plain advice per code ("This deposit was claimed already. Nothing was sent.") with the
          CLI's text kept as small details.
```

```
MEDIUM  Colour: green, blue, amber, red and orange used as meaning
  where   progress fills, "Private" chips, badges, "Live", "Dry run passed", info boxes (blue),
          stage chips (amber for "Claimable, wait advised")
  source  The brand: paper, ink and ONE accent (orange). The user rejected the green outright.
  why     Five hues compete. Amber on a normal state ("Claimable, wait advised") reads as a
          fault. Blue information boxes are a sixth voice.
  fix     Ink and dim for state, with shape, icon and word. Orange for the current step and the
          one primary action. Ochre only for "Less private". A restrained red only for errors
          and "Not private".
```

```
MEDIUM  Template chrome
  where   sidebar "MOVE IN / MOVE OUT / MORE" (all-caps tracked labels); meta strings joined with
          " · " (plan rows, tiles, stage lines); the "⇄" mark
  why     These read as generated defaults, not as Logos.
  fix     Group the sidebar by spacing and a hairline. Write meta as short sentences or separate
          columns. Mark: white η on an orange rounded tile.
```

```
MEDIUM  Deposit plan: only piece 1 can become a request
  claim   pieces 2 and 3 show a disabled "Create request" ("send the pieces one at a time")
  why     A person who wants all three requests (to send later, on schedule) cannot make them.
          The disabled reason misdescribes the limit.
  fix     Every piece can become a request at any time. Each row shows its own request state;
          the suggested send time stays as advice.
```

```
MEDIUM  Deposit request: call data walls
  where   Step 1/2 "Data" shows 200+ hex characters in one block
  why     It is unreadable and uncheckable. It pushes the QR and "Copy" below the fold.
  fix     The QR and "Copy request" come first. Call data folds under "Show call data", split
          into the function selector and 32-byte words, one per line.
```

```
LOW  Deposits timeline connectors
  where   the vertical lines between steps are short stubs; the circles float
  fix     The connector spans the whole row to the next step: solid ink where done, line colour
          where pending.
```

```
LOW  Overview "Needs you"
  claim   "Review" on three rows that say the same sentence
  fix     One row per deposit with its amount and state in columns, and a button named for the
          action ("Plan claim").
```

```
LOW  Proving view, dry-run job
  where   "Memory (resident)" and an empty clock "0:00" for a 0.3 s dry run
  fix     Hide the memory row unless a proof ran. Show "Simulated in 147.8 ms" instead of a clock.
```

**Clean:**
- Exits & release: the not-built statement is accurate (DESIGN §0, §4).
- The Redeem "blocked" text matches `vault/DEPLOYMENTS.md` (placeholder update image).
- Registry numbers match `DEPLOYMENTS.md`: caps 50,000 / 5, menus 0.5% and 1%, 24 h delay.

**The single change that most reduces risk:** make timing an explicit, pre-selected "Wait" choice
on the Claim screen, and have the privacy indicator name its cause in words.

## What changed (round 2, renders of 2026-11-13 afternoon)

| Finding | Change |
|---|---|
| "Less private" while the recommended option is selected | Timing is now an explicit choice: "Wait until … (recommended)" is pre-selected and "Claim now" is tagged "Less private". With Wait selected, the indicator reads Private and the main button reads "Claim after …", disabled. The indicator's sentence names its cause and the undo, from the module's `weakened_by`: "Because you claim before the suggested time. To make it private, choose Wait." |
| Headline overclaim | "Nothing published links the two sides. Amount, token and timing are visible." Changed in the module and the app |
| 16 GB "minimum" | "Measured on a 16 GB Apple M4 desktop … Smaller computers are untested." Changed in the module and the app |
| "an amount can't identify you" | "the amount hides you among everyone who moved the same amount" |
| "Live" (green) | A solid ink dot with "Reading the chains"; a hollow dot with "Recorded data" in fixture mode, which the render harness now reports |
| Attestation tile in green | No status colour. "Covers deposits up to block 12,123,027. Newer ones wait for the next update; nobody runs updates automatically yet." |
| Jargon in errors | Plain advice per code, with the CLI's text as "Details" |
| Five hues | Ink, dim and the orange accent only. Ochre only for "Less private", red only for errors and "Not private". A pixel scan of all 40 renders finds no green or blue |
| Template chrome | All-caps sidebar labels replaced by hairlines. Meta strings rewritten as sentences or columns. The mark is the brand's white η on an orange tile; the app icon is regenerated |
| Only piece 1 of a plan | Every piece can become a request. Each row shows "Make request", then a check and "Show request" |
| Call data walls | QR, the steps in words, and "Copy request" come first. Call data folds under "Show call data", split into the selector and 32-byte words |
| Short timeline connectors | The connector runs the full row: ink between done steps, line colour elsewhere. Done is a solid ink check; the next step is orange |
| "Review" ×3 on Overview | "Your deposits" is a table (amount, state, next step) with "Plan claim" |
| Dry-run job clock "0:00" | Hidden for a finished dry run. Its later phases read "skipped" with outlined bars |

**Left as is:**
- The Registry "Claimed" column counts LEZ claims for everyone, not yours; the column head says
  "Claimed".
- The job log tail stays monospace.
