import json,sys,glob
S=sys.argv[1]; tag=sys.argv[2]
def load(n):
    d=json.load(open(f"{S}/bc-{tag}-{n}.json"))
    txs=d["transactions"]; rcs=d["receipts"]
    out=[]
    for t,r in zip(txs,rcs):
        out.append((t.get("function") or ("create "+str(t.get("contractName"))), t.get("arguments"), int(r["gasUsed"],16), int(r["status"],16)))
    return out
res={}
for n in ["setUpAll","deposits","extraDeposits","updates","releases"]:
    try: L=load(n)
    except Exception as e: print("missing",n,e); continue
    bad=[x for x in L if x[3]!=1]
    if bad: print("FAILED TXS",n,len(bad))
    if n=="deposits":
        for i in [0,1,2,3,4,5,6,7,8,127,128,255,256,299]: print(f"  ERC20 deposit #{i}: {L[i][2]}")
        res["dep"]=[x[2] for x in L]
    else:
        for x in L: print(f"  {n}: {x[0][:40]:40s} {x[2]}")
json.dump(res,open(f"{S}/deps-{tag}.json","w"))
