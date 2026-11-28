// The deposit flow as a DOM-free state machine. The page renders `flow.state`; the tests
// drive the same object with a mock EIP-1193 provider (test/flow.test.mjs).
//
// Steps:
//   request        no request loaded yet (or it failed to parse)
//   connect        a request is loaded; no wallet connected
//   network        the wallet is on another chain than the request
//   checking       reading the vault, token and wallet state
//   blocked        a check failed; `error` says why and what to do
//   ready          every check passed; next action is approve (ERC-20) or deposit
//   approve-sign   waiting for the person to sign the approval in the wallet
//   approve-wait   approval sent, waiting for it to be mined
//   deposit-sign   waiting for the person to sign the deposit
//   deposit-wait   deposit sent, waiting for it to be mined
//   finalizing     mined; waiting for Ethereum finality
//   done           finalized (or, if the wallet cannot report finality, past the estimate)
//   failed         the deposit transaction reverted or vanished
//
// SPDX-License-Identifier: MIT OR Apache-2.0

import { parseRequestInput } from './request.js';
import { call, decode, decodeRevert, extractRevertData, decodeDepositLog, DEPOSIT_TOPIC, ETH, ZERO32 } from './vault.js';
import { rpc as makeRpc, isUserRejection, scanLogs, fallbackProvider, failureWords, isTransportError } from './providers.js';
import { findDeployment, findRetired, chainInfo, isSupportedChain, isTestChainAccount, LOGOS_SIDE, EXAMPLE_COMMITMENTS, DEPLOYMENTS, deskOf, deskTakesMaxTotal, DEV } from './deployments.js';
import { parseLogosAccount, makeDeposit, claimFile as buildClaimFile } from './create.js';
import { formatUnits } from './units.js';

/** Ethereum finalizes after two epochs: 64 slots of 12 s. Typical lag is 64 to 96 blocks. */
export const FINALITY_TYPICAL_SECONDS = 64 * 12 + 2 * 60; // ~15 min including a little slack
export const FINALITY_LATE_SECONDS = 30 * 60;

const CHECK_LABELS = {
  vault: 'Vault is a known deployment',
  code: 'Vault contract exists on this network',
  token: 'Vault accepts this token',
  amount: 'Amount is one of the vault’s denominations',
  desk: 'Bridge fee read from the deposit desk',
  open: 'Deposits are open',
  cap: 'Within the vault’s deposit limit',
  commitment: 'Commitment has not been used',
  balance: 'Enough funds in your wallet',
  allowance: 'Approved to take the amount',
  simulate: 'Deposit simulated against the chain',
};

function freshChecks(isToken, desk = false) {
  const ids = ['vault', 'code', 'token', 'amount', ...(desk ? ['desk'] : []), 'open', 'cap', 'commitment', 'balance'];
  if (isToken) ids.push('allowance');
  ids.push('simulate');
  return ids.map((id) => ({ id, label: CHECK_LABELS[id], status: 'pending', detail: '' }));
}

const initial = () => ({
  step: 'request',
  requestText: '',
  request: null,
  requestError: null,
  deployment: null,
  chain: null,
  token: null, // { address, symbol, unit, name, decimals, isEth, known }; `unit` is what amounts are written in
  wallets: [],
  wallet: null, // { id, name, icon }
  account: null,
  walletChainId: null,
  readOnly: false, // checks are running on a read-only RPC, not the wallet
  example: false, // a published example request: shown, never sent
  vaultAck: false,
  // A deposit made on this page (not from a wallet's request). The secret lives here, in
  // memory only: never in the URL, storage or any request. `saved` gates the deposit.
  createdHere: false,
  draft: null, // { token, denomIndex, account, accountError, accountId, s, k, commitment, createdAt }
  saved: false,
  unclaimableAck: false, // the person accepted that a deposit to an account off the team's test chain can't be claimed yet
  walletRpc: null, // { words, raw }: the wallet's own Sepolia connection did not answer before a signature
  checks: [],
  data: null,
  anon: null,
  needsApproval: false,
  fee: null, // { gas, gasPrice, cost } network fee estimate
  tx: null,
  approveHash: null,
  finality: null,
  notice: null,
  error: null,
  busy: false,
});

export function createFlow({
  sleep = (ms) => new Promise((r) => setTimeout(r, ms)),
  now = () => Date.now(),
  receiptPollMs = 3000,
  finalityPollMs = 12000,
  logChunk = 10000n,
} = {}) {
  let state = initial();
  const listeners = new Set();
  let walletProvider = null;
  let readProvider = null;
  let gen = 0; // bumps whenever inputs change, so stale async results are dropped
  let detach = null;

  const set = (patch) => {
    state = { ...state, ...patch };
    for (const fn of listeners) fn(state);
  };
  const setCheck = (id, status, detail = '') => {
    set({ checks: state.checks.map((c) => (c.id === id ? { ...c, status, detail } : c)) });
  };

  const wallet = () => (walletProvider ? makeRpc(walletProvider) : null);
  // Reads go through the page's own pinned RPC (PublicNode) first and fall back to the wallet's
  // connection, or the other way round when the person picks it in Settings. Only accounts,
  // the chain id, switching networks and sending use the wallet alone.
  let readPrefer = 'page';
  const reader = () => {
    const w = walletProvider && state.account && state.walletChainId === state.request?.chainId ? { name: 'your wallet’s Sepolia connection', provider: walletProvider } : null;
    const p = readProvider ? { name: readProvider.label || 'the page’s Sepolia connection', provider: readProvider } : null;
    const list = (readPrefer === 'wallet' ? [w, p] : [p, w]).filter(Boolean);
    if (!list.length) return null;
    return makeRpc(list.length === 1 ? list[0].provider : fallbackProvider(list));
  };

  /** Before a signature: is the wallet's own Sepolia connection answering? A broken one would fail the send. */
  async function walletAnswers(w) {
    try {
      await Promise.race([w.request('eth_blockNumber'), new Promise((_, no) => setTimeout(() => no(Object.assign(new Error('timed out'), { transport: true })), 8000))]);
      return true;
    } catch (e) {
      if (isUserRejection(e)) return true;
      set({ step: 'ready', walletRpc: { words: failureWords(e), raw: String(e.message || e) } });
      return false;
    }
  }

  /** Offer the wallet a working Sepolia connection (EIP-3085). Wallets that already have Sepolia may ignore it. */
  async function addSepolia() {
    const w = wallet();
    if (!w) return;
    try {
      await w.request('wallet_addEthereumChain', [SEPOLIA_PARAMS]);
    } catch {
      /* declined or ignored: the manual instruction stays on screen */
    }
    if (await walletAnswers(w)) set({ walletRpc: null, notice: { kind: 'success', text: 'Your wallet’s Sepolia connection answers now.' } });
  }

  function setReadPrefer(p) {
    readPrefer = p === 'wallet' ? 'wallet' : 'page';
    if (!state.tx) refresh();
  }

  const fmt = (v) => (state.token ? `${formatUnits(v, state.token.decimals)} ${state.token.unit || state.token.symbol}` : String(v));

  // ------------------------------------------------------------- request

  function load(text) {
    gen++;
    const keep = { wallets: state.wallets, wallet: state.wallet, account: state.account, walletChainId: state.walletChainId };
    let request;
    try {
      request = parseRequestInput(text);
    } catch (e) {
      set({ ...initial(), ...keep, requestText: text, requestError: { code: e.code || 'invalid', message: e.message } });
      return false;
    }
    return useRequest(request, { text });
  }

  function useRequest(request, { text = '', createdHere = false, draft = null } = {}) {
    gen++;
    const keep = { wallets: state.wallets, wallet: state.wallet, account: state.account, walletChainId: state.walletChainId };
    // This build reads from and sends on Sepolia only. Any other chain, Ethereum mainnet
    // included, is refused here, before any read and whatever the vault: no acknowledgement
    // can unlock it (deployments.js, SUPPORTED_CHAIN_IDS).
    const unsupported = isSupportedChain(request.chainId) ? null : unsupportedChain(request.chainId);
    const retired = unsupported ? null : findRetired(request.chainId, request.vault);
    const deployment = unsupported ? null : findDeployment(request.chainId, request.vault);
    // A vault not on this page's list is refused before any read (DEV.allowUnlistedVaults is
    // the development-only way around it).
    const unlisted = !unsupported && !retired && !deployment && !DEV.allowUnlistedVaults
      ? { code: 'unlisted-vault', message: `This request is for a vault this page doesn’t know (${request.vault}). This page sends only to its own demo vault on Sepolia, so it refuses the request. Nothing was read or sent.` }
      : null;
    const known = deployment?.tokens[request.token];
    const isEth = request.token === ETH;
    set({
      ...initial(),
      ...keep,
      requestText: text,
      request,
      deployment,
      chain: chainInfo(request.chainId),
      token: {
        address: request.token,
        isEth,
        known: !!known,
        symbol: known?.symbol || (isEth ? 'ETH' : 'tokens'),
        unit: known?.unit || known?.symbol || (isEth ? `${chainInfo(request.chainId).name} ETH` : 'tokens'),
        name: known?.name || (isEth ? 'Ether' : 'Unknown token'),
        decimals: known ? known.decimals : isEth ? 18 : null,
        howToGet: known?.howToGet || null,
      },
      requestError: unsupported || unlisted || (retired ? { code: 'retired', message: retired.reason } : null),
      example: EXAMPLE_COMMITMENTS.has(request.commitment),
      checks: freshChecks(!isEth, !!deskOf(deployment)),
      createdHere,
      draft,
      saved: false,
    });
    if (unsupported || unlisted) {
      set({ step: 'blocked', error: unsupported || unlisted });
      return false;
    }
    if (retired) {
      set({ step: 'blocked', error: { code: 'retired', message: retired.reason } });
      return false;
    }
    refresh();
    return true;
  }

  // ------------------------------------------------------------- a deposit made here

  /** The deposit form. Any change of token, amount or account makes a new secret and needs a new save. */
  // Each setDraft call awaits hashing; a later call (the person kept typing) wins, and an earlier
  // one that finishes after it is dropped instead of overwriting the newer draft.
  let draftSeq = 0;
  async function setDraft({ token, denomIndex, account }) {
    const mine = ++draftSeq;
    const d = DEPLOYMENTS[0];
    const t = d.tokens[token];
    if (!t) throw new Error(`unknown token ${token}`);
    const amount = BigInt(t.denoms[denomIndex] ?? -1);
    const base = { token, denomIndex, account: account ?? '', accountError: null, accountId: null, accountLabel: '' };
    let dest;
    try {
      const a = parseLogosAccount(account);
      dest = a.bytes;
      base.accountId = a.id;
      base.accountLabel = a.label;
      base.teamHeld = isTestChainAccount(a.id);
      base.claimable = base.teamHeld || LOGOS_SIDE.selfClaim;
    } catch (e) {
      base.accountError = String(account ?? '').trim() ? { code: e.code, message: e.message } : null;
      gen++;
      set({ ...initial(), wallets: state.wallets, wallet: state.wallet, account: state.account, walletChainId: state.walletChainId, step: 'form', createdHere: true, draft: base });
      return false;
    }
    const same = state.draft && state.draft.token === token && state.draft.denomIndex === denomIndex && state.draft.accountId === base.accountId && state.draft.commitment;
    if (same) return true;
    const made = await makeDeposit({ dest, token, amount, cfgId: d.cfgId });
    if (mine !== draftSeq) return false;
    const draft = { ...base, ...made, createdAt: new Date(now()).toISOString() };
    const request = { chainId: BigInt(d.chainId), vault: d.vault, token, denomIndex, amount, commitment: made.commitment };
    return useRequest(request, { createdHere: true, draft });
  }

  /** The desk's `lezPublicKey` argument: always zero. The page never asks for a key; a public claim's
   * fee is paid by the relayer's co-signature (DESIGN.md §3.6), so the desk names no account. */
  const grantKey = () => ZERO32;

  /** The claim file for the deposit made here (after sending, it carries the transaction too). */
  function claimFile() {
    const s = state;
    if (!s.createdHere || !s.draft?.commitment) return null;
    const d = s.deployment;
    const t = d.tokens[s.request.token];
    const unit = t.unit || t.symbol;
    return buildClaimFile({
      chainId: s.request.chainId,
      network: s.chain.name,
      vault: d.vault,
      cfgId: d.cfgId,
      token: s.request.token,
      symbol: t.symbol,
      decimals: t.decimals,
      denomIndex: s.request.denomIndex,
      amount: s.request.amount,
      amountText: `${formatUnits(s.request.amount, t.decimals)} ${unit}`,
      account: s.draft.accountId,
      s: s.draft.s,
      k: s.draft.k,
      commitment: s.draft.commitment,
      createdAt: s.draft.createdAt,
      deposit: s.tx?.blockNumber ? { tx: s.tx.hash, block: s.tx.blockNumber, leafIndex: s.tx.leafIndex } : null,
      bridgeFee: s.data?.desk ? { desk: s.data.desk, feeBps: s.data.feeBps, amount: s.data.bridgeFee } : null,
    });
  }

  function ackUnclaimable(yes) {
    set({ unclaimableAck: !!yes });
  }

  /** A deposit made here to an account the team cannot claim for needs the person's explicit yes. */
  // Any account the team does not hold needs one explicit "This is my account" first: an id has
  // no checksum, so a typo or a pasted key would make the deposit unclaimable (review C).
  const unclaimableGate = () => (state.createdHere && !state.draft?.teamHeld && !state.unclaimableAck);

  function markSaved(yes) {
    set({ saved: !!yes && !!state.draft?.commitment });
  }

  function unsupportedChain(chainId) {
    const name = chainInfo(chainId).name;
    return {
      code: 'unsupported-chain',
      message: `This request is for ${name} (chain id ${chainId}). This page sends only on Sepolia (chain id 11155111), so nothing was read or sent.`,
    };
  }

  function clear() {
    gen++;
    set({ ...initial(), wallets: state.wallets, wallet: state.wallet, account: state.account, walletChainId: state.walletChainId });
  }

  // ------------------------------------------------------------- wallets

  function setWallets(list) {
    set({ wallets: list });
  }

  async function connect(walletId, { silent = false } = {}) {
    const w = state.wallets.find((x) => x.id === walletId) || state.wallets[0];
    if (!w) {
      set({ notice: { kind: 'error', text: 'No browser wallet found. Open this page in your wallet’s built-in browser, or install a wallet extension.' } });
      return;
    }
    if (detach) detach();
    walletProvider = w.provider;
    set({ wallet: { id: w.id, name: w.name, icon: w.icon }, notice: null, busy: true });
    try {
      const r = wallet();
      const [accounts, chainId] = [silent ? await r.accounts() : await r.requestAccounts(), await r.chainId()];
      if (!accounts.length) {
        walletProvider = null;
        set({ wallet: null, account: null, busy: false, notice: silent ? null : { kind: 'error', text: `${w.name} shared no account. Unlock it and try again.` } });
        return;
      }
      set({ account: accounts[0] || null, walletChainId: chainId, busy: false });
    } catch (e) {
      walletProvider = null;
      set({
        wallet: null,
        account: null,
        busy: false,
        notice: { kind: isUserRejection(e) ? 'info' : 'error', text: isUserRejection(e) ? 'You declined the connection in your wallet.' : `Your wallet did not connect: ${e.message || e}` },
      });
      return;
    }
    const p = walletProvider;
    const onAccounts = (accs) => {
      if (!accs || !accs[0]) return disconnect();
      set({ account: String(accs[0]).toLowerCase() });
      if (!state.tx) refresh();
    };
    const onChain = (cid) => {
      set({ walletChainId: BigInt(cid) });
      if (!state.tx) refresh();
    };
    const onDisconnect = () => disconnect();
    p.on?.('accountsChanged', onAccounts);
    p.on?.('chainChanged', onChain);
    p.on?.('disconnect', onDisconnect);
    detach = () => {
      p.removeListener?.('accountsChanged', onAccounts);
      p.removeListener?.('chainChanged', onChain);
      p.removeListener?.('disconnect', onDisconnect);
    };
    refresh();
  }

  /** Reconnect without a prompt if the wallet already shares an account with this page. */
  async function restore(walletId) {
    if (walletProvider) return false;
    const w = state.wallets.find((x) => x.id === walletId);
    if (!w) return false;
    let accounts = [];
    try {
      accounts = await makeRpc(w.provider).accounts();
    } catch {
      return false;
    }
    if (!accounts.length || walletProvider) return false;
    await connect(walletId, { silent: true });
    return true;
  }

  function disconnect() {
    if (detach) detach();
    detach = null;
    walletProvider = null;
    gen++;
    set({ wallet: null, account: null, walletChainId: null });
    if (!state.tx) refresh();
  }

  function setReadProvider(p) {
    readProvider = p;
    if (!state.tx) refresh();
  }

  async function switchNetwork() {
    const r = wallet();
    // The request's chain, or Sepolia when the deposit is being made here and has no request yet.
    const target = state.request ? state.request.chainId : 11155111n;
    if (!r || !isSupportedChain(target)) return;
    const hexId = '0x' + target.toString(16);
    const netName = chainInfo(target).name;
    set({ busy: true, notice: null });
    try {
      await r.request('wallet_switchEthereumChain', [{ chainId: hexId }]);
      set({ walletChainId: await r.chainId(), busy: false });
      refresh();
    } catch (e) {
      set({ busy: false });
      if (isUserRejection(e)) {
        set({ notice: { kind: 'info', text: `You declined the switch. Switch your wallet to ${netName} to continue.` } });
      } else if (e.code === 4902 || e?.data?.originalError?.code === 4902) {
        // The wallet does not know Sepolia: offer to add it (EIP-3085), which also switches.
        try {
          await r.request('wallet_addEthereumChain', [SEPOLIA_PARAMS]);
          set({ walletChainId: await r.chainId(), busy: false });
          refresh();
        } catch (e2) {
          set({ notice: { kind: isUserRejection(e2) ? 'info' : 'error', text: isUserRejection(e2) ? `You declined adding ${netName}. Add it in your wallet to continue.` : `Your wallet could not add ${netName}: ${e2.message || e2}` } });
        }
      } else {
        set({ notice: { kind: 'error', text: `Your wallet could not switch networks: ${e.message || e}. Switch to ${netName} in the wallet itself.` } });
      }
    }
  }

  function acknowledgeVault(yes) {
    set({ vaultAck: !!yes });
    if (!state.tx) refresh();
  }

  // ------------------------------------------------------------- checks

  function refresh() {
    if (!state.request || state.tx) return;
    if (state.requestError) return;
    const myGen = ++gen;
    if (!state.account || !walletProvider) {
      if (readProvider) {
        set({ step: 'connect', readOnly: true });
        runChecks(myGen, { readOnly: true });
      } else {
        set({ step: 'connect', readOnly: false, checks: freshChecks(!state.token.isEth, !!deskOf(state.deployment)), error: null });
      }
      return;
    }
    if (state.walletChainId !== state.request.chainId) {
      set({ step: 'network', readOnly: false, error: null, checks: freshChecks(!state.token.isEth, !!deskOf(state.deployment)) });
      return;
    }
    set({ readOnly: false });
    runChecks(myGen, { readOnly: false });
  }

  async function runChecks(myGen, { readOnly }) {
    const r = reader();
    if (!r) return;
    const req = state.request;
    const live = () => myGen === gen;
    const fail = (id, detail, code, message) => {
      setCheck(id, 'fail', detail);
      set({ step: readOnly ? 'connect' : 'blocked', error: { code, message: message || detail }, busy: false });
    };
    set({ checks: freshChecks(!state.token.isEth, !!deskOf(state.deployment)), error: null, busy: true, ...(readOnly ? {} : { step: 'checking' }) });

    try {
      if (readOnly) {
        const cid = await r.chainId();
        if (!live()) return;
        if (cid !== req.chainId) {
          set({ busy: false, notice: { kind: 'error', text: `The read-only RPC is on chain ${cid}, but the request is for ${state.chain.name}. Pick another RPC in Settings.` } });
          return;
        }
      }

      // vault: allowlist
      if (state.deployment) setCheck('vault', 'pass', state.deployment.label);
      else if (state.vaultAck) setCheck('vault', 'warn', 'Not on this page’s list. You confirmed you checked the address yourself.');
      else {
        setCheck('vault', 'warn', 'Not on this page’s list of known vaults.');
      }

      // code
      const code = await r.code(req.vault);
      if (!live()) return;
      if (!code || code === '0x') return fail('code', `There is no contract at ${req.vault} on ${state.chain.name}.`, 'no-code', `There is no contract at this vault address on ${state.chain.name}. The request is wrong or for another network; do not send anything.`);
      if (state.createdHere) {
        // The commitment was made here with the allowlisted cfgId: the vault must hold the same.
        const cfg = String(await r.call(req.vault, call.cfgId())).toLowerCase();
        if (!live()) return;
        if (cfg !== state.deployment.cfgId) return fail('code', 'The vault’s configuration differs from the one this page knows.', 'cfg-mismatch', 'This vault’s configuration is not the one the page made your deposit for. Nothing was sent. Reload the page.');
      }
      setCheck('code', 'pass');

      // token config
      const cfg = decode.tokenConfig(await r.call(req.vault, call.tokenConfig(req.token)));
      if (!live()) return;
      if (!cfg.registered) return fail('token', 'The vault does not accept this token.', 'not-registered', 'This vault does not accept this token (it was never registered, or it was delisted). Ask your Names Wallet for a request for a supported token.');
      let token = state.token;
      if (!token.known) {
        let symbol = token.symbol;
        if (!token.isEth) {
          try {
            symbol = decode.string(await r.call(req.token, call.symbol())).slice(0, 16) || 'tokens';
          } catch {
            /* keep fallback */
          }
        }
        // Native ETH has 18 decimals, whatever a vault's config says: never take display units
        // for ETH from a vault (review C: a fake vault reporting 24 hid a 1 ETH deposit as 0.000001).
        if (token.isEth && cfg.decimals !== 18) return fail('token', `The vault records ${cfg.decimals} decimals for ETH, which has 18.`, 'decimals', 'This vault describes ETH wrongly, so the amounts it implies can’t be trusted. Nothing was sent; do not deposit.');
        token = { ...token, symbol, unit: token.isEth ? token.unit : symbol, decimals: token.isEth ? 18 : cfg.decimals };
      } else if (token.decimals !== cfg.decimals) {
        return fail('token', `The vault records ${cfg.decimals} decimals for this token, not ${token.decimals}.`, 'decimals', 'The vault and this page disagree about the token’s decimals. Do not deposit; report this to the operator.');
      }
      set({ token });
      setCheck('token', 'pass', `${token.unit}${token.known || token.isEth ? '' : ' (read from the token contract)'}`);

      // amount
      if (req.denomIndex >= cfg.denoms.length) {
        return fail('amount', `The vault has ${cfg.denoms.length} denominations for ${token.unit}; the request names number ${req.denomIndex + 1}.`, 'bad-denomination', 'The request names a denomination this vault does not have. Make a new deposit code in your Names Wallet.');
      }
      const denom = cfg.denoms[req.denomIndex];
      if (denom !== req.amount) {
        return fail(
          'amount',
          `The vault’s denomination is ${fmt(denom)}, the request says ${fmt(req.amount)}.`,
          'amount-mismatch',
          `The request says ${fmt(req.amount)} but the vault’s denomination at that position is ${fmt(denom)}. The commitment was made for the request’s amount, so a deposit would be locked forever. Do not deposit; make a new request in your Names Wallet.`,
        );
      }
      setCheck('amount', 'pass', `${fmt(denom)} (denomination ${req.denomIndex + 1} of ${cfg.denoms.length})`);
      const fee = cfg.depositFees[req.denomIndex];
      let total = denom + fee;

      // deposit desk (the bridge fee): on for every deposit once deployments.js names it
      const desk = deskOf(state.deployment);
      let bridgeFee = 0n;
      let feeBps = null;
      if (desk) {
        const dcode = await r.code(desk);
        if (!live()) return;
        if (!dcode || dcode === '0x') return fail('desk', 'There is no deposit desk contract at its address.', 'no-desk', 'The deposit desk this page sends through is not on Sepolia at its address. Nothing was sent. Tell the bridge team.');
        const dv = decode.address(await r.call(desk, call.deskVault())).toLowerCase();
        if (!live()) return;
        if (dv !== req.vault) return fail('desk', 'The deposit desk forwards to another vault.', 'desk-vault', 'The deposit desk this page knows forwards to a different vault than this deposit is for. Nothing was sent. Tell the bridge team.');
        const q = decode.deskQuote(await r.call(desk, call.deskQuote(req.token, req.denomIndex)));
        feeBps = decode.uint(await r.call(desk, call.deskFeeBps()));
        if (!live()) return;
        if (q.denom !== denom || q.vaultFee !== fee || q.total !== denom + fee + q.bridgeFee || q.bridgeFee * 10000n > denom * feeBps) {
          return fail('desk', 'The deposit desk quotes a different amount than the vault.', 'desk-quote', 'The deposit desk and the vault disagree about this deposit’s amount. Nothing was sent. Tell the bridge team.');
        }
        bridgeFee = q.bridgeFee;
        total = q.total;
        setCheck('desk', 'pass', `Bridge fee ${fmt(bridgeFee)} (${bpsText(feeBps)} of ${fmt(denom)}); you send ${fmt(total)} in all.`);
      }

      // frozen
      const gFrozen = decode.bool(await r.call(req.vault, call.globalDepositsFrozen()));
      if (!live()) return;
      if (gFrozen || cfg.depositsFrozen) {
        return fail('open', gFrozen ? 'The whole vault is paused for deposits.' : `Deposits of ${token.unit} are paused.`, 'frozen', 'Deposits are paused right now (the vault’s freeze switch is on). Nothing has been sent. Try again later.');
      }
      setCheck('open', 'pass');

      // caps
      const cap = decode.capState(await r.call(req.vault, call.capState(req.token)));
      const gcap = decode.capState(await r.call(req.vault, call.globalCapState()));
      const weight = decode.uint(await r.call(req.vault, call.capWeight(req.token)));
      if (!live()) return;
      let capDetail = 'No deposit limit is set.';
      if (cap.window !== 0n) {
        const room = cap.depositCap > cap.depUsed ? cap.depositCap - cap.depUsed : 0n;
        if (denom > cap.depositCap || cap.depUsed > cap.depositCap - denom) {
          return fail('cap', `Room left in this period: ${fmt(room)}.`, 'cap-exceeded', `The vault has taken close to its ${token.unit} limit for this period (room left: ${fmt(room)}). The limit frees up gradually over ${windowText(cap.window)}. Nothing has been sent; try again later.`);
        }
        capDetail = `${fmt(room)} of ${fmt(cap.depositCap)} left in the current ${windowText(cap.window)} window.`;
      }
      if (gcap.window !== 0n && weight !== 0n) {
        const g = (denom * weight) / 10n ** 18n;
        if (g > gcap.depositCap || gcap.depUsed > gcap.depositCap - g) {
          return fail('cap', 'The vault-wide limit for this period is reached.', 'cap-exceeded', 'The vault has reached its overall deposit limit for this period. It frees up gradually; nothing has been sent. Try again later.');
        }
      }
      setCheck('cap', 'pass', capDetail);

      // anonymity set
      const counter = decode.counters(await r.call(req.vault, call.counters(req.token, req.denomIndex)));
      const totalDeposits = decode.uint(await r.call(req.vault, call.depositCount()));
      if (!live()) return;
      set({ anon: { same: counter.deposits, total: totalDeposits } });

      // commitment
      const used = decode.bool(await r.call(req.vault, call.commitmentUsed(req.commitment)));
      if (!live()) return;
      if (used) {
        setCheck('commitment', 'fail', 'Already in the vault.');
        await recoverExisting(myGen, r, readOnly);
        return;
      }
      setCheck('commitment', 'pass');

      const data = { cfg, denom, fee, total, bridgeFee, feeBps, desk, cap, gcap, weight };
      set({ data });

      if (readOnly || !state.account) {
        setCheck('balance', 'skip', 'Connect a wallet to check.');
        if (!token.isEth) setCheck('allowance', 'skip', 'Connect a wallet to check.');
        setCheck('simulate', 'skip', 'Connect a wallet to check.');
        set({ busy: false, step: 'connect' });
        return;
      }

      // balances
      const me = state.account;
      const ethBal = await r.balance(me);
      let gasPrice = 0n;
      try {
        gasPrice = await r.gasPrice();
      } catch {
        /* estimate unavailable */
      }
      if (!live()) return;
      if (token.isEth) {
        if (ethBal < total) {
          return fail('balance', `You have ${fmt(ethBal)}; the deposit${desk ? ' with its bridge fee' : ''} is ${fmt(total)} plus the network fee.`, 'insufficient-balance', `This address holds ${fmt(ethBal)}, less than the ${fmt(total)} deposit${desk ? ' with its bridge fee' : ''}. ${state.chain.testnet ? 'Get Sepolia ETH from a faucet, or switch to an address that has some.' : 'Add funds or switch accounts.'}`);
        }
        setCheck('balance', 'pass', `You have ${formatUnits(ethBal, 18, { maxFraction: 4 })} ${state.chain.name} ETH.`);
      } else {
        const tBal = decode.uint(await r.call(req.token, call.balanceOf(me)));
        if (!live()) return;
        if (tBal < total) {
          return fail('balance', `You have ${fmt(tBal)}; the deposit${desk ? ' with its bridge fee' : ''} is ${fmt(total)}.`, 'insufficient-balance', `This address holds ${fmt(tBal)}, less than the ${fmt(total)} deposit${desk ? ' with its bridge fee' : ''}.${token.howToGet ? ' ' + token.howToGet : ''}`);
        }
        if (ethBal === 0n) {
          return fail('balance', `You have no ${state.chain.name} ETH to pay the network fee.`, 'no-gas', `This address has no ${state.chain.name} ETH to pay the network fee. Add a little first.`);
        }
        setCheck('balance', 'pass', `You have ${fmt(tBal)}.`);
        const spender = desk || req.vault;
        const allowance = decode.uint(await r.call(req.token, call.allowance(me, spender)));
        if (!live()) return;
        // exactly the quoted total: a larger standing approval would let a fee raised in the
        // same block take more than the page showed (review C), so it is replaced, not reused
        if (allowance !== total) {
          setCheck('allowance', 'action', allowance > total ? `Approval needed for exactly ${fmt(total)} (the current one is larger).` : `Approval needed for exactly ${fmt(total)}.`);
          set({ data: { ...data, allowance, ethBal }, needsApproval: true });
          await estimate(myGen, r, { to: req.token, data: call.approve(spender, total), from: me }, gasPrice);
          if (!live()) return;
          setCheck('simulate', 'pending', 'Runs after the approval.');
          set({ step: 'ready', busy: false });
          return;
        }
        setCheck('allowance', 'pass', `Approved for exactly ${fmt(allowance)}.`);
        set({ data: { ...data, allowance, ethBal }, needsApproval: false });
      }

      // final gate: simulate the exact deposit from this address
      const tx = depositTx();
      try {
        const ret = await r.call(tx.to, tx.data, { from: me, value: tx.valueBig });
        if (!isOneWord(ret)) return fail('simulate', 'The vault gave an unexpected answer.', 'bad-simulation', 'The vault did not answer the simulated deposit as a vault would. Nothing was sent.');
      } catch (e) {
        if (!live()) return;
        const d = decodeRevert(extractRevertData(e));
        return fail('simulate', d.message, d.code, d.message);
      }
      if (!live()) return;
      setCheck('simulate', 'pass', 'The vault would accept this deposit right now.');
      await estimate(myGen, r, { to: tx.to, data: tx.data, from: me, value: tx.value }, gasPrice);
      if (!live()) return;
      set({ data: { ...state.data, ethBal }, step: 'ready', busy: false, needsApproval: false });
    } catch (e) {
      if (!live()) return;
      set({
        busy: false,
        step: readOnly ? 'connect' : 'blocked',
        error: { code: 'read-failed', message: e.plain ? `Could not read Sepolia: ${e.plain} did not answer. Try again in a minute.` : 'Could not read Sepolia. Try again in a minute.', raw: e.raw || String(e.message || e) },
      });
    }
  }

  async function estimate(myGen, r, tx, gasPrice) {
    try {
      const t = { ...tx };
      if (!t.value) delete t.value;
      const gas = await r.estimateGas(t);
      if (myGen !== gen) return;
      set({ fee: { gas, gasPrice, cost: gas * gasPrice, for: tx.to === state.request.token && !state.token.isEth ? 'approve' : 'deposit' } });
    } catch {
      set({ fee: null });
    }
  }

  function depositTx() {
    const req = state.request;
    const total = state.data.total;
    const desk = state.data.desk;
    if (desk) {
      // one transaction through the desk: the vault's amount plus the bridge fee, the same commitment
      if (state.token.isEth) return { to: desk, data: call.depositETHWithFee(req.denomIndex, req.commitment, grantKey()), value: '0x' + total.toString(16), valueBig: total };
      // a desk with the cap is told the total the person was shown: it reverts (WrongAmount) above it
      if (deskTakesMaxTotal(state.deployment)) return { to: desk, data: call.depositWithFeeMax(req.token, req.denomIndex, req.commitment, grantKey(), total), value: undefined, valueBig: 0n };
      return { to: desk, data: call.depositWithFee(req.token, req.denomIndex, req.commitment, grantKey()), value: undefined, valueBig: 0n };
    }
    if (state.token.isEth) {
      return { to: req.vault, data: call.depositETH(req.denomIndex, req.commitment), value: '0x' + total.toString(16), valueBig: total };
    }
    return { to: req.vault, data: call.deposit(req.token, req.denomIndex, req.commitment), value: undefined, valueBig: 0n };
  }

  // A used commitment is either this person's own earlier deposit (resume tracking it) or
  // someone else's use of the same commitment. Find the event to tell which.
  async function recoverExisting(myGen, r, readOnly) {
    const req = state.request;
    const found = await findDepositEvent(r, req);
    if (myGen !== gen) return;
    // A receipt can be missing while its log is served (a pruning node answers some requests
    // from a full backend and others not): then the generic message below, never a crash.
    if (found && found.tx && found.receipt) {
      const ev = found.event;
      const same = ev.token === req.token && ev.denomIndex === req.denomIndex;
      const mine = state.account && found.tx.from?.toLowerCase() === state.account;
      if (same && !readOnly) {
        set({ busy: false, notice: { kind: 'info', text: mine ? 'This request was already deposited from this address. Showing that deposit.' : 'This commitment was already deposited, from another address. Your Names Wallet can claim that deposit if it holds the secret; there is nothing to send.' } });
        await trackMined(myGen, r, found.receipt, { recovered: true, mine });
        return;
      }
      if (!same) {
        set({ step: readOnly ? 'connect' : 'blocked', busy: false, error: { code: 'commitment-used', message: 'This commitment is already in the vault for a different token or amount, so it cannot be used again. Make a new deposit code in your Names Wallet.' } });
        return;
      }
    }
    set({
      step: readOnly ? 'connect' : 'blocked',
      busy: false,
      error: { code: 'commitment-used', message: 'This commitment has already been deposited. A commitment can only be used once, so there is nothing to send. If that deposit was yours, your Names Wallet can claim it.' },
    });
  }

  async function findDepositEvent(r, req) {
    const tokenTopic = '0x' + req.token.slice(2).padStart(64, '0');
    let head;
    try {
      head = await r.blockNumber();
    } catch {
      return null;
    }
    const from = state.deployment ? BigInt(state.deployment.deployBlock) : head > logChunk * 5n ? head - logChunk * 5n : 0n;
    let log;
    try {
      ({ value: log } = await scanLogs(r, { address: req.vault, topics: [DEPOSIT_TOPIC, tokenTopic] }, from, head, {
        found: (logs) => logs.find((l) => decodeDepositLog(l)?.commitment === req.commitment.toLowerCase()),
      }));
    } catch {
      return null;
    }
    if (!log) return null;
    const [tx, receipt] = [await r.transaction(log.transactionHash), await r.receipt(log.transactionHash)];
    return { event: decodeDepositLog(log), tx, receipt };
  }

  // ------------------------------------------------------------- actions

  async function approve() {
    if (state.step !== 'ready' || !state.needsApproval || state.example) return;
    if (!isSupportedChain(state.request?.chainId)) return;
    if (unclaimableGate()) return;
    if (!state.deployment && !state.vaultAck) return;
    const myGen = gen;
    const w = wallet();
    const r = { ...(reader() || w), send: w.send };
    const req = state.request;
    const me = state.account;
    const total = state.data.total;
    const spender = state.data.desk || req.vault;
    if (!(await walletAnswers(w))) return;
    set({ step: 'approve-sign', notice: null, error: null, walletRpc: null });
    if (!(await sameChain(w))) return;
    try {
      // Some tokens (USDT) refuse to change a non-zero allowance to another non-zero
      // value. Simulate first; if refused and an allowance exists, reset it to 0.
      if (state.data.allowance > 0n) {
        try {
          await r.call(req.token, call.approve(spender, total), { from: me });
        } catch {
          const h0 = await r.send({ from: me, to: req.token, data: call.approve(spender, 0n), chainId: hexChain(req.chainId) });
          set({ step: 'approve-wait', tx: null, approveHash: h0 });
          await waitReceipt(r, h0);
          set({ step: 'approve-sign' });
        }
      }
      const hash = await r.send({ from: me, to: req.token, data: call.approve(spender, total), chainId: hexChain(req.chainId) });
      set({ step: 'approve-wait', approveHash: hash });
      const receipt = await waitReceipt(r, hash);
      if (receipt.status !== '0x1') {
        set({ step: 'blocked', error: { code: 'approve-failed', message: 'The approval transaction failed on chain. Nothing was deposited. Try again.' } });
        return;
      }
      setCheck('allowance', 'pass', `Approved for exactly ${fmt(total)}.`);
      set({ needsApproval: false, notice: { kind: 'success', text: `Approved. The ${state.data.desk ? 'deposit desk' : 'vault'} may now take exactly ${fmt(total)}, once.` } });
      if (myGen === gen) refresh();
    } catch (e) {
      if (isUserRejection(e)) {
        set({ step: 'ready', notice: { kind: 'info', text: 'You declined the approval in your wallet. Nothing was sent.' } });
      } else if (isTransportError(e) && !extractRevertData(e)) {
        set({ step: 'ready', walletRpc: { words: failureWords(e), raw: String(e.message || e) } });
      } else {
        set({ step: 'ready', notice: { kind: 'error', text: `The approval did not go through: ${e.message || e}` } });
      }
    }
  }

  // The wallet may have changed network without telling the page (no chainChanged
  // event). Re-read it before every signature request; a mismatch goes back to `network`.
  async function sameChain(r) {
    let cid;
    try {
      cid = await r.chainId();
    } catch {
      cid = null;
    }
    if (cid === state.request.chainId && isSupportedChain(cid)) return true;
    set({ walletChainId: cid, step: 'network', notice: { kind: 'error', text: `Your wallet is no longer on ${state.chain.name}. Nothing was sent.` } });
    return false;
  }

  async function deposit() {
    if (state.step !== 'ready' || state.needsApproval || state.example) return;
    if (!isSupportedChain(state.request?.chainId)) return;
    if (state.createdHere && !state.saved) return;
    if (unclaimableGate()) return;
    if (!state.deployment && !state.vaultAck) return;
    const w = wallet();
    const r = reader() || w;
    const me = state.account;
    const tx = depositTx();
    if (!(await walletAnswers(w))) return;
    set({ step: 'deposit-sign', notice: null, error: null, walletRpc: null });
    if (!(await sameChain(w))) return;
    // The total must still be the one the person saw: re-read the desk's quote right before the
    // signature request. The approval was for exactly that total, and an ETH deposit carries it
    // as its value, so a fee raised since the checks stops here, not in the wallet (review C).
    if (state.data.desk) {
      try {
        const q = decode.deskQuote(await r.call(state.data.desk, call.deskQuote(state.request.token, state.request.denomIndex)));
        if (q.total !== state.data.total) {
          set({ step: 'blocked', error: { code: 'quote-changed', message: `The bridge fee changed since the checks: the total is now ${fmt(q.total)}, not ${fmt(state.data.total)}. Nothing was sent. Check again to see the new total.` } });
          return;
        }
      } catch (e) {
        set({ step: 'blocked', error: { code: 'read-failed', message: 'Could not re-read the bridge fee before signing. Nothing was sent. Try again in a minute.', raw: String(e.message || e) } });
        return;
      }
    }
    // final gate, immediately before the signature request: the vault must answer with
    // exactly one word (the new leaf index). A codeless address answers 0x and is refused.
    try {
      const ret = await r.call(tx.to, tx.data, { from: me, value: tx.valueBig });
      if (!isOneWord(ret)) {
        set({ step: 'blocked', error: { code: 'bad-simulation', message: 'The vault did not answer the simulated deposit as a vault would. Nothing was sent. Check that your wallet is on the right network, then check again.' } });
        return;
      }
    } catch (e) {
      const d = decodeRevert(extractRevertData(e));
      set({ step: 'blocked', error: { code: d.code, message: d.message } });
      setCheck('simulate', 'fail', d.message);
      return;
    }
    let hash;
    try {
      const send = { from: me, to: tx.to, data: tx.data, chainId: hexChain(state.request.chainId) };
      if (tx.value) send.value = tx.value;
      hash = await w.send(send);
    } catch (e) {
      if (isUserRejection(e)) {
        set({ step: 'ready', notice: { kind: 'info', text: 'You declined the deposit in your wallet. Nothing was sent.' } });
      } else if (isTransportError(e) && !extractRevertData(e)) {
        // the wallet's own Sepolia connection failed while preparing the transaction
        set({ step: 'ready', walletRpc: { words: failureWords(e), raw: String(e.message || e) } });
      } else {
        const data = extractRevertData(e);
        const d = data ? decodeRevert(data) : null;
        set({ step: 'ready', notice: { kind: 'error', text: d ? d.message : `Your wallet did not send the deposit: ${e.message || e}` } });
      }
      return;
    }
    gen++;
    const myGen = gen;
    set({ step: 'deposit-wait', tx: { hash, sentAt: now() } });
    const receipt = await waitReceipt(r, hash, myGen);
    if (!receipt || myGen !== gen) return;
    if (receipt.status !== '0x1') {
      let reason = 'The deposit transaction failed on chain. Your funds stayed in your wallet; only the network fee was spent.';
      try {
        await r.call(tx.to, tx.data, { from: me, value: tx.valueBig });
      } catch (e) {
        const d = decodeRevert(extractRevertData(e));
        if (d.code !== 'unknown') reason += ` Most likely reason: ${d.message}`;
      }
      set({ step: 'failed', tx: { ...state.tx, receipt }, error: { code: 'reverted', message: reason } });
      return;
    }
    await trackMined(myGen, r, receipt, { recovered: false, mine: true });
  }

  async function waitReceipt(r, hash, myGen) {
    const start = now();
    for (;;) {
      if (myGen !== undefined && myGen !== gen) return null;
      let receipt = null;
      try {
        receipt = await r.receipt(hash);
      } catch {
        /* transient */
      }
      if (receipt && receipt.blockNumber) return receipt;
      if (now() - start > 10 * 60 * 1000 && !state.notice) {
        set({ notice: { kind: 'info', text: 'Still waiting for the transaction to be mined. If your wallet shows it as dropped or replaced, check it there.' } });
      }
      await sleep(receiptPollMs);
    }
  }

  async function trackMined(myGen, r, receipt, { recovered, mine }) {
    const req = state.request;
    const ev = (receipt.logs || [])
      .filter((l) => l.address?.toLowerCase() === req.vault)
      .map(decodeDepositLog)
      .find((d) => d && d.commitment === req.commitment.toLowerCase());
    if (!ev) {
      set({ step: 'failed', error: { code: 'no-event', message: 'The transaction was mined but the vault recorded no deposit for this commitment. Check the transaction in a block explorer before trying again.' } });
      return;
    }
    let timestamp = null;
    try {
      const b = await r.block(BigInt(receipt.blockNumber));
      timestamp = b ? Number(BigInt(b.timestamp)) : null;
    } catch {
      /* optional */
    }
    let anon = state.anon;
    try {
      const counter = decode.counters(await r.call(req.vault, call.counters(req.token, req.denomIndex)));
      anon = { ...(anon || {}), same: counter.deposits, total: decode.uint(await r.call(req.vault, call.depositCount())) };
    } catch {
      /* keep */
    }
    set({
      anon,
      step: 'finalizing',
      tx: {
        ...(state.tx || {}),
        hash: receipt.transactionHash,
        blockNumber: BigInt(receipt.blockNumber),
        blockHash: receipt.blockHash,
        leafIndex: ev.leafIndex,
        feePaid: receipt.gasUsed && receipt.effectiveGasPrice ? BigInt(receipt.gasUsed) * BigInt(receipt.effectiveGasPrice) : null,
        root: ev.root,
        timestamp,
        recovered,
        mine,
      },
      finality: { mode: 'tag', finalized: null, startFinalized: null, done: false },
      busy: false,
    });
    await trackFinality(myGen, r);
  }

  async function trackFinality(myGen, r) {
    let misses = 0; // consecutive failed `finalized` reads
    for (;;) {
      if (myGen !== gen) return;
      const target = state.tx.blockNumber;
      let fin = null;
      try {
        const b = await r.block('finalized');
        fin = b ? BigInt(b.number) : null;
        misses = 0;
      } catch {
        misses++;
      }
      if (myGen !== gen) return;
      if (fin === null && (misses >= 3 || state.finality.mode === 'estimate')) {
        // The wallet's node does not report finality: estimate from time, honestly labelled.
        const since = state.tx.timestamp ? now() / 1000 - state.tx.timestamp : 0;
        const done = since > FINALITY_LATE_SECONDS;
        set({ finality: { ...state.finality, mode: 'estimate', done, elapsed: since } });
        if (done) {
          set({ step: 'done' });
          return;
        }
      } else if (fin !== null) {
        const start = state.finality.startFinalized ?? fin;
        if (fin >= target) {
          // confirm the block we were mined in is the canonical one at that height
          let b = null;
          try {
            b = await r.block(target);
          } catch {
            await sleep(finalityPollMs);
            continue; // transient: try again next poll
          }
          if (b && b.hash && b.hash.toLowerCase() !== String(state.tx.blockHash).toLowerCase()) {
            let receipt = null;
            try {
              receipt = await r.receipt(state.tx.hash);
            } catch {
              await sleep(finalityPollMs);
              continue;
            }
            if (receipt && receipt.blockNumber && receipt.status === '0x1') {
              set({ tx: { ...state.tx, blockNumber: BigInt(receipt.blockNumber), blockHash: receipt.blockHash } });
              continue;
            }
            set({ step: 'failed', error: { code: 'reorged', message: 'The block your deposit was in was replaced, and the deposit is not in the final chain. Check your wallet’s activity; you may need to deposit again with a new request.' } });
            return;
          }
          set({ step: 'done', finality: { ...state.finality, mode: 'tag', finalized: fin, startFinalized: start, done: true } });
          return;
        }
        set({ finality: { ...state.finality, mode: 'tag', finalized: fin, startFinalized: start, done: false } });
      }
      await sleep(finalityPollMs);
    }
  }

  function dismissNotice() {
    set({ notice: null });
  }

  function retry() {
    set({ error: null, notice: null });
    if (state.step === 'failed') {
      // a reverted deposit: back to checks, which will show the reason again if it persists
      set({ tx: null, finality: null });
    }
    refresh();
  }

  return {
    get state() {
      return state;
    },
    subscribe(fn) {
      listeners.add(fn);
      fn(state);
      return () => listeners.delete(fn);
    },
    load,
    useRequest,
    setDraft,
    claimFile,
    markSaved,
    ackUnclaimable,
    clear,
    setWallets,
    connect,
    restore,
    disconnect,
    setReadProvider,
    setReadPrefer,
    addSepolia,
    switchNetwork,
    acknowledgeVault,
    approve,
    deposit,
    retry,
    refresh,
    dismissNotice,
  };
}

const SEPOLIA_PARAMS = {
  chainId: '0xaa36a7',
  chainName: 'Sepolia',
  nativeCurrency: { name: 'Sepolia Ether', symbol: 'ETH', decimals: 18 },
  rpcUrls: ['https://ethereum-sepolia-rpc.publicnode.com'],
  blockExplorerUrls: ['https://sepolia.etherscan.io'],
};

const hexChain = (id) => '0x' + BigInt(id).toString(16);

function isOneWord(ret) {
  return typeof ret === 'string' && /^0x[0-9a-fA-F]{64}$/.test(ret);
}

/** Basis points as a percentage for people: 300 -> "3%", 250 -> "2.5%". */
export function bpsText(bps) {
  const b = BigInt(bps);
  const whole = b / 100n;
  const frac = (b % 100n).toString().padStart(2, '0').replace(/0+$/, '');
  return `${whole}${frac ? '.' + frac : ''}%`;
}

function windowText(seconds) {
  const s = Number(seconds);
  if (s % 86400 === 0) return s === 86400 ? '24-hour' : `${s / 86400}-day`;
  if (s % 3600 === 0) return `${s / 3600}-hour`;
  return `${Math.round(s / 60)}-minute`;
}
