import assert from "node:assert/strict";
import http from "node:http";
import {randomUUID} from "node:crypto";

// Real service + signed guest, MemoryVault and owned loopback upstream only.
export async function catalogDraftCases({api,plugin,provider,scoped,state,report,oldGets}) {
  report.draftCatalogCases=[];
  const passed=(name)=>{report.draftCatalogCases.push(name);console.log("通过："+name);};
  const secret="owned-edited-draft-key-8361";
  let gets=0,posts=0,hold=false,arrived;
  const held=new Map();
  const server=http.createServer((req,res)=>{
    if(req.method!=="GET"){posts++;res.writeHead(500);res.end();return;}
    assert.equal(req.headers.authorization,"Bearer "+secret);gets++;
    if(hold){held.set(res,req.url.includes("/models"));res.on("close",()=>held.delete(res));arrived?.();return;}
    const u=new URL(req.url,"http://127.0.0.1");
    if(u.pathname.endsWith("/models"))res.end(JSON.stringify({data:[{id:"draft-native"}]}));
    else {assert.equal(u.pathname,"/edited/v1/catalog");res.end(JSON.stringify(u.searchParams.has("after_id")?{entries:[{key:"draft-external-b",tools:false}],more:false}:{entries:[{key:"draft-external-a"}],more:true,cursor:"next"}));}
  });
  await new Promise(r=>server.listen(0,"127.0.0.1",r));
  const endpoint=`http://127.0.0.1:${server.address().port}/edited/v1/responses`;
  const ref={pluginId:plugin.manifest.id,digest:plugin.digest};
  const protocol=()=>api("/api/provider-operations/protocol");
  const action=(kind,request,status=200)=>api("/api/provider-draft-catalog",{kind,request},status);
  const owned=new Set();let storeId;
  const bind=async(target)=>{
    const before=await api("/api/provider-catalog/status",ref);
    const status=await scoped(storeId,{...ref,action:{kind:"bind",target,allowPrivateNetwork:true}});
    const row=status.bindings.find(b=>b.authorized&&b.endpoint===(target.kind==="draft"?target.endpoint:provider.endpoint)&&
      (target.kind==="draft"?b.label===target.label:b.providerId===target.providerId));assert(row);
    assert(!before.bindings.some(b=>b.bindingId===row.bindingId));owned.add(row.bindingId);
    return {...ref,bindingId:row.bindingId,target};
  };
  const release=(send=false)=>{hold=false;arrived=null;for(const [r,native] of held) {
    if(send)r.end(JSON.stringify(native?{data:[{id:"held-first-page"}],has_more:true,last_id:"next"}:{entries:[{key:"held-first-page"}],more:true,cursor:"next"}));else r.destroy();
  }};
  try {
    const key=await api("/api/provider-credentials",{label:"Owned edited Draft",secret});
    const start=await protocol();storeId=start.storeId;
    const saved=(await api("/api/provider-operations",{storeId,operationId:randomUUID(),expectedSequence:start.sequence,kind:"save",provider:{...provider,id:"custom-"+randomUUID(),name:"Owned draft source",revision:0,configured:false}})).provider;
    const sequence=(await protocol()).sequence;
    const query={storeId,providerId:saved.id,expectedRevision:String(saved.revision)};
    let source=await api("/api/provider-catalog-source",{kind:"state",query});
    const draft={endpoint,protocol:"responses",vendor:saved.vendor,credentialRef:key.credentialRef};
    const make=(choice={kind:"builtin"})=>({origin:{kind:"saved",query,sourceRevision:source.sourceRevision},draft:{...draft},source:choice});
    const select=async()=>{source=await api("/api/provider-catalog-source",{kind:"select",query,sourceRevision:source.sourceRevision,catalog:null});};
    const originalGets=oldGets();
    await action("check",make());assert.equal(gets,0);
    const invalid=make();invalid.source.catalog=null;
    await action("check",invalid,422);
    await action("check",{...make(),headers:{authorization:secret}},422);
    await action("check",{...make(),origin:{kind:"saved",query:{...query,storeId:randomUUID()},sourceRevision:null}},409);
    await action("check",{...make(),origin:{kind:"saved",query:{...query,expectedRevision:"0"},sourceRevision:null}},409);
    assert.equal(gets,0);assert.equal((await protocol()).sequence,sequence);
    passed("草稿严格字段、原数据库与连接修订准入先于 I/O；范围核查不读目录或推进 SQL 时钟");
    const native=await action("read",make());assert.deepEqual(native.models.map(m=>m.id),["draft-native"]);
    const legacy=await api("/api/provider-models",draft);delete native.fetchedAt;delete legacy.fetchedAt;assert.deepEqual(native,legacy);
    assert.equal(oldGets(),originalGets);
    passed("已编辑地址和新凭据只用于当前草稿；未请求 Saved 的旧地址，内置结果保持旧接口等价");
    const fresh={...make(),origin:{kind:"new",storeId,providerId:"custom-"+randomUUID()}};
    await action("check",fresh);await action("read",fresh);
    await action("read",{...fresh,origin:{kind:"new",storeId,providerId:saved.id}},409);
    passed("未保存草稿可读取，但不能用 New 身份接管已保存连接");
    const target={kind:"draft",label:"Owned current edited draft",...draft};
    const catalog=await bind(target);
    const external=make({kind:"external",catalog});
    let before=gets;await action("check",external);assert.equal(gets,before);
    const result=await action("read",external);assert.deepEqual(result.models.map(m=>m.id),["draft-external-a","draft-external-b"]);assert.equal(gets,before+2);
    passed("精确 Draft 授权通过统一宿主接口实际消费签名 guest 两页目录；元数据核查不启动读取");
    for(const changed of [{endpoint:endpoint+"/other"},{vendor:"custom"},{protocol:"messages"},{credentialRef:saved.credentialRef}]) {
      const r=await action("read",{...external,draft:{...draft,...changed}},409);assert.equal(r.code,"stale_snapshot");
    }
    before=gets;
    const savedGrant=await bind({kind:"saved",providerId:saved.id,expectedRevision:String(saved.revision)});
    await action("read",make({kind:"external",catalog:savedGrant}),403);assert.equal(gets,before);
    passed("地址、协议、供应商和凭据变化使旧草稿授权失效；Saved 授权不能代替 Draft 授权");
    const stale=make();await select();before=gets;
    await action("read",stale,409);await select();await action("read",stale,409);assert.equal(gets,before);
    passed("远端来源变化及切换往返均拒绝旧令牌，未发生内置请求或复活旧来源");
    for(const choice of [{kind:"builtin"},{kind:"external",catalog}]) {
      hold=true;const hit=new Promise(r=>arrived=r);const request=make(choice);before=gets;
      const pending=action("read",request,409);await hit;await select();release(true);await pending;
      assert.equal(gets,before+1);
    }
    passed("内置和外部读取在途来源变化停止交付和后续分页，最多保留已经准入的一次请求");
    hold=true;const hit=new Promise(r=>arrived=r);before=gets;
    const pending=action("read",make({kind:"external",catalog}),403);await hit;
    await state(plugin,"revoke-grants");release(true);await pending;assert.equal(gets,before+1);
    await action("check",make({kind:"external",catalog}),403);
    passed("撤权停止在途草稿交付，后续元数据核查失败，没有原生回退");
    assert.deepEqual((await api("/api/snapshot")).providers.find(p=>p.id===saved.id),saved);
    assert.equal((await protocol()).sequence,sequence);assert.equal(posts,0);assert.equal(oldGets(),originalGets);
    report.draftCatalogGets=gets;report.draftCatalogPosts=posts;
    passed("草稿读取和授权核查保留 Saved Provider 字节与原操作序号，零模型 POST");
  } finally {
    release();for(const bindingId of owned)await scoped(storeId,{...ref,action:{kind:"unbind",bindingId}});
    server.closeAllConnections();await new Promise(r=>server.close(r));
  }
}
