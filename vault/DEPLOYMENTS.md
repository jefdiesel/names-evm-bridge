# Deployments

## DepositDesk in front of v2: the demo bridge fee

**Deployed on Sepolia on 2026-11-13** at `0x77E37E1D6e886d8fb988496B9c2EfF3E78e27DF2` (block 12128491; record at the end of this file). Three deposits have gone through it (`FeePaid` logs, read 2026-11-14 afternoon): two at 300 bps, one of them the team's leaf 6 (block 12129802, `DESIGN.md` §3.6), and the team's leaf 8 at 30 bps (block 12134587, unclaimed). `src/desk/DepositDesk.sol` (`DESIGN.md` §3.6): one deposit transaction that adds a bridge fee in the deposited asset, **now 0.3% (30 bps)**, owner-set, hard cap 3% (300 bps, a constant) and forwards the deposit to the live **v2** vault `0x8C8b2664De28e5287158db4DB65D448638871b74` in the same call. Owner: the throwaway deployer `0x37a46E7334Dd061E0C7E2298247EE9278d0d321F`. The v2 vault is not changed or redeployed.

Deploy (the owner runs it; the key comes from `SEPOLIA_PK` like the other Sepolia scripts):

```sh
cd vault
node ../scripts/check-versions.mjs --onchain --deployment <the vault's deployment>   # first; must pass (VERSIONS.toml)
SEPOLIA_PK=<deployer key> forge script script/SepoliaDesk.s.sol --rpc-url https://ethereum-sepolia-rpc.publicnode.com          # simulate; prints DESK=
SEPOLIA_PK=<deployer key> EXPECTED_DESK=<DESK from the simulation> forge script script/SepoliaDesk.s.sol \
  --rpc-url https://ethereum-sepolia-rpc.publicnode.com --broadcast --slow --gas-estimate-multiplier 1000 \
  --with-gas-price <cap> --priority-gas-price <tip>
```

`DESK_FEE_BPS` (default 300, at most 300) sets the starting rate. The script refuses a key that is not the deployer and a vault whose `cfgId` is not v2's. After it: the desk address goes into `web/deposit/src/lib/deployments.js` (`desk`, the page's one switch) and, with its creation block, into the grants daemon's `DESK` and `DESK_FROM_BLOCK` (`public-demo/RUNBOOK.md` (not published) §12).

## Sepolia vaults v6 and v7 (2026-11-18): test vaults deployed from branches

Their code and full records are on branches beside `main`; this section lists the addresses only. v5 stays the default on the deposit page.

- **v7** (branch `v7-seqsig`, record `public-demo/prove/v7-seqsig-20261118/README.md`): v6's inbound side; withdrawals on 2 of 3 signer keys of ours through a gate anyone can submit to, with a 600 s contest window (a test-only setting).
  - Vault `0x4D7B360cf385d3477F1A67c1AEdEB6C17c26d23E` (block 12159635), gate `0x5E891d52eB9a729feCFDd9051685FDeaCd9F0FaE` (block 12159634), desk `0x9b4040A990aEe283F1DE5451fFBaEE3679507fb5` (block 12159647, 30 bps).
  - `cfgId` `0x7ef9153b69a348ff39e3daf7b893f1b775513489c71c7331d88b4a5cf30652d5`; update image `0x93c88f77e60ee5698849163ec51d6b8647e58df752984dcb15ca2f011c96955d`; release image as v5.
  - Runs (2026-11-18): one round trip (update finalized in block 12159962, release paid in 12159991); then an equivocation contest: a second update over LEZ 24721..26120 submitted in block 12162506 (tx `0x47f4cb216fd2e3939a3a916c99f5c5d43b1fee1c938ce9a1186878bc10e53eff`) and contested in 12162508 (tx `0x0291308ed09e18de620482673237e06d70ecc9d90fce682e90ecfd2500e91b84`) with a real evidence proof that signer S1 had also signed another hash for block 26120 (our own key, on purpose, with a test flag). The gate ejected S1 (`ejected()` 1, `activeSigners()` 2, not halted) and cancelled the update; the vault's `g2State()` is unchanged (`0x319f231d…f936`, one update applied). Every later update needs the two remaining signers.
  - Claim relay since 2026-11-18 about 20:25 UTC: payer `6kz6Lj4BLQgA55qVF93rcikSu5TVsDka2iHidMsV9Rak`, public burns relayed too. The v7 vault was added to the settlement service's list on 2026-11-18.
- **v6** (branch `v6-lc`, record `public-demo/prove/v6-lc-20261118/README.md`): v5's design with deposit roots from the RFP-021 light client instead of the operator attester.
  - Vault `0x95cea20e75d3B352B49d13cb0cDdC268a14C427e` (block 12158041), gate `0x4aFB8eeCd80B95e79D9280D462D13dA14f9aAC1B` (block 12158040), desk `0x8Aab670f14A53A41f20a6aA75137b13C0234A1c3` (block 12158055, 30 bps).
  - `cfgId` `0x7d1a304a12a486c730d94321244bd6d15741208c1ef9d00e352acda8589e6f60`; update and release images as v5.
  - Claim relay since 2026-11-18 16:49 UTC: payer `59wbZ9ftUCQ9bernX7PfdpNu2niV7U8tfxeFfvN9ahTZ`, for the bridge `4PxaLAfPZDvpfZaR8SsC2Y7pYMPNwPPGVGMpjEVPQWed`. Its first relayed claim, made from the Names Wallet, is in LEZ block 27401 (the vault's third deposit, Sepolia block 12162864).
- Verifier for both: RISC Zero's Groth16 verifier 3.1.0, `0x8c915b5071206C6E328BC18529586c513408aB3b`. All six contracts are source-verified on Sourcify (exact match).

## Sepolia vault v5 (2026-11-15): v4's design with no release delay

**Deployed on Sepolia on 2026-11-15** from `public-demo/V5-PLAN.md`. The record is
`public-demo/RUNBOOK.md` (not published) §21. **The default for the deposit page and the bridge app** since branch
`v5-live`; v4, v3 and v2 stay listed.
- **What it is:** v4's design. RISC Zero's bare `RiscZeroGroth16Verifier` 3.1.0 sits behind a new
  `UpdateGatedVerifier` instance with the SAME submitter key; the same two images; the v3 canonical
  bridge program at a new header, `CuV1ctZ2qJG4wQcXP4TBudVGKzNmdoYT3DpP1veHnDMA`; the same token
  program with new wrapped definitions.
- **What differs:** **`minReleaseDelay` 0 and each token's delay 0** (the owner's decision for this
  test network). An exit is releasable at the timestamp of the update that recorded it.
- **What that costs:** only the caps bound an update over a forged LEZ tip (a stolen submitter key);
  `public-demo/V5-PLAN.md` §1.
- **S0 was not repeated:** the same images on the same verifier, accepted for v4.
- **At deployment, no deposit, update or release** (`depositCount()` 0, `g2State()` = the genesis state, read 2026-11-15
  and again early on 2026-11-16). Since 2026-11-16: a private round trip (update accepted in Sepolia block 12149823,
  release paid in 12149867) and updates from the unattended updater (`docs/EVIDENCE.md`, rows `v5-private-round-trip`,
  `v5-unattended`; `public-demo/RUNBOOK.md` (not published) §22, §23).

| What | Value |
|---|---|
| RISC Zero verifier | `0x8c915b5071206C6E328BC18529586c513408aB3b` (3.1.0, selector `0xe71e6620`), as v4 |
| `UpdateGatedVerifier` (the vault's `verifier()`) | `0x0F3F80D37B1616C0E9B182D3A87FE4f6220715d1`, block 12143639, tx `0x8152f34e5cc411d34d2a891d2b9cc7ab8f65cb56f459b9200f9f0e1c38cab37c`, 2,954,104 gas |
| `SUBMITTER()` | `0xB81aa37D80F8a9588ce9B09B6d6145D1FC4095f4` (the same key as v4) |
| `BridgeVault` | `0xAFbDF192434E9888Bb23DAE6f2D15Eea6246eD10`, block 12143640, tx `0xc1eaa2ddbd495018176656cbe63cd6f4c1d35965cfacd100daa85e5d203efe21`, 33,047,965 gas |
| `cfgId()` | `0xc513c0f6a949cd81f7e834d0bcf771fd1e89f7430fa1b713c5aad71e1eb4f7d1` (= the v5 bridge Config's) |
| `programId()` | `0xb0e26e1628be4dd9d35319baf2075ffa260c13af3c43abd8ebd1b34d0a7dd3a9` (bridge `CuV1ctZ2…nMA`) |
| `lezChainId()` / `zoneId()` | as v3 and v4 |
| `updateImageId()` / `releaseImageId()` | `0x098f5a1a…3017` / `0x6bcbd714…72a4`, as v4 |
| `g2State()` | `0xa339efab8cc3c73264e144d6b07b093a2258334dedf1ec00cb65095270954051` (the genesis state at LEZ block 11598; `Initialize` in 11617) |
| `minReleaseDelay()` | 0; the ETH token's delay 0 (read back) |
| Admin, freezer, fee recipient | `0x37a46E7334Dd061E0C7E2298247EE9278d0d321F` (the throwaway deployer) |
| Tokens | as v4: ETH 0.01/0.1/1, dUSDC 100/1,000/10,000; zero vault fees; 0.5%/1% relay-fee menus; caps as v3 |
| `configure()` | blocks 12143642–12143650, 9 transactions, 6,972,192 gas: `registerToken` ×2 (`0xe0eb1a32…f758`, `0x5dd70708…239f`), `setRelayFeeMenu` ×2 (`0x48f37d7a…7b47`, `0x92e2f67d…dc1f`), `setCaps` ×2 (`0xd1b204a1…bb70`, `0x1df5318b…2c81`), `setGlobalCaps` (`0x60b02c97…4ad2`), `setCapWeight` ×2 (`0x9c1385d4…49f3`, `0xc7e23e62…3d77`) |
| DepositDesk v5 | `0xbAf1F99145D4F09c731273fc78709785dD959880`, block 12143651, tx `0x24cbd5e98d40d835095ae62cbdad8e924010e2929202f99bf8992d5bc4226301`, 6,516,024 gas; `vault()` = the v5 vault, `feeBps()` 30 |

- **Read back on 2026-11-15 through Tenderly:** the vault's `g2State`, `cfgId`, `programId`,
  `verifier()`, both image ids and `minReleaseDelay`; the ETH `tokenConfig` (delay 0); the gate's
  `VERIFIER()`, `SUBMITTER()` and `UPDATE_IMAGE_ID()`; the desk's `vault` and `feeBps`.
- `node scripts/check-versions.mjs --onchain --deployment v5` passes.
- **Hashes and gas** are from the forge broadcast records (not in git). Every transaction paid about
  10,000,015 wei per gas: 49,490,285 gas, 0.000495 Sepolia ETH in all.
- **Script:** `script/SepoliaV4.s.sol` with `MIN_RELEASE_DELAY_SECONDS=0 RELEASE_DELAY_SECONDS=0`
  (`run()`, then `configure()`); the desk with `script/SepoliaDesk.s.sol`.

## Sepolia vault v4 (2026-11-15): RISC Zero's Groth16 verifier 3.1.0, behind an update gate

**Deployed on Sepolia on 2026-11-15** from `public-demo/V4-PLAN.md`; the record
is `public-demo/RUNBOOK.md` (not published) §18. **The default for the deposit page and the bridge app from branch `v4-live` until v5** (branch `v5-live`);
listed since, as an earlier vault: withdrawals work, with a one-hour wait. Paired with a new bridge instance on the team's public LEZ rc3
demo chain: the v3 bridge program (image `0dabbd6c…9ffa`) deployed again at a new header,
`BKXe3q5JcKeiSrKv9FUBjZw3AowbvYgyHZ3SYPgDxJBf`.

- **Its proofs are checked on Ethereum.** The verifier is RISC Zero's bare `RiscZeroGroth16Verifier` 3.1.0,
  not an operator key. Before the broadcast, the stop step (V4-PLAN S0) passed: one real Groth16 seal of
  each pinned image was accepted by `verify` on that verifier, wrapped in 163 s each on a GitHub runner.
- **Updates come only through the gate's `submitUpdate`, from one fixed submitter.** Releases are
  permissionless. Which LEZ blocks count as final is the submitter's word: the pinned update image is
  TrustedTip, and LEZ rc3 headers are unsigned (`src/UpdateGatedVerifier.sol`).
- **The genesis state is not zero:** it is `outbound_host genesis` at LEZ block 10265, right after the
  bridge's deploy and before its `Initialize` in block 10286. The first update starts from it.
- **Not trustless yet:**
  - deposit roots reach LEZ through the operator's attester (stand-in root source);
  - LEZ finality is the submitter's word;
  - the update image takes private transactions on the sequencer's word (no privacy-receipt check,
    `public-demo/v4/BUILD-outbound.txt`).
- **First relayed claim and burn on the LEZ side** (team deposit, leaf 0): claim LEZ block 10359, burn LEZ
  block 10361, exit toward the deployer; the claim relay serves v4 since about 18:50 UTC
  (`public-demo/RUNBOOK.md` (not published) §18, §18.1).
- **Our first round trip with real proofs (2026-11-15):** the update over LEZ blocks 10266..10362, proved
  and wrapped, was accepted through the gate in Sepolia block 12142209 (tx
  `0x7fd725cd4480a247760aa4abca19fef20d67b39c3e1c43a64c8c192c9a3bb369`, 899,746 gas); exit 0 (the burn
  of LEZ block 10361) was released by proof after the one-hour delay in Sepolia block 12142503 (tx
  `0x8b39bdc7df743f9b6bb23b90866586b02908e64b0d06beaa88e5d1820d74c7bb`, 447,552 gas; the deployer's
  balance +0.01 ETH). A second update (LEZ 10363..10873, 2 exits) was accepted in Sepolia block
  12143000 (tx `0xf7519de9d2964e8d720a81cda7309ab89ef45cca99bd6baedf4dfa1b3d1cab62`, 671,254 gas).
  Proofs, wraps and numbers: `public-demo/prove/receipts/v4-update-1/`, `v4-release-1/`,
  `v4-update-2/`. Read 2026-11-16 through Tenderly: `g2State()` `0x155b12e6…dab0`, `depositCount()` 6,
  two `Released` events. Read-only re-check: `docs/VERIFY-ROUND-TRIP.md`.
- **Source-verified on Sourcify, exact match (2026-11-16, `forge verify-contract --verifier sourcify`, forge 1.5.1):** the v4 vault `0xC2c6…54Ed`, its gate `0xe523…4967` and its desk `0x25C1…7fd5`; the v5 vault `0xAFbD…eD10`, its gate `0x0F3F…15d1` and its desk `0xbAf1…9880` (creation and runtime bytecode both `exact_match`).

| What | Value |
|---|---|
| RISC Zero verifier | `0x8c915b5071206C6E328BC18529586c513408aB3b` (`RiscZeroGroth16Verifier` 3.1.0, selector `0xe71e6620`, control root `e6feef13…551d`; RISC Zero's deployment, not ours) |
| `UpdateGatedVerifier` (the vault's `verifier()`) | `0xe5236Bc2a03235145C0FaCF327fdc9D1c90F4967`, block 12142002, tx `0xb0305e338d15f70cbe2332eaa026ad0d62d4d582c0c3c97ab2ecf69ae306678c`, 2,954,104 gas |
| `SUBMITTER()` | `0xB81aa37D80F8a9588ce9B09B6d6145D1FC4095f4` (a fresh key, not the deployer; funded 0.05 Sepolia ETH with `script/SepoliaFund.s.sol`) |
| `BridgeVault` | `0xC2c6A059eAB928966B24FA5549d441A3B17B54Ed`, block 12142003, tx `0xdc48a114879acfe1d844336e17b21d0f1dfbb23339ba7e187ef39693691b2e2f`, 33,048,001 gas |
| `cfgId()` | `0x0c0f00c30fbe03c00e566a332fe73aa2f11d9e0fd6b8684e509a8be33cbd8be5` (= the v4 bridge Config's) |
| `programId()` | `0x9953f79a352801f441d113dc338e2b6f9be6320729b61d743b1509af9772a486` (bridge `BKXe3q5J…xJBf`) |
| `lezChainId()` / `zoneId()` | `0x2da7f7df…5374` (the demo chain's channel id) / `0x4bd1c1bd…2afb` (stand-in source id), as v3 |
| `updateImageId()` | `0x098f5a1ab0900f06552cca2538999e17954114ad72ac98c7c085c2d1917e3017` (G2-lite trusted tip, risc0-zkvm 3.1.0, `public-demo/v4/BUILD-outbound.txt`) |
| `releaseImageId()` | `0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4` (G3, unchanged) |
| `g2State()` | `0x3d1b762a529d2c0d9eed637913c64e65aabab0961a5a2fbdb1384f6b85a24155` (the genesis state at LEZ block 10265) |
| `minReleaseDelay()` | 3,600 s (1 hour: the freezer's window against a stolen submitter key) |
| Admin, freezer, fee recipient | `0x37a46E7334Dd061E0C7E2298247EE9278d0d321F` (the throwaway deployer) |
| Tokens | ETH 0.01/0.1/1 and dUSDC `0xAc207b9Bfebf3eF7C26297eA6AbaDeEcCC4Aa71e` (reused) 100/1,000/10,000; zero vault fees; 0.5%/1% relay-fee menus; caps as v3; release delay 1 hour |
| `configure()` | blocks 12142006–12142014, 9 transactions, 6,972,240 gas: `registerToken` ×2 (`0x4710066d…244e`, `0xe8081cab…af46`), `setRelayFeeMenu` ×2 (`0x6176501b…63df`, `0x91c2e223…4e89`), `setCaps` ×2 (`0xbefad04d…154a`, `0xe982cdff…848e`), `setGlobalCaps` (`0x728ed102…9699`), `setCapWeight` ×2 (`0x055a43d3…78c6`, `0x030eb4d1…4b72`) |
| DepositDesk v4 | `0x25C1653888e7a4E2c35393A5656b7d1645487fd5`, block 12142016, tx `0x866acbcf67ed21f57e216e378252457e4818f5e958738a5ebd0dcbf31304c877`, 6,516,024 gas; `vault()` = the v4 vault, `feeBps()` 30, owner the deployer |
| First deposit | 0.01 ETH through the v4 desk, a team deposit for the deploy wallet's account D4, leaf 0, Sepolia tx `0x0d4fb073e3984460fa87234ab1b680606d2d507b0ca7a0b6a5901a11c3955387`, block 12142034 (`depositCount()` 1) |

- **Read back on 2026-11-15 through `https://sepolia.gateway.tenderly.co`:**
  - the vault: `g2State`, `cfgId`, `programId`, `verifier()`, both image ids, `minReleaseDelay`, `lezChainId`, `zoneId`, `admin`;
  - the gate: `VERIFIER()`, `SUBMITTER()`, `UPDATE_IMAGE_ID()`;
  - the desk: `vault`, `feeBps`, `owner`.
  - `node scripts/check-versions.mjs --onchain --deployment v4` reads the same and passes.
- **Hashes and gas** are from the forge broadcast records (`broadcast/SepoliaV4.s.sol/11155111/`,
  `broadcast/SepoliaDesk.s.sol/11155111/`, not in git).
- **Fees:** every transaction paid 10,000,016 wei per gas (0.01 gwei): 49,490,369 gas,
  0.000495 Sepolia ETH in all.

Scripts: `script/SepoliaV4.s.sol` (`run()`, then `configure()`; since branch `v5-plan` the delays are required env, and v4's were `MIN_RELEASE_DELAY_SECONDS=3600 RELEASE_DELAY_SECONDS=3600`), `script/SepoliaDesk.s.sol`
(`DESK_VAULT`, `DESK_VAULT_CFG_ID`, `DESK_FEE_BPS=30`), `script/SepoliaFund.s.sol` (test ETH to the
submitter: a plain transfer to a never-used address now needs about 204,600 gas on Sepolia, so it
uses a gas multiplier of 3000). The plan followed: `public-demo/V4-PLAN.md`.

## Sepolia DEMO vault v3 (2026-11-14): operator-attested, with the fixed LEZ programs

**Deployed on Sepolia on 2026-11-14 and paired with the fixed bridge** (the 2026-11-14 security
fixes, `research/security-review-2026-11-14.md`) on the team's public LEZ rc3 demo chain
(`public-demo/RUNBOOK.md` (not published) §16). Initialised and configured. **First deposit and first public claim
done** (below); no release, and no private claim on v3 yet. **v3 can never release** (it can accept
no `update`: below, "How v3 differs"). All three deposits are claimed (public, relayed, and from the Names Wallet). **The claim relay serves v3
since 2026-11-14 14:13** (new payer `6zkdhN9yAmzxkPrJ2Co92QAD7EKESM1AQcmoNEQ3KQpq`, funded 100,000,000
in LEZ block 4739; `public-demo/RUNBOOK.md` (not published) §16.1); **first relayed v3 claim: LEZ block 4802** (leaf 1
below). The deposit page defaults to v3 once the site deploys it; the bridge app's default is v3.

**More team deposits (2026-11-14):** leaf 1, Sepolia tx
`0x632e1823389a052dc1a9d0c06f182664a02baf5b367e299345709a76cd6907d7`, block 12135103, for a never-used
team account (`4YhpVk8KXRWs7eXgPuWTPvVAd2u7S1xzEX3FvN1N6HHY`): the first relayed v3 claim, tx `a86b62d3947a562456711a7eb78ad70d4e374beff351967ea6c7fac16bcbaee0`, LEZ block 4802 (14:28:21), root
`0x698d4bc7…5abe` attested in block 4796, the relay's new payer paid the fee (`public-demo/RUNBOOK.md` (not published) §16.1). Leaf 2, Sepolia tx `0x54418f90144066fe4e918ee78e71262754f013d2b578a28d32241c9d31aed84a`,
block 12135104, for the never-used account `GqopZUdc57htanTsAAVvWfTqzNmLLczg8zNi5aBFzGPp`: the first
claim from the Names Wallet's bridge app (a team test Basecamp, unreleased wallet build 0.17), relayed,
tx `419bc8801a42a3625c16054f94da0e7e81e9563ba422f769c53c491097fcc191`, LEZ block 4852 (14:41) (`public-demo/RUNBOOK.md` (not published) §16).

**First deposit and claim (a team test deposit, 2026-11-14):** 0.01 ETH plus the desk's 0.3% fee
(0.01003 ETH sent) through the v3 desk, tx `0x1e4e95a7c7d79fcdff2afeb2ea984df60a94a35a4ff39de6f118404d381865d8`,
Sepolia block 12134858, 820,681 gas, leaf 0, bound to the team's public LEZ account
`CH9uYfaLePZv2CDmLafQdFaLhqLfdNd1bUEwnLNZ7UHC`. The v3 root attester recorded root
`0xb62e1f712b5614a5fc25e0b10e2a9176be3e82f20e9d5c409eb3e795c7df067d` (deposits 1, finalized block
12134888) in LEZ block 4617 at 13:42:29, 18 minutes after the deposit; the self-paid public claim
(tx `3bd7d5fb8cd685f7498337c0b1d92df509ab446f36192ae19b72b1723790f8ce`) was applied in LEZ block 4703
(`public-demo/RUNBOOK.md` (not published) §16). Read back 2026-11-14: the receipt (to the desk, value, status 1, gas),
the vault's `Deposit` log carrying that root, and `depositCount()` 1 at `finalized`.

**How v3 differs from v2 (read this first):**
- **No honest proof can release v2** (its update image id is a placeholder). It is bound to the 3.0.0 verifier, which GHSA-g6qg-9j8p-7j87 covers, so a forged proof could; test ETH only. **v3, as deployed, cannot accept
  any update either, so nothing can be released from it** (found 2026-11-15): it was deployed with `initialG2State` zero
  (`g2State()` reads `0x00…00`), `update` requires `prevState == g2State`, and the G2-lite guest's
  `prevState` is always the hash of a real state, never zero. Every other check of `update` passes
  on the journal G2-lite produces over the public demo chain; this one fails, attested or proved.
  The bridge's cfgId binds this vault's address, so no later vault can take over the v3 bridge's
  burns either. The only way around it would be a hand-made bootstrap journal (prevState zero,
  newState a genesis state) signed by the attestor: no guest produces it, so signing it breaks the
  attestor's rule (`src/demo/README.md`), and a release would still rest on the attestor's key, not
  on a proof. **Not done.** Its verifier is the operator-attested `DemoAttestedVerifier`, which
  expires on 2027-05-13 18:15:36 UTC (`EXPIRES_AT` 1810232136, 180 days); deposits in v3 stay in the
  vault. `script/SepoliaDemoV3.s.sol` now refuses `initialG2State` zero for any later deployment.
- **Not trustless:** no zk proof is checked on chain; the attestor would check the G3 receipt and run
  G2-lite off chain, then sign (`src/demo/README.md`). No signing service exists yet; the attestor
  key has never signed.
- The vault still enforces releases ≤ burns ≤ claims ≤ deposits per token, caps, the delay,
  nullifiers and the freeze: only deposited principal can leave. Label it "operator-attested demo
  stage" everywhere (`KIND()` says so).
- **cfgId binds the LEZ chain and the root source** (v1/v2: zero): `lezChainId` = the demo chain's
  channel id, `zoneId` = the stand-in root-source id (`DESIGN.md` §2).

| What | Value |
|---|---|
| `DemoAttestedVerifier` | `0xBF416525287D958043e69219CeAdcdF421803948`, block 12134823, tx `0x2d87407791d89ea16c3c52e02b29ffa3ad8cec7641ff712057450b972bb8a566`, 3,829,493 gas |
| `ATTESTOR()` | `0x5b96c4Bfb130B3ed15f9EFf264562955E66721A8` (a new key; encrypted keystore on the owner's workstation; never used) |
| `EXPIRES_AT()` | 1810232136 (2027-05-13 18:15:36 UTC) |
| `BridgeVault` | `0x26af59855b99a8af199361289A5Ed6088729E60E`, block 12134824, tx `0xcb7169d6172358a2c8f97c5cfb36af19b9ab4ec8f630a203428006ac7ed0f52c`, 32,939,697 gas |
| `cfgId()` | `0x9a37708a79334bbce6946e4dab4d9f6530538e867fcec10083b05b7d19f20d15` (equals `lez-bridge cfg-id` and the bridge Config's) |
| `lezChainId()` | `0x2da7f7dfb6e51a28c942ea80391aa518253ec515e98d28cb4d83cc9e16ff5374` (the demo chain's channel id) |
| `zoneId()` | `0x4bd1c1bd6ea22e722a0cad604c306cce32cf6001d735888ec0ea5d1dd0882afb` (stand-in source id) |
| `programId()` | `0x363397603aee2f10bc26aad792cd96692e8bda7faa7c952879564513c822e5af` (bridge `4eaeK3yiqATXhpCEYEFMMtjUn1bS7HLxPwfrH31mXCvr`) |
| `updateImageId()` | `0x6b96962992970cf6e6a2164175fb1c74092077af4f6b9b95bf95c14eb5921bd6` (G2-lite TrustedTip, built from `c9ec880`, `v3/BUILD.txt`) |
| `releaseImageId()` | `0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4` (G3, unchanged) |
| `g2State()` | `0x0000000000000000000000000000000000000000000000000000000000000000` (read 2026-11-15; the constructor's `initialG2State` was zero): no G2-lite journal can match it, so `update` always reverts `StaleState` |
| `minReleaseDelay()` | 600 s (10 minutes; a demo setting) |
| Admin, freezer, fee recipient | `0x37a46E7334Dd061E0C7E2298247EE9278d0d321F` (the throwaway deployer, as v1/v2) |
| Tokens | ETH 0.01/0.1/1 and dUSDC `0xAc207b9Bfebf3eF7C26297eA6AbaDeEcCC4Aa71e` (reused) 100/1,000/10,000; zero vault fees; 0.5%/1% relay-fee menus; caps as v2 |
| `configure()` | blocks 12134827–12134835, 9 transactions, 6,972,240 gas: `registerToken` ×2 (`0x73a6b762…a8b`, `0x65103963…f0a`), `setRelayFeeMenu` ×2 (`0xed519a71…55c`, `0x83222d96…9b2`), `setCaps` ×2 (`0x4589e58e…600`, `0x59dfdcd0…477`), `setGlobalCaps` (`0x7edc705d…348`), `setCapWeight` ×2 (`0x2754ff19…250`, `0x6eeb5c55…a15`) |
| DepositDesk v3 | `0x6b7F38D05751b1ed432a0A0d377A63cFB8c4DBc5`, block 12134838, tx `0xacd85639add419404325936626e1f6e6bfbb97170966fe804f31fdfe038734a6`, 6,516,024 gas; `vault()` = the v3 vault, `feeBps()` 30, owner the deployer |

All values above were read back from Sepolia through PublicNode on 2026-11-14 (`cfgId`, the ids,
the image ids, `minReleaseDelay`, admin/freezer, `ATTESTOR`, `EXPIRES_AT`, `KIND`, the desk's
`vault`/`feeBps`/`owner`); the transaction hashes and gas are from the forge broadcast records
(`broadcast/SepoliaDemoV3.s.sol/11155111/`, `broadcast/SepoliaDesk.s.sol/11155111/`, not in git).
Fees: every transaction paid about 10,000,021 wei per gas (0.01 gwei).

**Before any Sepolia deploy or configuration run:** `node scripts/check-versions.mjs --onchain --deployment <name>`
from the repository root passes, with the new deployment declared in `VERSIONS.toml` first (verifier, image ids and their
ELFs, a non-zero genesis G2 state); after the deploy, record the vault there and run it again before the first deposit.
Every value in this file that `VERSIONS.toml` pins (vault, cfgId, programId, image ids, `g2State`) is checked against it.

Script: `script/SepoliaDemoV3.s.sol` (`run()`, then `configure()`), with `LEZ_CHAIN_ID` and `ZONE_ID`
required (and, since 2026-11-15, a non-zero `INITIAL_G2_STATE`; v3 itself was deployed with zero); desk: `script/SepoliaDesk.s.sol` with `DESK_VAULT`/`DESK_VAULT_CFG_ID` and `DESK_FEE_BPS=30`.
Attestor duties, the signing scheme and test vectors: `src/demo/README.md`. The plan that was followed:
`public-demo/V3-PLAN.md`.

## Sepolia DEMO vault v2 (2026-11-13, fixed code): deposits only, no exits possible

**Current demo vault.** It replaces v1 (below), which ran the pre-fix code. v2 is built from commit `5e2ce69`:
- one `claim refused` text on the LEZ side;
- vault delay history and the `BadRecipient`/`BadRelayer` checks;
- `optimizer_runs = 200`.

**Same terms as v1:**
- `updateImageId` is the placeholder `keccak256("RFP022/demo/no-update-image")`, so no exit can ever be recorded or released. The 0.01 ETH and 1,300 dUSDC deposited are locked for good.
- The admin, freezer and fee recipient are the same throwaway key, `0x37a46E7334Dd061E0C7E2298247EE9278d0d321F`.
- The contract is not verified on any explorer.

Machine-readable record: `deployments/sepolia-demo-v2.json` (laptop), and `sepolia.env` (run sepolia2) plus `sepolia-demo-v2.json` on the test machine.

| What | Value |
|---|---|
| `BridgeVault` v2 | `0x8C8b2664De28e5287158db4DB65D448638871b74` (runtime 20,614 bytes) |
| `DemoUSDC` | `0xAc207b9Bfebf3eF7C26297eA6AbaDeEcCC4Aa71e`: **reused from v1** through the new `REUSE_USDC` option of `script/SepoliaDeploy.s.sol` |
| Verifier | `0x2a098988600d87650Fb061FfAff08B97149Fa84D` (same as v1) |
| `releaseImageId_` | `0x6bcbd714…72a4`: **unchanged**. The test machine's `target-attest` `release_guest.bin` is still sha256 `4238085e…fe05` (built 11-12 14:10), and `release/` and `methods/release_guest` have no commits since `1d07ade` |
| `updateImageId_` | `0x5f425420…82e3`: the placeholder |
| `lezChainId_`, `zoneId_` | zero |
| `programId_` | `0xfff8054169fcabf078493cfda8214d14f03a7294be509dfd9180d29e33fde910`: bridge program `JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7` (image `249a19c6…0129`), deployed immutable on the local sequencer `seq2` |
| `cfgId` | `0x7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11` |
| Configuration | the same as v1: denominations, zero fees, 0.5%/1% relay-fee menus, caps, cap weights, 1-day release delay. Read back on chain after the deposits with `tokenConfig`, `relayFeeMenu` (indexes 0–2), `capState`, `globalCapState` and `capWeight` for both tokens. Results: dUSDC caps 50,000/50,000 per day, weight 1e18; ETH caps 5/5, weight 3e9; global 100,000/100,000; deposit usage 1,300 dUSDC, 0.01 ETH, 1,330 USD |
| `isDepositRoot` | slot index 83, re-checked with `forge inspect … storage-layout` on 11-13 |

- **The `cfgId` was checked before broadcasting.** The simulation predicted the vault address `0x8C8b…1b74` (deployer nonce 19) and the CLI's `cfg-id` for it. The script enforced both through `EXPECTED_VAULT` and `EXPECTED_CFG_ID`, and on chain `cfgId()` returns the same value.
- **Gas pricing:** fee cap 0.1 gwei, tip 0.01 gwei, `--gas-estimate-multiplier 1000`. The effective price was about 10,000,030 wei.

| Step | Tx | Block | Gas used |
|---|---|---|---:|
| create BridgeVault v2 | `0x107b557777b653ac5f48cf89c0b17f9eecbcaa53cbefb5e5db723324c6b39fcc` | 12122976 | 32,895,648 |
| registerToken dUSDC / ETH | `0xc2376f2e…dfb95` / `0xe2b8ee98…a7fa` | 12122979 / 80 | 1,037,730 / 828,103 |
| setRelayFeeMenu dUSDC / ETH | `0xddf6419f…ce83` / `0xb0a14c95…5dec` | 12122981 / 82 | 1,705,443 / 1,705,371 |
| setCaps dUSDC / ETH | `0x342eded1…3606` / `0x676d18cd…786b` | 12122983 / 84 | 477,828 / 477,636 |
| setGlobalCaps | `0x7a86a5b2…2729` | 12122985 | 475,083 |
| setCapWeight dUSDC / ETH | `0x75ed6558…06d9` / `0xce211b6b…2ac4` | 12122986 / 87 | 132,673 / 132,397 |
| mint 1,300 dUSDC / approve | `0x16ada5c7…b852` / `0x44d1a3c3…f88b` | 12122991 / 92 | 42,292 / 127,839 |

| # | For | Amount | Leaf | Commitment | Tx | Block | Gas |
|---|---|---|---|---|---|---|---:|
| 1 | V2 (private) | 100 dUSDC | 0 | `0xa50e0f44…ad2e` | `0x5ca7529743269c4ce77fe9d1f2777d6f47a96030fed4113091853be8a925598e` | 12122993 | 801,427 |
| 2 | V2 | 100 dUSDC | 1 | `0x188c6f3a…bacd` | `0x3543c1b3a68c7ad037930054784f18929fd4d7ef39b4129ae34b3681377887c3` | 12122994 | 510,476 |
| 3 | V2 | 1,000 dUSDC | 2 | `0x4b7b9d32…e514` | `0x50ecdb3c2e262de6380c791840b7aa453b550ad0626785354c60ee03ad603d7a` | 12122995 | 509,832 |
| 4 | P2 (public) | 100 dUSDC | 3 | `0x6cdd53cd…878d` | `0x7a0ca82a6dd36e9276fc0c098b12645a4c9d4fcce34397a51cc62ba189c2e7b0` | 12122996 | 513,309 |
| 5 | V2 | 0.01 ETH | 4 | `0xffb6f336…7e37` | `0x75239c1c373d9932009a55b0254db20fcfe0529e7e1d801dd43acd127cd70a76` | 12122997 | 471,329 |

- **Later deposits:** the vault holds 9 deposits in all (`depositCount()` read through the Tenderly gateway on 2026-11-14 afternoon), 3 of them through the deposit desk (below). All are locked like these. The ninth is the team's leaf 8 (0.01 ETH at 0.3%, Sepolia tx `0x77892830…94e2`, block 12134587), made for a wallet test that then moved to v3: unclaimed, and claimable on v2, self-paid (the relay serves v3 only).
- **LEZ accounts:** V2 = `2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC` (private), P2 = `6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f` (public).
- **Secrets:** fresh random values, kept only in `secrets.env` (run sepolia2) (mode 600) on the test machine.
- **Root after the last deposit:** `0xbe1e080d85207b6b7832392daf38ce65347cabe7902bed16e3251ee03a21d3e8` (`depositCount` 5).
  - Last deposit: block 12122997, hash `0x8cdcc813…ac9d`, slot 11566945.
  - Slot key `keccak256(abi.encode(root, 83))` = `0x8a4947a6b1393817e081b66486230d161aecbd58de58c26d7a3e0c69dff781cd`; its value reads 1.
- **Repeated commitment:** `deposit` with commitment 1 and `depositETH` with commitment 5 revert with `CommitmentUsed()` (`0x7b000717`). Checked by `eth_call` only. Control: a fresh commitment estimates 428,510 gas.
- **Cost:** 17 transactions, 0.000428445420351432 ETH in fees, plus the 0.01 ETH deposit. The deployer went from 5.489488275920805701 to 5.479059830500454269 ETH.

## Sepolia DEMO vault v1 (2026-11-12): SUPERSEDED 2026-11-12/13

**Superseded** by v2 (above): pre-fix code; kept for the evidence of the first attested claims (`research/attested-claim-sepolia.md`). Its bridge program is `55yo6…6q9o` on `seq2`. The CLI binary that matches its images is kept on the test machine as `lez-bridge/target/release/lez-bridge.v1-20261112`.

**This vault is a demo. It takes deposits and nothing can ever leave it.**
- `updateImageId` is a placeholder, `keccak256("RFP022/demo/no-update-image")`. The G2 outbound state proof is not built, so no `update` can verify.
- Without `update`, no exit is ever recorded, and `release` always fails with `UnknownExitRoot`.
- The 0.01 ETH and 1,300 dUSDC deposited here are locked for good.
- Exits need a **new vault version** with the real G2 image. The verifier and the image ids are immutable.

**The admin, freezer and fee recipient are a throwaway test key** (`0x37a46E7334Dd061E0C7E2298247EE9278d0d321F`). It holds Sepolia test ETH only. Treat this vault's admin as untrusted.

The contracts are **not verified** on Etherscan or Sourcify, and no source is published.

Machine-readable record: `deployments/sepolia-demo.json` (laptop) and `sepolia.env` (run sepolia1) (test machine).

### Addresses

| What | Value |
|---|---|
| Chain | Sepolia, chain id 11155111 |
| `BridgeVault` | `0xA34adbe8413838668b0521230A4d96C6B7e91484` (runtime 22,721 bytes) |
| `DemoUSDC` (dUSDC, 6 decimals, only the deployer can mint, no value) | `0xAc207b9Bfebf3eF7C26297eA6AbaDeEcCC4Aa71e` (`script/DemoUSDC.sol`) |
| Verifier | `0x2a098988600d87650Fb061FfAff08B97149Fa84D`: RISC Zero `RiscZeroGroth16Verifier`, bare and immutable |
| Deployer / admin / freezer / fee recipient | `0x37a46E7334Dd061E0C7E2298247EE9278d0d321F` (throwaway) |
| Script | `script/SepoliaDeploy.s.sol`, in three stages: `run()` creates, `configure()`, `deposits()` |

### Verifier checks (on chain, 2026-11-12)

| Check | Result |
|---|---|
| `VERSION()` | `"3.0.0"` |
| `SELECTOR()` | `0x73c457ba`. A bad seal reverts with `SelectorMismatch(0xdeadbeef, 0x73c457ba)` |
| `CONTROL_ROOT_0` / `CONTROL_ROOT_1` | `0x41af1873…4da5` / `0x561f8c99…e7db`. Byte-reversed, these give `ALLOWED_CONTROL_ROOT` `a54dc85a…8c1f56` from `risc0-circuit-recursion` 4.0.5, which both `Cargo.lock`s pin (risc0-zkvm 3.0.5) |
| `BN254_CONTROL_ID` | `0x04446e66…657ac0`. Byte-reversed, this gives `BN254_IDENTITY_CONTROL_ID` `c07a6514…6e4404` (same crate) |
| Not checked | The bytecode against the risc0-ethereum source. No explorer API was used |

### Constructor arguments

| Argument | Value |
|---|---|
| `verifier_` | `0x2a098988600d87650Fb061FfAff08B97149Fa84D` |
| `updateImageId_` | `0x5f425420081746e3e77f3c94e7a44e7d0b3bdcc3cd4e114eaa07d7540fa282e3` = `keccak256("RFP022/demo/no-update-image")`: a **placeholder**, so no exits are possible |
| `releaseImageId_` | `0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4`: the G3 guest `RELEASE_GUEST_ID` from a **local, non-reproducible build** on the test machine (`target-attest`, 2026-11-12 14:10; `release_guest.bin` sha256 `4238085e159037073ecfa83db6872fe2ed46aa481e912d79ba6ff7b2c507fe05`). The sources match the laptop's by hash. It is not reproducible with `cargo risczero build`, so a real G3 seal must come from this exact ELF |
| `lezChainId_`, `zoneId_` | zero bytes32: the local LEZ 0.4 test chain has no ids |
| `programId_` | `0x3cb549f35aa0b8ba9e9f7864d85cb5b383d8f2950fbe79ec273e4feb0001d646`: bridge program `55yo6Et5XtzSLFYNe9gjQhhBhd3X4LcidsUaPanw6q9o` on the **local** LEZ 0.4 sequencer `seq2` on the test machine (127.0.0.1:3250), deployed immutable |
| `initialG2State` | zero |
| `minReleaseDelay_` | 86,400 (1 day) |
| `admin_`, `freezer_`, `feeRecipient_` | the deployer |

`cfgId` = `0xda620b16999ff986739727aef71641d74f04e745fd197693ed1211b8f2fc0118`. It equals `lez-bridge cfg-id --vault 0xA34a… --chain-id 11155111 --lez-chain-id 0 --zone-id 0 --bridge 55yo6…`, which was checked before the broadcast and again on chain (`computeCfgId`).

### Configuration

| Token | Denominations | Deposit / withdraw fee | Relay-fee menu (0 always allowed too) | Per-token caps (1-day window) | Cap weight | Release delay |
|---|---|---|---|---|---|---|
| dUSDC | 100, 1,000, 10,000 | 0 / 0 | 0.5%, 1% of each: [0.5, 1], [5, 10], [50, 100] | deposit 50,000, release 50,000 | 1e18 | 1 day |
| ETH | 0.01, 0.1, 1 | 0 / 0 | 0.5%, 1%: [0.00005, 0.0001], [0.0005, 0.001], [0.005, 0.01] | deposit 5, release 5 | 3e9 ($3,000/ETH) | 1 day |

- **Global caps:** 1-day window, deposit 100,000 and release 100,000, in USD units with 6 decimals.
- **The cap values are this deploy's own choice.** No recommended defaults are recorded yet (DESIGN S4 is open). Each cap is above the token's largest denomination.

### Transactions

| Step | Tx | Block | Time (UTC) | Gas used |
|---|---|---|---|---|
| **FAILED** first DemoUSDC create, nonce 0 (out of gas, see below) | `0x8602ac1b6ecfa4847eabab83e9a9595b5efa68500177d17b021b942fe8f10034` | 12121169 | | 804,667 |
| create DemoUSDC | `0x27a170fb593e42aab050ef25d4c1e53c38373b1ed2a44f4cadf4934c2dddd923` | 12121199 | 20:24:48 | 4,098,169 |
| create BridgeVault | `0x42a596755476edc08a4563d61061d33b99424bf8f78d17288b5670f4fb231322` | 12121200 | 20:25:00 | 36,133,590 |
| registerToken dUSDC | `0xdd8c3b67477c5e6fc5ddb9ca931a18ac2195146db325e2a1cffd43819fe09f4d` | 12121204 | 20:25:48 | 1,037,498 |
| registerToken ETH | `0x7859c57668e3d3e3f9c272466830a26119502cf59ebcf4e742a9f4674dd23e23` | 12121205 | 20:26:00 | 827,892 |
| setRelayFeeMenu dUSDC | `0x994d4f2d50ca1a3a4d21d2f9d49f6aa509c96816b9ac6336faa36091687a848a` | 12121206 | 20:26:12 | 1,705,208 |
| setRelayFeeMenu ETH | `0x9effb8d02a13ce3f83d0b391677e0b861d1895d3ff91e69d6ac8cd2cc0c05385` | 12121207 | 20:26:24 | 1,705,136 |
| setCaps dUSDC | `0x23eabef61d5dc64f479c203b246803f418846356f9d95486640bd5de19b11142` | 12121208 | 20:26:36 | 477,589 |
| setCaps ETH | `0x92745cdfaad9a3490201c4d2a89c4c278c5f7ee89d4d5f83a86192766dc788b6` | 12121209 | 20:26:48 | 477,397 |
| setGlobalCaps | `0x6f3f366207169219cf85ba9f61f945c818b345a69aef726a9f6dc8fbd4b39048` | 12121210 | 20:27:00 | 474,850 |
| setCapWeight dUSDC | `0x02ed511a12939507041739f9fb990f88cf44704bb77a477d25faab0e0818f28b` | 12121211 | 20:27:12 | 132,616 |
| setCapWeight ETH | `0x6d6c28084d80dea1d9b37fce4ff9270660f6c4b520f06bd11ab109278234c02d` | 12121212 | 20:27:24 | 132,340 |
| mint 1,400 dUSDC to deployer | `0x1244cd0741446cf805ad27521fc6293b65e45386c9d7cf0d70ef8a68b88d4519` | 12121218 | 20:28:36 | 238,132 |
| approve 1,400 | `0x0addfd0c694bcdf454e013b509e82e02ca18288794d802d81a34d9b6e2d0aa2a` | 12121219 | 20:28:48 | 127,839 |

- **Fees:** 0.000511724079194299 ETH for all 19 transactions, including the failed one. Every transaction paid 10,000,015 wei per gas (the failed one 10,000,017).
- **Balance:** the deployer went from 5.5 to 5.489488275920805701 ETH. That is the fees plus the 0.01 ETH deposit.

### Deposits

The commitments were computed with `lez-bridge commitment` on the test machine. The secrets are only in `secrets.env` (run sepolia1) there (mode 600).

| # | For | Amount | Leaf | Commitment | Tx | Block | Time (UTC) | Gas |
|---|---|---|---|---|---|---|---|---|
| 1 | V (private) | 100 dUSDC | 0 | `0x3526387494d14b26f388be0566fd27c743709d16c47203f0968b949ea883370b` | `0x220a9257ddfe6f9ee11d80f6be2fcc6a12c84f3f68ed37aaa79a7998b7e47bac` | 12121220 | 20:29:00 | 800,033 |
| 2 | V | 100 dUSDC | 1 | `0xb291628f77d31eb09bb4076d42db0117f76b3a3aaaee3888df078fe1547e4d89` | `0x110800c5a720fc1e1dde9097ba6b80498ba3ca894af13b98ca1d0c5a21bcabf6` | 12121221 | 20:29:12 | 509,076 |
| 3 | V | 1,000 dUSDC | 2 | `0xcbb24950fe34ecc43c18fc299a6535f12a55fbfe333ba61a76aaf95958910696` | `0x779e97d1e80519fd3ccbb480f7d1960f4bdcf37ebfc18e23cf392fd04c3e25ff` | 12121222 | 20:29:24 | 508,450 |
| 4 | P (public) | 100 dUSDC | 3 | `0xd7d6996cde989fcc0ad5277536e95c23c7eb39434fa542310ed7109d4aecfe25` | `0x2ca9753b40deb5fa4da8bf87f2f2127fbe36f9b51ade3602f26ddfa880f703d4` | 12121223 | 20:29:36 | 511,879 |
| 5 | V | 0.01 ETH | 4 | `0x805438345af1dbd3ff69d56656a1db676787ca4219eac56138293db4748cab53` | `0xa9351249aafdcff2ea9c5dea5ac07e9a8aa2656b7fdc17337be55307e8b0581b` | 12121224 | 20:29:48 | 469,970 |

LEZ accounts on the local sequencer:
- V = `DMv13rBBeDRPpcJUea7Buts4xwhx3C4EA13zWVBi2HJ1` (private);
- P = `HPqQSHYDmQ9Ma9ySWFFeGZDqN7fCiVnFrqzH8ag29BzW` (public).

**Roots and the repeated-commitment check**
- Empty root at deployment: `0xc6f67e02e6e4e1bdefb994c6098953f34636ba2b6ca20a4721d2b26a886722ff`.
- **Root after the last deposit:** `0x88031a1cde4ab9bbbc7c9320070e80487b8e531c47ecb02c82ad0080fd810394` (`depositCount` 5).
- A repeated commitment reverts with `CommitmentUsed()` (`0x7b000717`), for both `deposit` (commitment 1) and `depositETH` (commitment 5). This was checked by `eth_call` and gas estimate only. As a control, a fresh commitment estimates 427,090 gas from the same account.

### Storage-proof target for the attested claim

| What | Value |
|---|---|
| Slot | `isDepositRoot[root]`, mapping at slot index **83** (`forge inspect` storage layout, re-checked at deploy) |
| Slot key | `keccak256(abi.encode(0x88031a1c…0394, 83))` = `0xcc872c95ba998c95918c52e9d801f1cbe7b21b0b9777c867831df82669230dfc` |
| Value | `cast storage` reads `0x…01` |
| Last deposit | execution block 12121224, beacon slot 11565149 (epoch 361410, Gloas). The slot's `signed_execution_payload_bid.block_hash` is that block's hash, `0xa40d4ae084f870a14a514301eb3a356d8cfcb0d6a24680647567e7c6591d0c6f` |
| Finalized | block 12121226 was seen finalized at 20:43:13 UTC (about 13.5 min after the deposit). The beacon finalized checkpoint was then epoch 361411 (slot 11565152), root `0x536d5f2c47ef9376734f65244de548d1c3bbe43f89ddc850bba130a37858e6b0` |
| Next step | initialise the attestation instance at a finalized checkpoint **after** slot 11565149, then update it past that point |

- **Sample `eth_getProof` proofs:** two, both in `deployments/sepolia-demo.json`:
  - one at the latest block, 12121234;
  - one at the finalized block 12121226 (block hash `0x5545e9c0…c9b1`, state root `0x13c573cc…16f0`), which only thirdweb served.
  - Both are samples. The claim needs a proof at the anchor that the attestation instance finalizes.
- **Which endpoints serve `eth_getProof`:**
  - **thirdweb** (`11155111.rpc.thirdweb.com`) served both the latest and the finalized block.
  - **publicnode** served only near the head. At the finalized block (behind the head; the head-to-finalized gap was about 70 blocks earlier that day) it refused with "distance to target block exceeds maximum proof window" (code 4444).
  - **1rpc** served the latest block and returned 400 for the finalized one.
  - **drpc** returned 400; **rpc.sepolia.org** returned 404.
  - **For the claim, use thirdweb** (or a node with state history), because the anchor will be well behind the head.
- **Raw header:** none of the three serves `debug_getRawHeader`. A Gloas-era anchor needs the header RLP (DESIGN: Sepolia anchors are by block hash plus header RLP), so the client must build it from `eth_getBlockByNumber`.

### Gas on Sepolia differs from forge's local EVM (2026-11)

- **What happened:** forge set gas limits from its local simulation (×1.3). On chain the first DemoUSDC create ran out of gas: 618,975 gas locally, against about 4.1M needed on Sepolia.
- **On-chain cost against local:**
  - vault create: 36.1M on chain, about 5.1M locally;
  - `registerToken`: about 1.04M, against about 283k;
  - first ETH deposit: 681k (estimate), against about 207k with caps on in the local gas table.
- **How the successful runs were sent:** with the command-line flags `--gas-estimate-multiplier 1000 --with-gas-price 100000000 --priority-gas-price 10000000`. The flags are not in the script. Take the limits from `eth_estimateGas`.
- **Cause (found later, `../research/gas-sepolia-vs-local.md`):** Sepolia runs Glamsterdam, whose execution-layer (Amsterdam) part reprices gas: EIP-8037 (state creation: 1,530 gas per state byte, so a new storage slot costs 97,920 state gas and code costs 1,530 per byte), EIP-8038 (state access) and EIP-2780 (intrinsic gas). All 18 successful v1 transactions were reproduced to the unit on a local anvil (Foundry 1.9.0) with `--hardfork amsterdam`; the same transactions under `--hardfork osaka` give today's mainnet-rule costs (vault create 5,140,054). It is not the consensus-layer (Gloas) part of the fork.
- **Effect:** gas measured with forge's local EVM does not hold on Sepolia. The research note has both rule sets, and `README.md` quotes them.
- **Per-transaction cap:** none at 2^24. Sepolia includes transactions with gas limits up to about 82M. The scan covered the 200 blocks up to the head at about 20:23 UTC; the largest limit was in block 12121145, and 202 transactions were above 2^24.

## DepositDesk (bridge fee), Sepolia, 2026-11-13

- Address: `0x77E37E1D6e886d8fb988496B9c2EfF3E78e27DF2`
- Vault: `0x8C8b2664De28e5287158db4DB65D448638871b74` (demo vault v2, unchanged)
- Owner: `0x37a46E7334Dd061E0C7E2298247EE9278d0d321F` (the demo deployer)
- Fee: 300 bps (3%) at deploy, hard cap 300; **30 bps (0.3%) since 2026-11-14** (below)
- Deploy tx `0xa224c204261dc434a41c9b7c1d56002f9ba5af4e6c44926593cd904b28190873`, block 12128491, gas used 6,516,036
- Checked after deploy: `vault()`, `owner()`, `feeBps()` read back as above; a simulated
  `depositETHWithFee(0, <random>, 0)` with 0.0103 ETH returns leaf index 6, and with 0.01 ETH reverts.
- Deposits through the desk: 3 (`FeePaid` logs, read 2026-11-14 afternoon through the Tenderly gateway): two at 300 bps, and the team's leaf 8 at 30 bps (Sepolia tx `0x7789283082132c4cdc3fb47fc63e787635c6a631c1e1b62aa11fc268ee7594e2`, block 12134587, 0.01003 ETH; unclaimed, claimable on v2 self-paid). The team's leaf 6: 0.01 ETH, Sepolia tx `0x7933e194…549e`, block 12129802, leaf 6, fee 0.0003 ETH, claimed by a never-used account through the claim relay (LEZ block 552, `DESIGN.md` §3.6). The grants daemon has sent no grant (`public-demo/RUNBOOK.md` (not published) §12).

### Fee change to 30 bps (0.3%), 2026-11-14

- `setFeeBps(30)` by the owner, sent with `script/SepoliaDeskSetFee.s.sol` (`FEE_BPS=30`): tx `0xdf39c20d547af313665007ef44ab645d00f30044c7f884eea012a922f0c805cd`, Sepolia block 12132632 (the desk's `FeeBpsSet` log, data 30). Foundry's broadcast record of that run is in `vault/broadcast/` and is not committed.
- Read back through PublicNode on 2026-11-14: `cast call 0x77E37E1D6e886d8fb988496B9c2EfF3E78e27DF2 "feeBps()(uint256)" --rpc-url https://ethereum-sepolia-rpc.publicnode.com` → `30`; `MAX_FEE_BPS()` → `300`; `owner()` → the deployer above.
- The deposit page reads the rate on chain (`feeBps`), so it quotes 0.3% with no page change. A rate change between a quote and the transaction makes the deposit revert (exact amounts, `test_FeeChangeAfterQuoteReverts`).
- Why 0.3%: `research/open-settlement.md` ("Fee policy").
