import assert from 'node:assert/strict';
import fs from 'node:fs';import path from 'node:path';import os from 'node:os';
import {spawn,execFileSync} from 'node:child_process';import {once} from 'node:events';import {createInterface} from 'node:readline';import {randomUUID,createHash} from 'node:crypto';
const binary=path.resolve(process.env.PLUGIN_TEST_BINARY??'target/debug/local-agent-service');
const previous=process.env.PLUGIN_CONTRIBUTIONS_BASELINE;
const root=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'local-agent-contributions-')));const data=path.join(root,'data'),home=path.join(root,'home');fs.mkdirSync(home);
const report={complete:false,binary,binarySha256:createHash('sha256').update(fs.readFileSync(binary)).digest('hex'),root,cases:[],realModelCalls:0};
const passed=name=>{report.cases.push(name);console.log('通过：'+name);};
const sdk=(...args)=>execFileSync(binary,['--plugin-sdk',...args],{encoding:'utf8'});const key=path.join(root,'publisher.key');sdk('keygen',key);
function pack(example,edit=()=>{},editContribution=()=>{}){const folder=path.join(root,randomUUID());fs.cpSync(`sdk/plugins/examples/${example}/package`,folder,{recursive:true});const file=path.join(folder,'manifest.json');const m=JSON.parse(fs.readFileSync(file));edit(m);fs.writeFileSync(file,JSON.stringify(m));const cfile=path.join(folder,'contributions.json');if(fs.existsSync(cfile)){const c=JSON.parse(fs.readFileSync(cfile));editContribution(c);fs.writeFileSync(cfile,JSON.stringify(c));}const output=folder+'.laplug';sdk('pack',folder,output,key);return output;}
let child,exited,base,token,stderr='';
async function boot(exe=binary){token=randomUUID();child=spawn(exe,['--data-dir',data],{env:{...process.env,LOCAL_AGENT_SOURCE_HOME:home,LOCAL_AGENT_SERVICE_TOKEN:token},stdio:['ignore','pipe','pipe']});exited=once(child,'exit');child.stderr.on('data',b=>stderr+=b);const lines=createInterface({input:child.stdout});const [line]=await Promise.race([once(lines,'line',{signal:AbortSignal.timeout(15000)}),exited.then(()=>{throw Error(stderr)})]);base='http://127.0.0.1:'+JSON.parse(line).port;}
async function api(route,body,status=200){const r=await fetch(base+route,{method:body===undefined?'GET':'POST',headers:{authorization:'Bearer '+token,'content-type':'application/json'},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(15000)});const v=await r.json();assert.equal(r.status,status,JSON.stringify(v));return v;}
async function stop(){if(child?.exitCode===null){await api('/api/runtime/shutdown',{});await exited;}}
async function install(file){const p=await api('/api/plugins/inspect',{path:file});return api('/api/plugins/install',{packageRef:p.packageRef,digest:p.digest,approvePublisher:true});}
const state=(p,action,status)=>api('/api/plugins/state',{pluginId:p.manifest.id,digest:p.digest,action},status);
const action=(p,kind)=>api('/api/plugins/action',{pluginId:p.manifest.id,digest:p.digest,action:{kind}});
const view=(p,id,status)=>api('/api/plugins/view',{pluginId:p.manifest.id,viewId:id,cursor:null},status);
const helperId='net.fixture.helper';
try{
 await boot();const original=await api('/api/snapshot');assert.equal((await api('/api/sessions')).total,0);
 const file=pack('detail-report',()=>{},c=>{c.optionalDependencies[0].pluginId=helperId;});
 let p=await install(file);assert(!p.enabled);p=await state(p,'enable');
 let list=await api('/api/plugins');const contribution=()=>list.contributions.items.find(c=>c.digest===p.digest);
 assert.equal(list.contributions.schemaVersion,1);assert.equal(contribution().slots.length,6);assert.equal(contribution().dependencies[0].available,false);
 await view(p,'summary');assert.equal((await view(p,'enhanced',400)).code,'dependency_missing');
 await action(p,'authorize-report');await action(p,'refresh');let current=(await api('/api/plugins')).items.find(s=>s.digest===p.digest);assert.equal((await view(p,'summary')).report.requests,'0');
 passed('缺失可选依赖不阻止安装或启用；仅指定视图降级，基础摘要可授权读取');
 let helper=await install(pack('detail-helper',m=>{m.id=helperId;m.publisher='net.fixture';}));helper=await state(helper,'enable');
 assert.equal((await view(p,'enhanced')).report.requests,'0');await state(helper,'disable');
 list=await api('/api/plugins');let kept=list.items.find(s=>s.digest===p.digest);assert(kept.enabled&&kept.reportAuthorized);assert.equal(kept.grantRevision,current.grantRevision);assert.equal((await view(p,'enhanced',400)).code,'dependency_missing');assert.equal((await view(p,'summary')).report.requests,'0');
 helper=await state(helper,'enable');let upgraded=await install(pack('detail-helper',m=>{m.id=helperId;m.publisher='net.fixture';m.version='2.0.0';}));upgraded=await state(upgraded,'enable');list=await api('/api/plugins');assert.equal(contribution().dependencies[0].reason,'version-mismatch');assert.equal((await view(p,'enhanced',400)).code,'dependency_missing');
 helper=await state(upgraded,'rollback');assert.equal((await view(p,'enhanced')).report.requests,'0');
 passed('依赖停用与主版本不匹配仅降级；恢复与回退恢复视图，消费者启用和原授权不改变');
 const required=await install(pack('detail-helper',m=>{m.id='org.example.required';m.requires=['plugin:'+helperId];}));await state(required,'enable');await state(helper,'disable');list=await api('/api/plugins');assert(!list.items.find(s=>s.digest===required.digest).enabled);assert(list.items.find(s=>s.digest===p.digest).enabled);helper=await state(helper,'enable');
 passed('原必需依赖仍阻断或反向停用；可选依赖不改变此契约');
 const a=await install(pack('detail-report',m=>{m.id='org.example.optional-a';},c=>{c.optionalDependencies[0].pluginId='org.example.optional-b';}));
 const b=await install(pack('detail-report',m=>{m.id='org.example.optional-b';},c=>{c.optionalDependencies[0].pluginId='org.example.optional-a';}));
 await state(a,'enable');assert.equal((await state(b,'enable',400)).code,'dependency_cycle');
 passed('已存在的可选依赖参与循环检查，失败候选不影响活动版本');
 // A new host-required feature is refused by the old checker, never silently ignored.
 if(previous){assert.throws(()=>execFileSync(previous,['--plugin-sdk','check',file],{stdio:'pipe'}));await stop();await boot(previous);assert.equal((await api('/api/snapshot')).settings.retentionDays,original.settings.retentionDays);const unsupported=await view(p,'summary',400);assert(['unsupported','permission_denied'].includes(unsupported.code));await stop();await boot();p=(await api('/api/plugins')).items.find(s=>s.digest===p.digest);assert.equal(p.runtimeStatus,'failed');p=await state(p,'enable');assert(!p.reportAuthorized);await action(p,'authorize-report');await action(p,'refresh');passed('旧 0.1.2 明确拒绝新宿主特性；同数据返回新版需明确重启和重新授权，原功能保持');}
 await stop();await boot();list=await api('/api/plugins');assert(list.items.find(s=>s.digest===p.digest).enabled);await view(p,'summary');
 await state(helper,'revoke-publisher');list=await api('/api/plugins');assert(list.items.find(s=>s.digest===p.digest).enabled);assert.equal((await view(p,'enhanced',400)).code,'dependency_missing');
 const snap=await api('/api/snapshot');assert.deepEqual(snap.providers,original.providers);assert.deepEqual(snap.settings,original.settings);assert.equal((await api('/api/sessions')).total,0);
 passed('重开、撤销依赖发布者与不同目标插槽保持隔离；原数据、来源和配置不变');
 assert.equal(stderr.includes('panic'),false);await stop();report.complete=true;
}finally{try{await stop();}finally{fs.rmSync(key,{force:true});report.temporarySigningKeyRemoved=!fs.existsSync(key);report.processesExited=child?.exitCode!==null;if(process.env.PLUGIN_CONTRIBUTIONS_REPORT)fs.writeFileSync(process.env.PLUGIN_CONTRIBUTIONS_REPORT,JSON.stringify(report,null,2)+'\n');}}
