import assert from 'node:assert/strict';
import {mkdtempSync,realpathSync,mkdirSync,rmSync,writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
import {once} from 'node:events';
import {randomUUID} from 'node:crypto';
import {DatabaseSync} from 'node:sqlite';
const dir=realpathSync(mkdtempSync(path.join(tmpdir(),'request-sources-'))),token=randomUUID();
const home=path.join(dir,'source'),nativeDir=path.join(home,'.claude','projects','fixture');mkdirSync(nativeDir,{recursive:true});mkdirSync(path.join(dir,'observation'));
const db=new DatabaseSync(path.join(dir,'observation','observations.sqlite3'));db.exec('CREATE TABLE model_observations(id TEXT PRIMARY KEY, started_at TEXT NOT NULL, revision INTEGER NOT NULL, value TEXT NOT NULL)');
const insert=db.prepare('INSERT INTO model_observations VALUES(?,?,?,?)');
const fixtureTime='2026-10-18T16:00:00.000Z';
for(let i=0;i<4;i++){
 const row={id:'g'+i,logicalId:null,agentId:'claude',agentProductId:'claude-code-cli',sessionId:null,responseId:i<2?'response'+i:'ambiguous',model:'claude-test',provider:'fixture-provider',providerId:'p',endpoint:'https://fixture.test',bindingRevision:1,startedAt:fixtureTime,endedAt:'2026-10-18T16:00:01Z',status:'success',httpStatus:200,ttftMs:100,durationMs:1000,usage:{inputTokens:35,outputTokens:2,totalTokens:37,cachedInputTokens:20,cacheWriteInputTokens:5},protocol:'messages',timingIssue:null,revision:1};
 insert.run(row.id,row.startedAt,1,JSON.stringify(row));
}
const fixtures=Array.from({length:48},(_,i)=>({type:'assistant',sessionId:'native-fixture',uuid:'line'+i,timestamp:i===47?undefined:fixtureTime,message:{id:i===2?'ambiguous':'response'+i,model:'claude-test',content:[{type:'text',text:'fixture-native-text'}],usage:{input_tokens:i===1?99:10,cache_read_input_tokens:20,cache_creation_input_tokens:5,output_tokens:2}}}));
writeFileSync(path.join(nativeDir,'fixture.jsonl'),[...fixtures,fixtures[0],{type:'user',sessionId:'native-fixture',uuid:'user',timestamp:fixtureTime,message:{content:'not a model call'}}].map(r=>JSON.stringify(r)).join('\n')+'\n');
let child,base,stderr='';
const request=(route,opts={})=>fetch(base+route,{...opts,headers:{authorization:`Bearer ${token}`,'content-type':'application/json'}});
const api=async(route,opts)=>{const r=await request(route,opts);assert.equal(r.status,200,await r.clone().text());return r.json()};
const dashboard=q=>api('/api/request-dashboard?'+new URLSearchParams(q));
try {
 child=spawn(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY??'target/debug/local-agent-service',['--data-dir',dir],{env:{...process.env,LOCAL_AGENT_SERVICE_TOKEN:token,LOCAL_AGENT_SOURCE_HOME:home,LOCAL_AGENT_TEST_MEMORY_VAULT:'1'},stdio:['ignore','pipe','pipe']});child.stderr.on('data',b=>stderr+=b);
 const lines=createInterface({input:child.stdout});const [line]=await Promise.race([once(lines,'line'),once(child,'exit').then(()=>{throw Error(stderr)}),new Promise((_,reject)=>setTimeout(()=>reject(Error('service timeout')),10000).unref())]);base='http://127.0.0.1:'+JSON.parse(line).port;
 assert.equal((await dashboard({source:'native'})).total,0);
 await api('/api/session-sources',{method:'PUT',body:JSON.stringify({agentId:'claude',enabled:true})});
 let native;
 for(let i=0;i<100;i++){native=await dashboard({source:'native',sort:'time'});if(native.total===48)break;await new Promise(r=>setTimeout(r,100));}
 assert.equal(native.total,48);assert.equal(native.items.length,40);assert.equal(native.summary.requests,48);assert.equal(native.summary.attempts,0);assert.equal(native.summary.successRate,null);assert.equal(native.summary.ttft.samples,0);assert.equal(native.summary.cost.unknownCalls,48);assert.equal(native.missingTime,1);
 assert.equal(native.sourceReport.gateway.requests,4);assert.equal(native.sourceReport.matchedNative,2);assert.equal(native.sourceReport.ambiguousNative,1);assert.equal(native.sourceReport.unlinkedNative,46);assert.equal(native.sourceReport.usageConflicts,1);
 assert.equal(native.sourceReport.linked.usage.totalTokens,74);assert.equal(native.heatmap.days.reduce((n,d)=>n+d.records,0),47);
 assert.ok(native.heatmap.days.some(d=>d.state==='unrecorded' && d.tokens===null));
 const frozen={...native.query,snapshot:native.snapshot};const next=await dashboard({...frozen,cursor:native.nextCursor});assert.equal(next.items.length,8);assert.ok(next.items.every(b=>native.items.every(a=>a.id!==b.id)));
 const csv=await api('/api/request-export?snapshot='+native.snapshot);assert.equal(csv.count,48);assert.ok(csv.csv.includes('模型响应记录'));assert.ok(csv.csv.includes('时间未知'));assert.ok(!csv.csv.includes('fixture-native-text'));
 const linked=await dashboard({source:'linked'});assert.equal(linked.total,2);assert.equal(linked.summary.usage.totalTokens,74);assert.equal(linked.summary.attempts,2);assert.equal(linked.items[0].nativeLinks.length,1);
 const day=await dashboard({source:'native',start:'2026-10-19T00:00:00+08:00',end:'2026-10-20T00:00:00+08:00'});assert.equal(day.total,47);assert.equal(day.missingTime,1);
 const n=await api('/api/request-detail?id='+encodeURIComponent(native.items[0].id));assert.equal(n.request.source,'native');assert.ok(n.request.native.eventId);assert.equal(n.request.attempts.length,0);
 const exportId=randomUUID();await api('/api/request-exports',{method:'POST',body:JSON.stringify({operationId:exportId,snapshot:native.snapshot})});
 await api('/api/request-exports/'+exportId+'?after=-1');
 await api('/api/session-sources',{method:'PUT',body:JSON.stringify({agentId:'claude',enabled:false})});
 assert.equal((await request('/api/request-exports/'+exportId+'/finish',{method:'POST',body:JSON.stringify({after:47,rows:48})})).status,409,'final confirmation must reject revoked consent');
 assert.equal((await request('/api/request-exports/'+exportId+'?after=-1')).status,409,'revoked export resources cannot resume');
 assert.equal((await request('/api/request-export?snapshot='+native.snapshot)).status,409);assert.equal((await request('/api/request-dashboard?'+new URLSearchParams(frozen))).status,409);
 assert.equal((await api('/api/request-detail?id='+encodeURIComponent(n.request.id))).request,null);assert.equal((await dashboard({source:'native'})).total,0);
 assert.equal((await request('/api/request-dashboard?source=all')).status,400);
 await api('/api/session-sources',{method:'PUT',body:JSON.stringify({agentId:'claude',enabled:true})});
 assert.equal((await request('/api/request-export?snapshot='+native.snapshot)).status,409,'re-enabling never revives a snapshot from a revoked authorization');
 console.log('通过：原生授权、48 条响应去重、未知时间、原生固定分页 / 导出、明确 ID 关联、重复身份与用量冲突、消费不叠加、上海日历热力图、撤权后快照与详情失效。');
} finally {db.close();if(child?.exitCode===null){const closed=once(child,'exit');child.kill();await closed;}rmSync(dir,{recursive:true,force:true});}
