import assert from "node:assert/strict";
import {randomUUID} from "node:crypto";
import {DatabaseSync} from "node:sqlite";
import path from "node:path";

// Test-only upstream. The host performs actual verification HTTP against this
// response, with no real model, external account or Agent process.
export async function catalogProbe(req,res,control,secret) {
  assert.equal(req.headers.authorization,"Bearer "+secret);
  assert.equal(req.url,"/custom/v1/responses");
  let raw="";for await(const part of req) raw+=part;
  const request=JSON.parse(raw);
  if(control.mode==="auth-once") {control.mode="ok";res.writeHead(403,{"content-type":"application/json"});res.end("{}");return;}
  if(control.mode==="shape") {res.writeHead(200,{"content-type":"application/json"});res.end(JSON.stringify({choices:[]}));return;}
  if(control.mode==="hold") {control.waiting?.();control.held.add(res);res.on("close",()=>control.held.delete(res));return;}
  res.writeHead(200,{"content-type":"application/json"});
  res.end(JSON.stringify({id:"catalog-probe-fixture",object:"response",status:"completed",model:request.model,output:[{type:"message",id:"catalog-message",role:"assistant",content:[{type:"output_text",text:"CATALOG_OFFLINE",annotations:[]}]}],usage:{input_tokens:4,output_tokens:1,total_tokens:5}}));
}
export async function catalogConnectionCases({api,plugin,provider,posts,gets,state,scoped,control,report,data}) {
  report.connectionCases=[];
  const fixed=process.env.CATALOG_CONNECT_EXPECT_FIXED!=="0";report.fixedNoResend=fixed;
  const passed=(name)=>{report.connectionCases.push(name);console.log("通过："+name);};
  const protocol=()=>api("/api/provider-operations/protocol");
  const custom=()=>({...provider,id:"custom-"+randomUUID(),name:"Owned catalog connection",vendor:"custom",model:"",models:[],disabledModels:[],revision:0,configured:false,modelCatalog:null,category:"custom"});
  const bind=async(p)=>{
    const scope=await protocol();
    const target={kind:"draft",label:p.name,endpoint:p.endpoint,protocol:p.protocol,vendor:p.vendor,credentialRef:p.credentialRef};
    const input={pluginId:plugin.manifest.id,digest:plugin.digest,action:{kind:"bind",target,allowPrivateNetwork:true}};
    const response=await scoped(scope.storeId,input);
    const binding=response.bindings.find((r)=>!r.providerId&&r.label===p.name&&r.authorized);
    assert(binding);return {pluginId:plugin.manifest.id,digest:plugin.digest,bindingId:binding.bindingId,target};
  };
  const frozen=async(p,catalog)=>{
    const scope=await protocol();return {operation:{storeId:scope.storeId,operationId:randomUUID(),expectedSequence:scope.sequence,kind:"connect",provider:p},catalog};
  };
  report.manualSaveSupported=process.env.PROVIDER_CATALOG_MANUAL_SAVE==="1";
  if(report.manualSaveSupported) {
    const manual={...custom(),name:"Owned manual catalog",model:"manual-entry",models:["manual-entry"],credentialRef:randomUUID()};
    // This reference has no Vault entry. Manual persistence must not read it.
    const manualChoice=await bind(manual), saveInput=await frozen(manual,manualChoice);saveInput.operation.kind="save";
    const beforePosts=posts(),beforeGets=gets();
    const saved=await api("/api/catalog-provider-operations",saveInput);
    assert.equal(saved.status,"committed");assert.equal(saved.verification,null);assert.equal(saved.provider.model,"manual-entry");
    assert.equal(posts(),beforePosts);assert.equal(gets(),beforeGets);
    const source=await api("/api/provider-catalog-source",{kind:"state",query:{storeId:saveInput.operation.storeId,providerId:manual.id,expectedRevision:String(saved.provider.revision)}});
    assert.equal(source.kind,"external");assert.equal(source.authorized,false);assert.equal(source.selection,null);assert(!JSON.stringify(source).includes(manual.credentialRef));
    report.manualSaveCases=["manual Save persists source and the unchanged receipt without directory/Vault/model work"];
    // Reproduce a lost SQL result on owned data. The original native IPC must
    // not take over the bound request, even when its clock appears unconsumed.
    const db=new DatabaseSync(path.join(data,"local-agent.sqlite3"));db.exec("PRAGMA busy_timeout=5000");
    const op=db.prepare("SELECT * FROM provider_operations WHERE operation_id=?").get(saveInput.operation.operationId);
    const value=db.prepare("SELECT value FROM providers WHERE id=?").get(manual.id).value;
    const clock=db.prepare("SELECT sequence FROM provider_operation_scope WHERE id=1").get().sequence;
    db.exec("BEGIN IMMEDIATE");db.prepare("DELETE FROM provider_operations WHERE operation_id=?").run(saveInput.operation.operationId);db.prepare("DELETE FROM providers WHERE id=?").run(manual.id);db.prepare("UPDATE provider_operation_scope SET sequence=? WHERE id=1").run(saveInput.operation.expectedSequence);db.exec("COMMIT");
    try {
      const refused=await api("/api/provider-operations",saveInput.operation,503);assert.equal(refused.code,"catalog_operation_unavailable");assert.equal(refused.outcomeUnknown,true);
      assert.equal(db.prepare("SELECT count(*) AS n FROM providers WHERE id=?").get(manual.id).n,0);
      assert.equal(db.prepare("SELECT count(*) AS n FROM provider_operations WHERE operation_id=?").get(saveInput.operation.operationId).n,0);
      assert.equal(posts(),beforePosts);assert.equal(gets(),beforeGets);
    } finally {
      db.exec("BEGIN IMMEDIATE");db.prepare("INSERT INTO providers(id,value) VALUES(?,?)").run(manual.id,value);db.prepare("INSERT INTO provider_operations(sequence,operation_id,state,request_hash,value) VALUES(?,?,?,?,?)").run(op.sequence,op.operation_id,op.state,op.request_hash,op.value);db.prepare("UPDATE provider_operation_scope SET sequence=? WHERE id=1").run(clock);db.exec("COMMIT");db.close();
    }
    report.manualSaveCases.push("legacy Save refuses takeover after owned SQL result loss; original rows restored for subsequent checks");
    await state(plugin,"disable");assert.deepEqual(await api("/api/catalog-provider-operations",saveInput),saved);assert.deepEqual(await api("/api/provider-operations",saveInput.operation),saved);await state(plugin,"enable");
    assert.equal(posts(),beforePosts);assert.equal(gets(),beforeGets);
    passed("手动保存不读目录、凭据或模型，保留原回执；SQL 结果缺失时旧入口不能接管外部原请求");
  } else {
    const p=custom(), selection=await bind(p), input=await frozen(p,selection);
    const sequenceBefore=(await protocol()).sequence;
    assert.equal((await api("/api/catalog-provider-operations",{...input,operation:{...input.operation,kind:"save"}},400)).code,"invalid_input");
    assert.equal((await protocol()).sequence,sequenceBefore);
    passed("旧开发产物不支持的保存模式在操作准入前拒绝，不消费序号或执行探测");
  }
  const p=custom(),selection=await bind(p),input=await frozen(p,selection);
  let before=posts();
  const result=await api("/api/catalog-provider-operations",input);
  assert.equal(result.status,"committed");assert.equal(result.provider.revision,1);assert.equal(result.verification.state,"verified");
  assert.deepEqual(result.provider.models,["model-a","model-b","model-false"]);
  assert(result.provider.modelCatalog.sourceUrl.endsWith("/custom/v1/catalog"));assert.equal(posts(),before+1);
  const count=posts(),reads=gets();
  assert.deepEqual(await api("/api/catalog-provider-operations",input),result);
  assert.deepEqual(await api("/api/provider-operations",input.operation),result);
  assert.equal(posts(),count);assert.equal(gets(),reads);
  passed("插件目录实际进入验证、模型保存和原操作回执；新旧入口重放均不二次读取或发送");
  const mismatch=await api("/api/catalog-provider-operations/query",{...input,catalog:{...selection,digest:"0".repeat(64)}},fixed?503:400);
  assert.equal(mismatch.code,"catalog_operation_unavailable");if(fixed)assert.equal(mismatch.outcomeUnknown,true);
  assert.equal(posts(),count);assert.equal(gets(),reads);
  await state(plugin,"disable");
  assert.deepEqual(await api("/api/catalog-provider-operations/query",input),result);
  assert.equal(posts(),count);assert.equal(gets(),reads);
  passed("结果核查固定原目录选择，错误选择拒绝；插件停用后仍能读取已确认结果且不重放");
  await state(plugin,"enable");
  const wrongScope=randomUUID();const unchanged=(await protocol()).sequence;
  await scoped(wrongScope,{pluginId:plugin.manifest.id,digest:plugin.digest,action:{kind:"bind",target:selection.target,allowPrivateNetwork:true}},409);
  assert.equal((await protocol()).sequence,unchanged);
  passed("跨数据库目录授权先被拒绝，没有消费业务操作序号或发起网络请求");
  const cancelled=custom(),cancelChoice=await bind(cancelled),cancelInput=await frozen(cancelled,cancelChoice);
  const cancelledResult=await api("/api/provider-operations/cancel",cancelInput.operation);
  assert.equal(cancelledResult.status,"cancelled");
  const beforeCancelled=posts();
  assert.equal((await api("/api/catalog-provider-operations",cancelInput)).status,"cancelled");
  assert.equal(posts(),beforeCancelled);
  passed("先取消的原操作不会被新的目录入口接管或重新发送");
  const malformed=custom(),shapeChoice=await bind(malformed),shapeInput=await frozen(malformed,shapeChoice);
  control.mode="shape";before=posts();
  const shapeResult=await api("/api/catalog-provider-operations",shapeInput);
  assert.equal(shapeResult.status,"failed");assert.equal(shapeResult.error.code,fixed?"probe_shape":"connection_unknown");
  assert.equal(posts()-before,fixed?1:3);
  assert(!(await api("/api/snapshot")).providers.some((r)=>r.id===malformed.id));
  const afterShape=posts();assert.deepEqual(await api("/api/catalog-provider-operations",shapeInput),shapeResult);assert.equal(posts(),afterShape);
  passed(fixed?"模型已发送后的无效响应只保留一次发送，不改用其他模型重发":"复现：固定协议的无效响应仍导致三个模型发送");
  control.mode="auth-once";const permitted=custom(),authChoice=await bind(permitted),authInput=await frozen(permitted,authChoice);before=posts();
  const authResult=await api("/api/catalog-provider-operations",authInput);assert.equal(authResult.status,"committed");assert.equal(authResult.provider.model,"model-b");assert.equal(posts(),before+2);
  passed("明确的 HTTP 权限拒绝保留有限模型候选选择，不把未知响应当拒绝重试");
  const revoked=custom(),revokeChoice=await bind(revoked),revokeInput=await frozen(revoked,revokeChoice);
  control.mode="hold";const arrived=new Promise((resolve)=>control.waiting=resolve);before=posts();
  const pending=api("/api/catalog-provider-operations",revokeInput);
  await arrived;await api("/api/snapshot");await state(plugin,"revoke-grants");
  const failed=await pending;assert.equal(failed.status,"failed");assert.equal(failed.error.code,"grant_revoked");
  assert.equal(posts(),before+1);assert(!(await api("/api/snapshot")).providers.some((r)=>r.id===revoked.id));
  for(const response of control.held)response.destroy();control.mode="ok";
  assert.deepEqual(await api("/api/catalog-provider-operations",revokeInput),failed);
  assert.equal(posts(),before+1);
  passed("模型验证在途撤权立即停止，失败回执保留，不保存连接、不回退或二次发送");
  // Grants used by these drafts are explicitly released; saved metadata survives.
  const scope=await protocol();
  const status=await api("/api/provider-catalog/status",{pluginId:plugin.manifest.id,digest:plugin.digest});
  for(const binding of status.bindings.filter((b)=>!b.providerId)) {
    await scoped(scope.storeId,{pluginId:plugin.manifest.id,digest:plugin.digest,action:{kind:"unbind",bindingId:binding.bindingId}});
  }
  assert((await api("/api/snapshot")).providers.some((r)=>r.id===result.provider.id));
  passed("草稿目录授权可以独立释放，已确认的 Provider 与原回执继续保留");
  return {provider:result.provider,input};
}
