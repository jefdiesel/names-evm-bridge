#!/usr/bin/env python3
"""Fetch real light-client + storage-proof fixtures for the attest-spike.

Read-only public HTTP GET/POST only, generic user agent, no credentials.

  python3 -I scripts/fetch_fixtures.py sepolia   # Gloas: LC data + EL header + eth_getProof
  python3 -I scripts/fetch_fixtures.py mainnet   # Fulu:  LC data only

Writes fixtures/<network>/ and fixtures/<network>/meta.json (endpoints, slots,
fork names, fetch time). eth_getProof must be fetched within ~128 blocks of the
finalized block (public RPCs are not archive nodes), so everything is fetched in
one run.
"""
import json, os, sys, time, urllib.request, datetime

UA = "Mozilla/5.0 (compatible; lc-fetch/0.1)"
NETS = {
    "sepolia": {
        "beacon": "https://lodestar-sepolia.chainsafe.io",
        # Lodestar answers bootstrap with 500 "syncCommitteeWitness not available" and
        # publicnode with 404, so bootstraps come from Nimbus (a second client).
        "bootstrap": "http://unstable.sepolia.beacon-api.nimbus.team",
        "cross": ["https://ethereum-sepolia-beacon-api.publicnode.com", "https://lodestar-sepolia.chainsafe.io"],
        "el": "https://ethereum-sepolia-rpc.publicnode.com",
        # publicnode, tenderly and 1rpc refuse eth_getProof at the finalized block
        # ("distance to target block exceeds maximum proof window"); these serve it.
        "el_proof": ["https://sepolia.rpc.thirdweb.com", "https://eth-sepolia.api.onfinality.io/public"],
        "chain_id": 11155111,
        # OP Sepolia L1CrossDomainMessenger (ResolvedDelegateProxy).
        # mapping(bytes32 => bool) successfulMessages is at storage slot 203
        # (found empirically: cast storage at keccak256(abi.encode(h, N)) for N in 195..215,
        #  cross-checked against eth_call successfulMessages(h) == true).
        "contract": "0x58Cc85b8D04EA49cC6DBd3CbFFd00B4B8D6cb3ef",
        "mapping_slot": 203,
        "relayed_topic": "0x4641df4a962071e12719d8c8c8e5ac7fc4d97b927346a3d7a335b1f7517e133c",
    },
    "mainnet": {
        "beacon": "https://lodestar-mainnet.chainsafe.io",
        # Neither Nimbus nor Lodestar served a bootstrap for a checkpoint at the start of the
        # previous period, so mainnet only gets the same-period test: bootstrap at the
        # current finalized checkpoint, then wait for finality to move on.
        "bootstrap": "https://lodestar-mainnet.chainsafe.io",
        "bootstrap_finalized_first": True,
        "cross": ["https://ethereum-beacon-api.publicnode.com"],
        "el": None,
        "chain_id": 1,
    },
}
SLOTS_PER_PERIOD = 32 * 256


def get(url, ssz=False):
    h = {"User-Agent": UA, "Accept": "application/octet-stream" if ssz else "application/json"}
    req = urllib.request.Request(url, headers=h)
    with urllib.request.urlopen(req, timeout=60) as r:
        body = r.read()
        return body, dict((k.lower(), v) for k, v in r.headers.items())


def getj(url):
    b, h = get(url)
    return json.loads(b), h


def rpc(url, method, params):
    data = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(url, data=data, headers={"User-Agent": UA, "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        out = json.loads(r.read())
    if "error" in out:
        raise RuntimeError(f"{method}: {out['error']}")
    return out["result"]


def _rlp_bytes(x):
    if len(x) == 1 and x[0] < 0x80:
        return x
    if len(x) <= 55:
        return bytes([0x80 + len(x)]) + x
    n = len(x).to_bytes((len(x).bit_length() + 7) // 8, "big")
    return bytes([0xb7 + len(n)]) + n + x


def _rlp_list(items):
    p = b"".join(items)
    if len(p) <= 55:
        return bytes([0xc0 + len(p)]) + p
    n = len(p).to_bytes((len(p).bit_length() + 7) // 8, "big")
    return bytes([0xf7 + len(n)]) + n + p


def header_rlp(b):
    """RLP of an execution header from eth_getBlockByHash JSON. Field order through Amsterdam
    (Glamsterdam EL): ... parentBeaconBlockRoot, requestsHash, blockAccessListHash, slotNumber.
    Verified by keccak == hash below (the other order of the last two does not match)."""
    h = lambda k: _rlp_bytes(bytes.fromhex(b[k][2:]))
    def i(k):
        v = int(b[k], 16)
        return _rlp_bytes(v.to_bytes((v.bit_length() + 7) // 8, "big") if v else b"")
    fields = [h("parentHash"), h("sha3Uncles"), h("miner"), h("stateRoot"), h("transactionsRoot"),
              h("receiptsRoot"), h("logsBloom"), i("difficulty"), i("number"), i("gasLimit"), i("gasUsed"),
              i("timestamp"), h("extraData"), h("mixHash"), h("nonce"), i("baseFeePerGas")]
    for k, f in [("withdrawalsRoot", h), ("blobGasUsed", i), ("excessBlobGas", i), ("parentBeaconBlockRoot", h),
                 ("requestsHash", h), ("blockAccessListHash", h), ("slotNumber", i)]:
        if k in b:
            fields.append(f(k))
    return _rlp_list(fields)


def split_chunks(b):
    """light_client/updates SSZ: [u64 LE len][4-byte fork digest][payload] per item; len = 4 + payload."""
    out, i = [], 0
    while i < len(b):
        n = int.from_bytes(b[i:i + 8], "little")
        digest = b[i + 8:i + 12]
        out.append((digest.hex(), b[i + 12:i + 8 + n]))
        i += 8 + n
    return out


def bootstrap_at_period(beacon, period, not_after_slot, outdir, name, cross=()):
    """Bootstrap at the first available epoch-boundary block of `period` (<= not_after_slot).
    The checkpoint block root is cross-checked against independent beacon endpoints."""
    slot = period * SLOTS_PER_PERIOD
    tries = 0
    while slot <= not_after_slot and tries < 8:
        tries += 1
        try:
            hdr, _ = getj(f"{beacon}/eth/v1/beacon/headers/{slot}")
            root = hdr["data"]["root"]
            if int(hdr["data"]["header"]["message"]["slot"]) != slot:
                raise RuntimeError("not at boundary")
            agree = []
            for c in cross:
                ch, _ = getj(f"{c}/eth/v1/beacon/headers/{slot}")
                if ch["data"]["root"] != root:
                    raise RuntimeError(f"checkpoint root mismatch vs {c}")
                agree.append(c)
            bj, bh = getj(f"{beacon}/eth/v1/beacon/light_client/bootstrap/{root}")
            bs, bsh = get(f"{beacon}/eth/v1/beacon/light_client/bootstrap/{root}", ssz=True)
            open(f"{outdir}/{name}.json", "w").write(json.dumps(bj, indent=1))
            open(f"{outdir}/{name}.ssz", "wb").write(bs)
            return {"block_root": root, "slot": slot, "period": period,
                    "fork": bsh.get("eth-consensus-version"), "ssz_bytes": len(bs),
                    "endpoint": beacon, "root_cross_checked_with": agree}
        except Exception as e:  # missed slot or not served: try next epoch boundary
            print(f"  bootstrap slot {slot}: {e}", file=sys.stderr)
            slot += 32
    raise RuntimeError(f"no bootstrap served in period {period}")


def main():
    net = sys.argv[1] if len(sys.argv) > 1 else "sepolia"
    cfg = NETS[net]
    beacon = cfg["beacon"]
    here = os.path.dirname(os.path.abspath(__file__))
    outdir = os.path.join(here, "..", "fixtures", net)
    os.makedirs(outdir, exist_ok=True)
    meta = {"network": net, "beacon_endpoint": beacon, "el_endpoint": cfg["el"], "chain_id": cfg["chain_id"],
            "fetched_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")}

    gen, _ = getj(f"{beacon}/eth/v1/beacon/genesis")
    meta["genesis_validators_root"] = gen["data"]["genesis_validators_root"]
    meta["genesis_time"] = int(gen["data"]["genesis_time"])
    fs, _ = getj(f"{beacon}/eth/v1/config/fork_schedule")
    meta["fork_schedule"] = fs["data"]
    open(f"{outdir}/fork_schedule.json", "w").write(json.dumps(fs, indent=1))

    boot_first = None
    if cfg.get("bootstrap_finalized_first"):
        hdr, _ = getj(f"{cfg['bootstrap']}/eth/v1/beacon/headers/finalized")
        root, bslot = hdr["data"]["root"], int(hdr["data"]["header"]["message"]["slot"])
        for c in cfg["cross"]:
            ch, _ = getj(f"{c}/eth/v1/beacon/headers/{root}")
            assert ch["data"]["root"] == root and int(ch["data"]["header"]["message"]["slot"]) == bslot
        bj, _ = getj(f"{cfg['bootstrap']}/eth/v1/beacon/light_client/bootstrap/{root}")
        bs, bsh = get(f"{cfg['bootstrap']}/eth/v1/beacon/light_client/bootstrap/{root}", ssz=True)
        open(f"{outdir}/bootstrap_same_period.json", "w").write(json.dumps(bj, indent=1))
        open(f"{outdir}/bootstrap_same_period.ssz", "wb").write(bs)
        boot_first = {"block_root": root, "slot": bslot, "period": bslot // SLOTS_PER_PERIOD,
                      "fork": bsh.get("eth-consensus-version"), "ssz_bytes": len(bs),
                      "endpoint": cfg["bootstrap"], "root_cross_checked_with": cfg["cross"]}
        print("bootstrap", boot_first, flush=True)
        for _ in range(60):  # wait (<= 30 min) for a finality update past the bootstrap
            fj, _ = getj(f"{beacon}/eth/v1/beacon/light_client/finality_update")
            if int(fj["data"]["finalized_header"]["beacon"]["slot"]) > bslot:
                break
            time.sleep(30)

    # Finality update: SSZ (guest input) and JSON (human-readable); refetch until both agree on slot.
    for _ in range(5):
        fs1, h1 = get(f"{beacon}/eth/v1/beacon/light_client/finality_update", ssz=True)
        fj, hj = getj(f"{beacon}/eth/v1/beacon/light_client/finality_update")
        fs2, _ = get(f"{beacon}/eth/v1/beacon/light_client/finality_update", ssz=True)
        if fs1 == fs2:
            break
        time.sleep(3)
    else:
        raise RuntimeError("finality update kept changing")
    open(f"{outdir}/finality_update.ssz", "wb").write(fs1)
    open(f"{outdir}/finality_update.json", "w").write(json.dumps(fj, indent=1))
    d = fj["data"]
    sig_slot = int(d["signature_slot"])
    fin_slot = int(d["finalized_header"]["beacon"]["slot"])
    period = sig_slot // SLOTS_PER_PERIOD
    meta["finality_update"] = {
        "fork": h1.get("eth-consensus-version"), "json_version": fj.get("version"),
        "attested_slot": int(d["attested_header"]["beacon"]["slot"]), "finalized_slot": fin_slot,
        "signature_slot": sig_slot, "signature_period": period, "ssz_bytes": len(fs1),
        "finalized_execution_block_hash": d["finalized_header"].get("execution_block_hash")
        or d["finalized_header"].get("execution", {}).get("block_hash"),
    }
    meta["finality_update"]["participation"] = bin(int(d["sync_aggregate"]["sync_committee_bits"], 16)).count("1")
    print("finality update", meta["finality_update"], flush=True)

    if cfg["el"]:
        el = cfg["el"]
        bh = meta["finality_update"]["finalized_execution_block_hash"]
        blk = rpc(el, "eth_getBlockByHash", [bh, False])
        blk.pop("transactions", None)
        num = blk["number"]
        open(f"{outdir}/el_block_header.json", "w").write(json.dumps(blk, indent=1))
        raw = None
        try:
            raw = rpc(el, "debug_getRawHeader", [num])
        except Exception as e:
            print("debug_getRawHeader unavailable:", e, file=sys.stderr)
        if not raw:
            raw = "0x" + header_rlp(blk).hex()
        import subprocess
        kh = subprocess.check_output([os.path.expanduser("~/.foundry/bin/cast"), "keccak", raw]).decode().strip()
        assert kh == bh, f"rebuilt header hashes to {kh}, expected {bh}"
        open(f"{outdir}/el_block_header.rlp.hex", "w").write(raw)
        # A message hash relayed on the messenger at or before the finalized block.
        logs = []
        n = int(num, 16)
        lo = n - 2000
        while not logs and lo > n - 200000:
            logs = rpc(el, "eth_getLogs", [{"address": cfg["contract"], "fromBlock": hex(lo), "toBlock": num,
                                            "topics": [cfg["relayed_topic"]]}])
            lo -= 2000
        msg_hash = logs[-1]["topics"][1]
        meta["storage"] = {"contract": cfg["contract"], "mapping_slot_index": cfg["mapping_slot"],
                           "mapping_key": msg_hash, "relayed_in_block": int(logs[-1]["blockNumber"], 16),
                           "relay_tx": logs[-1]["transactionHash"], "block_number": n, "block_hash": bh,
                           "state_root": blk["stateRoot"], "raw_header_from_debug_api": bool(raw)}
        # slot key = keccak256(abi.encode(key, N)) is computed by the host, which also re-checks it here:
        try:
            import subprocess
            enc = subprocess.check_output([os.path.expanduser("~/.foundry/bin/cast"), "abi-encode", "f(bytes32,uint256)",
                                           msg_hash, str(cfg["mapping_slot"])]).decode().strip()
            slot_key = subprocess.check_output([os.path.expanduser("~/.foundry/bin/cast"), "keccak", enc]).decode().strip()
        except Exception as e:
            raise RuntimeError(f"need cast to derive slot key: {e}")
        meta["storage"]["slot_key"] = slot_key
        proof, err = None, []
        for pe in cfg["el_proof"]:
            try:
                proof = rpc(pe, "eth_getProof", [cfg["contract"], [slot_key], num])
                blk2 = rpc(pe, "eth_getBlockByNumber", [num, False])
                assert blk2["hash"] == bh and blk2["stateRoot"] == blk["stateRoot"], "canonical check failed"
                meta["storage"]["proof_endpoint"] = pe
                break
            except Exception as e:
                err.append(f"{pe}: {e}")
        if proof is None:
            raise RuntimeError(f"eth_getProof failed: {err}")
        open(f"{outdir}/eth_getProof.json", "w").write(json.dumps(proof, indent=1))
        meta["storage"]["value"] = proof["storageProof"][0]["value"]
        meta["storage"]["account_proof_nodes"] = len(proof["accountProof"])
        meta["storage"]["storage_proof_nodes"] = len(proof["storageProof"][0]["proof"])
        print("storage", meta["storage"])

    # Committee-period updates for period-1 (carries next committee = committee of `period`) and period.
    us, uh = get(f"{beacon}/eth/v1/beacon/light_client/updates?start_period={period - 1}&count=2", ssz=True)
    uj, _ = getj(f"{beacon}/eth/v1/beacon/light_client/updates?start_period={period - 1}&count=2")
    chunks = split_chunks(us)
    meta["updates"] = []
    for i, (digest, payload) in enumerate(chunks):
        p = period - 1 + i
        open(f"{outdir}/update_period_{p}.ssz", "wb").write(payload)
        open(f"{outdir}/update_period_{p}.json", "w").write(json.dumps(uj[i], indent=1))
        ud = uj[i]["data"]
        meta["updates"].append({
            "period": p, "fork": uj[i].get("version"), "fork_digest": digest, "ssz_bytes": len(payload),
            "attested_slot": int(ud["attested_header"]["beacon"]["slot"]),
            "finalized_slot": int(ud["finalized_header"]["beacon"]["slot"]),
            "signature_slot": int(ud["signature_slot"]),
            "participation": bin(int(ud["sync_aggregate"]["sync_committee_bits"], 16)).count("1"),
        })
    print("updates", meta["updates"])

    if boot_first:
        meta["bootstrap_same_period"] = boot_first
        meta["bootstrap_prev_period"] = None
        open(f"{outdir}/meta.json", "w").write(json.dumps(meta, indent=1))
        print("wrote", outdir)
        return
    # Bootstraps: one in period-1 (handoff test) and one in period (same-period test).
    meta["bootstrap_prev_period"] = bootstrap_at_period(cfg["bootstrap"], period - 1, (period) * SLOTS_PER_PERIOD - 1,
                                                        outdir, "bootstrap_prev_period", cfg["cross"])
    meta["bootstrap_same_period"] = bootstrap_at_period(cfg["bootstrap"], period, fin_slot - 32, outdir,
                                                        "bootstrap_same_period", cfg["cross"])
    print("bootstraps", meta["bootstrap_prev_period"], meta["bootstrap_same_period"])

    open(f"{outdir}/meta.json", "w").write(json.dumps(meta, indent=1))
    print("wrote", outdir)


if __name__ == "__main__":
    main()
