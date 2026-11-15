import json,urllib.request,sys,subprocess,time
hf,port,txfile,verfile,outfile=sys.argv[1],sys.argv[2],sys.argv[3],sys.argv[4],sys.argv[5]
U=f"http://127.0.0.1:{port}"
def rpc(m,p):
    req=urllib.request.Request(U,data=json.dumps({"jsonrpc":"2.0","id":1,"method":m,"params":p}).encode(),headers={"content-type":"application/json"})
    r=json.load(urllib.request.urlopen(req,timeout=300))
    if "error" in r: raise Exception(m+" "+json.dumps(r["error"])[:500])
    return r["result"]
txs=json.load(open(txfile))
D="0x37a46e7334dd061e0c7e2298247ee9278d0d321f"
rpc("anvil_setBalance",[D,hex(10**24)])
rpc("anvil_setCode",["0x2a098988600d87650Fb061FfAff08B97149Fa84D",open(verfile).read().strip()])
res=[]
for t in txs:
    if t["status"]==0: continue
    rpc("anvil_setNonce",[D,hex(t["nonce"])])
    rpc("evm_setNextBlockTimestamp",[t["ts"]])
    gas = 16_000_000 if hf!="amsterdam" else 200_000_000
    if t["name"]=="createBridgeVault" and hf=="amsterdam": gas=60_000_000
    tx={"from":D,"nonce":hex(t["nonce"]),"data":t["input"],"value":t["value"],"gas":hex(gas),"maxFeePerGas":hex(10**11),"maxPriorityFeePerGas":hex(1)}
    if t["to"]: tx["to"]=t["to"]
    # estimate first (same state)
    est=None
    try: est=int(rpc("eth_estimateGas",[{k:v for k,v in tx.items() if k not in("gas","maxFeePerGas","maxPriorityFeePerGas")}]),16)
    except Exception as e: est=str(e)[:200]
    h=rpc("eth_sendTransaction",[tx])
    rc=rpc("eth_getTransactionReceipt",[h])
    while rc is None:
        time.sleep(0.2); rc=rpc("eth_getTransactionReceipt",[h])
    # storage diff
    diff=None
    try:
        tr=rpc("debug_traceTransaction",[h,{"tracer":"prestateTracer","tracerConfig":{"diffMode":True}}])
        pre,post=tr["pre"],tr["post"]
        new=chg=clr=0; per={}
        addrs=set(pre)|set(post)
        for a in addrs:
            ps=pre.get(a,{}).get("storage",{}) or {}; qs=post.get(a,{}).get("storage",{}) or {}
            for k in set(ps)|set(qs):
                b=int(ps.get(k,"0x0"),16); c=int(qs.get(k,ps.get(k,"0x0")),16) if k in qs else 0
                # diffMode: post omits deleted slots -> treat as zero only if absent in post but present in pre
                if k not in qs: c=0
                if b==0 and c!=0: new+=1; per.setdefault(a,[0,0,0])[0]+=1
                elif b!=0 and c==0: clr+=1; per.setdefault(a,[0,0,0])[2]+=1
                elif b!=c: chg+=1; per.setdefault(a,[0,0,0])[1]+=1
        newacct=[a for a in post if a not in pre]
        diff=dict(newSlots=new,changedSlots=chg,clearedSlots=clr,perAddr=per,newAccounts=newacct)
    except Exception as e: diff=str(e)[:200]
    r=dict(name=t["name"],sepoliaGas=t["gasUsed"],localGas=int(rc["gasUsed"],16),status=int(rc["status"],16),estimate=est,contract=rc.get("contractAddress"),diff=diff,receiptKeys=sorted(rc.keys()))
    res.append(r); print(r["name"],r["sepoliaGas"],r["localGas"],r["status"],r["estimate"],json.dumps(diff)[:220])
json.dump(res,open(outfile,"w"),indent=1)
print(res[0]["receiptKeys"])
