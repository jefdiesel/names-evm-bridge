// Built-in allowlist of known vault deployments. A request for any other vault is refused
// outright on this page (DEV.allowUnlistedVaults keeps the old warn-and-confirm path for
// development only; it is false in every published build, and check.mjs fails otherwise).
// Values come from vault/deployments/sepolia-demo-v2.json and vault/DEPLOYMENTS.md (v2 and v3).
//
// SPDX-License-Identifier: MIT OR Apache-2.0

export const CHAINS = {
  1: { name: 'Ethereum mainnet', testnet: false, explorer: 'https://etherscan.io', nativeSymbol: 'ETH', blockSeconds: 12 },
  11155111: { name: 'Sepolia', testnet: true, explorer: 'https://sepolia.etherscan.io', nativeSymbol: 'SepoliaETH', blockSeconds: 12 },
  560048: { name: 'Hoodi', testnet: true, explorer: 'https://hoodi.etherscan.io', nativeSymbol: 'HoodiETH', blockSeconds: 12 },
};

/**
 * The only networks this build will ever read from or send on. A request for any other chain
 * (Ethereum mainnet included) is refused outright when it is loaded, whatever its vault, and
 * the wallet is only ever asked to switch to a chain on this list.
 */
export const SUPPORTED_CHAIN_IDS = [11155111n];
export const isSupportedChain = (chainId) => {
  try {
    return SUPPORTED_CHAIN_IDS.includes(BigInt(chainId));
  } catch {
    return false;
  }
};

/**
 * Development switches. Both false in the published page.
 * allowUnlistedVaults: let a request name a vault not in DEPLOYMENTS, behind the
 * "Unknown vault. Stop and check." confirmation. Off: such a request is refused at load,
 * before any read (review C, MEDIUM: a fake vault could report decimals that make the page
 * show one amount while the wallet sends another).
 */
export const DEV = { allowUnlistedVaults: false };

const SEPOLIA_DEMO_V2 = {
    id: 'sepolia-demo-v2',
    label: 'Sepolia demo vault v2',
    chainId: 11155111,
    vault: '0x8c8b2664de28e5287158db4db65d448638871b74',
    deployBlock: 12122976,
    cfgId: '0x7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11',
    // THE ONE SWITCH for the bridge fee: the DepositDesk address (vault/script/SepoliaDesk.s.sol).
    // Empty = off: deposits go straight to the vault, as before. Set = on: EVERY deposit on this
    // page goes through the desk and pays its bridge fee (read from the desk, never from here).
    desk: '0x77E37E1D6e886d8fb988496B9c2EfF3E78e27DF2',
    demo: true,
    // The team's private demo account V is offered (and is the default destination): a private
    // claim of a v2 deposit to V was proved and applied on the public demo chain (LEZ block 3069).
    demoAccount: '2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC',
    // Shown before every deposit into this vault and on the Deposited screen: v2's exit image
    // is a placeholder, so no exit can ever be recorded or released (vault/DEPLOYMENTS.md).
    demoNote: 'A deposit into this demo vault is locked on Ethereum for good: it can be claimed on the bridge test network (LEZ 0.4), but never withdrawn back to Ethereum.',
    // One visible line (the page's plain layout, 2026-11-14); demoNote stays, behind "More".
    demoShort: 'Demo vault, test coins only: a deposit here can never be withdrawn back to Ethereum.',
    tokens: {
      // `denoms` as configured at deployment (vault/DEPLOYMENTS.md), for the request tool.
      // The page never trusts these: it reads tokenConfig from the chain before every deposit.
      '0x0000000000000000000000000000000000000000': { symbol: 'ETH', unit: 'Sepolia ETH', name: 'Sepolia ETH, the test network’s own coin', decimals: 18, denoms: ['10000000000000000', '100000000000000000', '1000000000000000000'] },
      '0xac207b9bfebf3ef7c26297ea6abadeeccc4aa71e': {
        symbol: 'dUSDC',
        unit: 'dUSDC',
        name: 'demo USDC (Sepolia test token)',
        decimals: 6,
        denoms: ['100000000', '1000000000', '10000000000'],
        howToGet: 'Only the demo operator can mint demo USDC. Ask the operator for some, or try the 0.01 ETH deposit instead.',
      },
    },
};

/**
 * v3 (vault/DEPLOYMENTS.md, "Sepolia DEMO vault v3"; public-demo/RUNBOOK.md §16): the fixed LEZ
 * programs, paired with a vault whose verifier is operator-attested. It can never release anything
 * (found 2026-11-15): it was deployed with its outbound state at zero, and no update the bridge can
 * produce starts from zero (research/outbound-public-measure-2026-11-15.md). Withdrawals need a
 * later vault (the visible line says so without "never": the owner read that as the whole bridge
 * having no way back, 2026-11-15). Its claim files bind v3's cfgId and are claimed with the v3 bridge only.
 *
 * The page's default since 2026-11-14 (first v3 deposit and public claim: LEZ block 4703; the
 * claim relay serves v3 since 14:13). about/security/index.html names this vault and says the same
 * as `demoNote` under "Limits today": change both with it.
 *
 * No demo account (`demoAccount: null`): the only team account a visitor's deposit could go to by
 * default is the private account V, and a private claim on v3 has not been proved yet (it needs a
 * proving run). So on v3 "Use my own account" is the only choice; its claim is a public claim the
 * relay pays for.
 */
const SEPOLIA_DEMO_V3 = {
    id: 'sepolia-demo-v3',
    label: 'Sepolia demo vault v3',
    chainId: 11155111,
    vault: '0x26af59855b99a8af199361289a5ed6088729e60e',
    deployBlock: 12134824,
    cfgId: '0x9a37708a79334bbce6946e4dab4d9f6530538e867fcec10083b05b7d19f20d15',
    desk: '0x6b7F38D05751b1ed432a0A0d377A63cFB8c4DBc5',
    demo: true,
    demoAccount: null,
    demoNote: 'Why: the vault on Sepolia was deployed with its outbound starting state at zero, and no update the bridge produces starts from zero. The bridge’s way back (burn, update, release) is built and runs in the local demo; it needs a new vault. Test coins only.',
    // One visible line (the page's plain layout, 2026-11-14); demoNote stays, behind "More".
    demoShort: 'This test vault was set up with a wrong starting value, so it can’t pay ETH back out. Deposits can be claimed on the bridge test network; withdrawals come with the next vault.',
    tokens: SEPOLIA_DEMO_V2.tokens,
};

/**
 * THE ONE LINE that picks the page's deployment: new deposits go into `DEPLOYMENTS[0]`
 * (src/app.js). v3 since 2026-11-14; both stay listed, so wallet requests for either vault are
 * recognised (a v2 deposit is claimed on the v2 bridge, self-paid since the relay moved to v3).
 */
const LIVE_DEFAULT = 'sepolia-demo-v3';

export const DEPLOYMENTS = LIVE_DEFAULT === SEPOLIA_DEMO_V3.id ? [SEPOLIA_DEMO_V3, SEPOLIA_DEMO_V2] : [SEPOLIA_DEMO_V2, SEPOLIA_DEMO_V3];

/**
 * Where the bridge's Logos side runs, and whether visitors can claim there themselves. ONE
 * place: the page's copy and its own-account tick follow from these fields. Names (site rule,
 * nameswallet-site partials/README.md §4): LEZ 0.4 = "bridge test network", LEZ 0.3 = "public
 * Logos testnet"; the bridge does not run on LEZ 0.3.
 * `selfClaim` means a visitor can claim on their own with a released tool. The fee is not the
 * visitor's problem: the bridge's claim relay co-signs a public claim as its fee payer, so the
 * account needs no LEZ. It is false today: the network is public (`publicRpc`) and claims to any
 * account work, and the Names Wallet has "Claim a bridge deposit" (Wallet → LEZ 0.4), but the wallet build with
 * it (0.17) is not released yet and the claim tool is not published. Flip it when that build is
 * released (the page's copy then says the claim can be made now).
 */
export const LOGOS_SIDE = {
  network: 'the bridge test network (LEZ 0.4)',
  publicRpc: 'https://lez.nameswallet.app/',
  selfClaim: false,
};

/**
 * LEZ accounts the team holds on the bridge test network (vault/DEPLOYMENTS.md). A deposit to one
 * of them needs no tick, as long as the team can claim it on the live deployment: a public one
 * always (a public claim), the private one only where the deployment offers it as `demoAccount`
 * (a private claim there has been proved). Any other account needs the tick while `selfClaim` is
 * false.
 */
export const TEST_CHAIN_ACCOUNTS = [
  { id: '2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC', kind: 'private', note: 'demo account (V2)' },
  { id: '6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f', kind: 'public', note: 'public demo account (P2)' },
];
/** The live deployment's demo account, or null when it offers none (v3). */
export const DEMO_ACCOUNT = DEPLOYMENTS[0].demoAccount || null;
export const isTestChainAccount = (id, deployment = DEPLOYMENTS[0]) =>
  TEST_CHAIN_ACCOUNTS.some((a) => a.id === id && (a.kind === 'public' || a.id === deployment?.demoAccount));

/**
 * The deposit desk of a deployment, or null when the bridge fee is off. A value that is not
 * an address keeps it off (and fails the test in test/desk-flow.test.mjs).
 */
export function deskOf(deployment) {
  const a = String(deployment?.desk || '').toLowerCase();
  return /^0x[0-9a-f]{40}$/.test(a) && !/^0x0{40}$/.test(a) ? a : null;
}

/** Superseded deployments: requests for these are refused outright, with the reason. */
export const RETIRED = [
  {
    chainId: 11155111,
    vault: '0xa34adbe8413838668b0521230a4d96c6b7e91484',
    reason: 'This is the retired Sepolia demo vault v1. Its bridge program no longer runs; deposits into it could never be claimed. Make a new request in your Logos wallet.',
  },
];

export function findDeployment(chainId, vault) {
  const v = String(vault).toLowerCase();
  const c = Number(chainId);
  return DEPLOYMENTS.find((d) => d.chainId === c && d.vault === v) || null;
}

export function findRetired(chainId, vault) {
  const v = String(vault).toLowerCase();
  return RETIRED.find((d) => d.chainId === Number(chainId) && d.vault === v) || null;
}

export function chainInfo(chainId) {
  return CHAINS[Number(chainId)] || { name: `chain ${chainId}`, testnet: false, explorer: null, nativeSymbol: 'ETH', blockSeconds: 12 };
}

/**
 * Optional read-only RPCs (used only if the person picks one in Settings, and only when
 * no wallet is connected). Each origin must also be listed in the page's CSP connect-src.
 * thirdweb's keyless Sepolia endpoint was tried and dropped: it answers HTTP 429 to
 * repeated reads and limits eth_getLogs to 1,000 blocks (2026-11-13).
 */
export const READ_RPCS = [
  { id: 'publicnode', label: 'PublicNode (Sepolia)', chainId: 11155111, url: 'https://ethereum-sepolia-rpc.publicnode.com' },
];

/**
 * Commitments of the published example requests (README, test vectors). They are SHA-256
 * hashes of public labels, so no deposit secret exists for them and a deposit made with one
 * could never be claimed. The page shows these requests but will not send them.
 */
export const EXAMPLE_COMMITMENTS = new Set([
  '0x962f9a3ffc44690aac8d3d0ea8af24ff63b54b58624534c10dee46b139dfdb0f',
  '0xc3151049b536194e935bf261d8f841244921fe2892826663238327362cb3dbf3',
  '0x910140139fc3c687e14887a1b731e4397df0078990108880f9a1a102fde24a44',
]);
