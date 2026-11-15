import json,urllib.request,sys
U="http://127.0.0.1:"+sys.argv[1]
def rpc(m,p):
    r=json.load(urllib.request.urlopen(urllib.request.Request(U,data=json.dumps({"jsonrpc":"2.0","id":1,"method":m,"params":p}).encode(),headers={"content-type":"application/json"})))
    if "error" in r: raise Exception(r["error"])
    return r["result"]
for bn in map(int,sys.argv[2:]):
    b=rpc("eth_getBlockByNumber",[hex(bn),False]); h=b["transactions"][0]
    tr=rpc("debug_traceTransaction",[h,{"tracer":"prestateTracer","tracerConfig":{"diffMode":True}}])
    print("block",bn,h)
    for a,v in tr["post"].items():
        for k,val in (v.get("storage") or {}).items():
            old=(tr["pre"].get(a,{}).get("storage") or {}).get(k,"0x0")
            print(" ",a[:8],k,"NEW" if int(old,16)==0 else "chg",old[:14],"->",val[:14])
