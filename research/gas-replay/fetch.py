import json,urllib.request,sys
R="https://ethereum-sepolia-rpc.publicnode.com"
def rpc(m,p,url=R):
    req=urllib.request.Request(url,data=json.dumps({"jsonrpc":"2.0","id":1,"method":m,"params":p}).encode(),headers={"content-type":"application/json","user-agent":"curl/8"})
    r=json.load(urllib.request.urlopen(req,timeout=60))
    if "error" in r: raise Exception(r["error"])
    return r["result"]
H="""8602ac1b6ecfa4847eabab83e9a9595b5efa68500177d17b021b942fe8f10034 failedDemoUSDCcreate
27a170fb593e42aab050ef25d4c1e53c38373b1ed2a44f4cadf4934c2dddd923 createDemoUSDC
42a596755476edc08a4563d61061d33b99424bf8f78d17288b5670f4fb231322 createBridgeVault
dd8c3b67477c5e6fc5ddb9ca931a18ac2195146db325e2a1cffd43819fe09f4d registerToken_dUSDC
7859c57668e3d3e3f9c272466830a26119502cf59ebcf4e742a9f4674dd23e23 registerToken_ETH
994d4f2d50ca1a3a4d21d2f9d49f6aa509c96816b9ac6336faa36091687a848a setRelayFeeMenu_dUSDC
9effb8d02a13ce3f83d0b391677e0b861d1895d3ff91e69d6ac8cd2cc0c05385 setRelayFeeMenu_ETH
23eabef61d5dc64f479c203b246803f418846356f9d95486640bd5de19b11142 setCaps_dUSDC
92745cdfaad9a3490201c4d2a89c4c278c5f7ee89d4d5f83a86192766dc788b6 setCaps_ETH
6f3f366207169219cf85ba9f61f945c818b345a69aef726a9f6dc8fbd4b39048 setGlobalCaps
02ed511a12939507041739f9fb990f88cf44704bb77a477d25faab0e0818f28b setCapWeight_dUSDC
6d6c28084d80dea1d9b37fce4ff9270660f6c4b520f06bd11ab109278234c02d setCapWeight_ETH
1244cd0741446cf805ad27521fc6293b65e45386c9d7cf0d70ef8a68b88d4519 mint
0addfd0c694bcdf454e013b509e82e02ca18288794d802d81a34d9b6e2d0aa2a approve
220a9257ddfe6f9ee11d80f6be2fcc6a12c84f3f68ed37aaa79a7998b7e47bac deposit1_dUSDC
110800c5a720fc1e1dde9097ba6b80498ba3ca894af13b98ca1d0c5a21bcabf6 deposit2_dUSDC
779e97d1e80519fd3ccbb480f7d1960f4bdcf37ebfc18e23cf392fd04c3e25ff deposit3_dUSDC
2ca9753b40deb5fa4da8bf87f2f2127fbe36f9b51ade3602f26ddfa880f703d4 deposit4_dUSDC
a9351249aafdcff2ea9c5dea5ac07e9a8aa2656b7fdc17337be55307e8b0581b deposit5_ETH"""
out=[]
for line in H.splitlines():
    h,name=line.split(); h="0x"+h
    tx=rpc("eth_getTransactionByHash",[h]); rc=rpc("eth_getTransactionReceipt",[h])
    blk=rpc("eth_getBlockByNumber",[rc["blockNumber"],False])
    out.append(dict(name=name,hash=h,nonce=int(tx["nonce"],16),to=tx["to"],value=tx["value"],input=tx["input"],gasLimit=int(tx["gas"],16),
      gasUsed=int(rc["gasUsed"],16),status=int(rc["status"],16),contract=rc.get("contractAddress"),block=int(rc["blockNumber"],16),ts=int(blk["timestamp"],16),
      accessList=tx.get("accessList"),inputLen=(len(tx["input"])-2)//2, receiptKeys=sorted(rc.keys())))
    print(name,out[-1]["nonce"],out[-1]["gasLimit"],out[-1]["gasUsed"],out[-1]["status"],out[-1]["inputLen"],out[-1]["contract"])
json.dump(out,open(sys.argv[1],"w"),indent=1)
print(out[0]["receiptKeys"])
code=rpc("eth_getCode",["0x2a098988600d87650Fb061FfAff08B97149Fa84D","latest"])
open(sys.argv[2],"w").write(code); print("verifier code bytes",(len(code)-2)//2)
for a in ["0xA34adbe8413838668b0521230A4d96C6B7e91484","0xAc207b9Bfebf3eF7C26297eA6AbaDeEcCC4Aa71e"]:
    print(a,"code bytes",(len(rpc("eth_getCode",[a,"latest"]))-2)//2)
