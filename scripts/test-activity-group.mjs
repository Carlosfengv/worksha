import assert from "node:assert/strict";
import {activityGroup} from "../src/features/observation/activity-group.ts";
import {summarizeActivity} from "../src/features/observation/activity-summary.ts";
import {latestSessionActivities} from "../src/features/observation/session-activities.ts";
import {agentProducts} from "../src/shared/types/agent.ts";
const now=Date.parse('2026-10-19T12:00:00Z');
const gateway=(id,speed=20,ttft=1000,session='codex:'+id)=>({
  request:{id,agentId:'codex',agentProductId:'codex-cli',sessionId:session,
    startedAt:new Date(now-5000).toISOString(),activityMessage:session?{id:'input:'+session,startedAt:new Date(now-5000).toISOString()}:null,status:'in_progress',ttftMs:ttft,timingIssue:null},
  stale:false,phase:'streaming',textTokensPerSecond:speed,elapsedMs:5000,
});
const native=(id,phase='processing',time=now)=>({id,sessionId:'codex:'+id,messageId:'input:codex:'+id,messageStartedAt:new Date(now-5000).toISOString(),agentId:'codex',agentProductId:'codex-cli',phase,eventAt:new Date(time).toISOString(),stale:false});
const group=(g=[],n=[],stale=false)=>activityGroup('codex-cli',g,n,stale,now);
let g=group([gateway('a'),gateway('b',40,3000)]);
assert.equal(g.count,2); assert.equal(g.speed,30); assert.equal(g.ttft,2000); assert.equal(g.speedSamples,2);
g=group([gateway('a',0,0),gateway('b',null,null),gateway('c',Number.NaN,-1)]);
assert.equal(g.count,3); assert.equal(g.speed,0); assert.equal(g.ttft,0); assert.equal(g.speedSamples,1);assert.equal(g.ttftSamples,1);
g=group([gateway('a',20,1000),{...gateway('b',500,500),phase:'tool_arguments'}, {...gateway('c'),stale:true}, {...gateway('d'),request:{...gateway('d').request,status:'success'}}, {...gateway('e',null),request:{...gateway('e').request,timingIssue:'partial stream'}}]);
assert.equal(g.count,3);assert.equal(g.speed,20); assert.equal(g.speedSamples,1);assert(Math.abs(g.ttft-2500/3)<1e-10);assert.equal(g.ttftSamples,3);
g=group([gateway('a'),gateway('b',40,3000,'codex:a'),gateway('a')],[native('a'),native('a'),native('other')]);
assert.equal(g.count,2);assert.equal(g.activeNative.length,1);assert.equal(g.activeGateway.length,2);assert.equal(g.speed,30);assert.equal(g.overlapUnknown,false);
g=group([gateway('unlinked',20,1000,null)],[native('a'),native('b')]);
assert.equal(g.count,2);assert.equal(g.overlapUnknown,true);assert.equal(g.activeNative.length,2);
g=group([],[native('a'),native('done','processing',now-1000),native('done','waiting_input'),native('old','processing',now-31000),native('future','processing',now+1000),native('reply','reply_recorded'),native('error','error')]);
assert.equal(g.count,1); assert.equal(g.speed,null);assert.equal(g.ttft,null);
assert.equal(group([gateway('a')],[native('b')],true).count,0);
g=group([{...gateway('desktop'),request:{...gateway('desktop').request,agentProductId:'codex-desktop'}},{...gateway('unknown'),request:{...gateway('unknown').request,agentProductId:null}},gateway('cli')]);
assert.equal(g.count,1);assert.equal(g.gateway[0].request.id,'cli');
g=group([gateway('a',Number.MAX_VALUE),gateway('b',Number.MAX_VALUE)]);assert(Number.isFinite(g.speed));
console.log('通过：并发活动平均、独立采样、真实零值、未知/过期/异常排除、共享 Session 去重、未确认关联下限、五 Agent 来源隔离。');

const summarize=(g=[],n=[],filter='all')=>summarizeActivity(g,n,filter,agentProducts,now);
const modelGateway=(id,rate=20,session='codex:a',model='model-a')=>({...gateway(id,rate,1000,session),request:{...gateway(id).request,sessionId:session,model}});
let summary=summarize([modelGateway('a'),modelGateway('a'),modelGateway('b',40)],[native('a'),native('a')]);
assert.equal(summary.sessions,1); assert.equal(summary.rate,60); assert.equal(summary.sources,2); assert.equal(summary.models[0].sessions,1); assert.equal(summary.partial,false);
summary=summarize([modelGateway('a'),modelGateway('b',40,'codex:a','model-b')],[native('a'),native('other')]);
assert.equal(summary.sessions,2); assert.equal(summary.rate,60); assert.equal(summary.models.length,3); assert.equal(summary.partial,true);
summary=summarize([modelGateway('unlinked',0,null)],[native('a')]);
assert.equal(summary.sessions,1); assert.equal(summary.unlinked,1); assert.equal(summary.rate,0); assert.equal(summary.partial,true);
summary=summarize([modelGateway('negative',-1),modelGateway('infinite',Infinity),{...modelGateway('thinking',100),phase:'public_reasoning'},{...modelGateway('tools',100),phase:'tool_arguments'}]);
assert.equal(summary.sessions,1); assert.equal(summary.rate,null); assert.equal(summary.samples,0);
summary=summarize([{...modelGateway('done'),request:{...modelGateway('done').request,status:'success'}},{...modelGateway('stale'),stale:true}],[native('old','thinking',now-30001),native('future','thinking',now+1),native('done','waiting_input')]);
assert.equal(summary.sessions,0);assert.equal(summary.rate,0);assert.equal(summary.models.length,0);
const first=native('session');
const second={...first,messageId:'new-message',messageStartedAt:new Date(now-1000).toISOString(),phase:'waiting_input'};
assert.equal(summarize([],[second,first]).sessions,0);
assert.equal(summarize([modelGateway('a')],[],'codex-desktop').sessions,0);
const unknown={...modelGateway('unknown'),request:{...modelGateway('unknown').request,agentProductId:null}};
assert.equal(summarize([unknown,modelGateway('a')],[],'unknown').sources,1);
summary=summarize([modelGateway('overflow-a',Number.MAX_VALUE),modelGateway('overflow-b',Number.MAX_VALUE)]);
assert.equal(summary.rate,null);assert.equal(summary.partial,true);
console.log('通过：汇总按 Session 去重、跨来源去重、同时请求速率求和、多模型、未关联请求、真实零/未知、终态/过期/未来排除、Agent 筛选与溢出边界。');

const displayRow=(id,sessionId,messageAt,requestAt=null,time=now)=>({id,sessionId,messageAt:messageAt==null?null:new Date(messageAt).toISOString(),requestAt:requestAt==null?null:new Date(requestAt).toISOString(),time:new Date(time).toISOString()});
const earlier=displayRow('message:a','codex:one',now-10000,null,now);
const later=displayRow('message:b','codex:one',now-5000,null,now-4000);
const other=displayRow('message:c','codex:two',now-7000);
const unlinkedA=displayRow('gateway:a',null,null,now-3000);
const unlinkedB=displayRow('gateway:b',null,null,now-2000);
const displayInput=[later,earlier,other,unlinkedA,unlinkedB];
assert.deepEqual(latestSessionActivities(displayInput),[later,other,unlinkedA,unlinkedB]);
assert.equal(displayInput.length,5);
assert.equal(latestSessionActivities([earlier,{...later,phase:'waiting_input'}])[0].id,later.id);
const missingTime=displayRow('message:new','codex:one',null,now-1000);
assert.equal(latestSessionActivities([later,missingTime])[0].id,missingTime.id);
const nativeMissingTime=displayRow('message:newer','codex:one',null,null,now);
assert.equal(latestSessionActivities([missingTime,nativeMissingTime])[0].id,nativeMissingTime.id);
const equalA=displayRow('message:a','codex:tie',now-1000,null,now);
const equalB=displayRow('message:b','codex:tie',now-1000,null,now-500);
assert.deepEqual(latestSessionActivities([equalA,equalB]),latestSessionActivities([equalB,equalA]));
assert.equal(latestSessionActivities([equalA,equalB])[0].id,'message:b');
console.log('通过：活动列表每 Session 仅最新消息、旧工具步骤不反超、终态不回退、缺失时间降级、等时稳定选择及未关联请求独立。');

const firstOutput=gateway('first-output',null,700,'codex:continued');
firstOutput.request.status='success';
const continued=gateway('continued',null,null,'codex:continued');
continued.request.startedAt=new Date(now-1000).toISOString();
continued.phase='waiting_first';
summary=summarize([continued,firstOutput]);
assert.equal(summary.ttft,700);
assert.equal(summary.models[0].pendingLabel,'等待模型响应');
for(const [phase,rate] of [['public_reasoning',31],['tool_arguments',49],['streaming',0]]){
  const live={...continued,phase,outputTokensPerSecond:rate};
  summary=summarize([live,firstOutput]);
  assert.equal(summary.rate,rate);assert.equal(summary.ttft,700);
  assert.equal(group([live,firstOutput]).ttft,700);
}
const nextMessage={...continued,request:{...continued.request,activityMessage:{id:'next-user',startedAt:continued.request.startedAt}}};
assert.equal(summarize([nextMessage,firstOutput]).ttft,null);
const missingFirst={...continued,request:{...continued.request,ttftMs:250,activityMessage:{...continued.request.activityMessage}}};
assert.equal(summarize([missingFirst]).ttft,250); // Current request timing, not an invented message first.
assert.equal(summarize([{...continued,phase:'public_reasoning',outputTokensPerSecond:NaN}]).rate,null);
console.log('通过：工具续调不重置消息 TTFT、思考/工具参数实时输出采样、真实零速率、等待状态与新消息指标隔离。');

const nativeDesktop={...native('desktop','thinking'),agentProductId:'codex-desktop',model:'native-model',realtime:{state:'connected',firstTokenMs:1050,textTokensPerSecond:null,outputTokensPerSecond:null}};
for(const phase of ['thinking','tool_requested','tool_returned','streaming']){
  for(const rate of [null,0,NaN,-1]){
    const unknown={...nativeDesktop,phase,realtime:{...nativeDesktop.realtime,outputTokensPerSecond:rate}};
    const card=activityGroup('codex-desktop',[],[unknown],false,now);
    assert.equal(card.speed,null);assert.equal(card.speedSamples,0);assert.equal(card.ttft,1050);
    const all=summarize([],[unknown]);
    assert.equal(all.rate,null);assert.equal(all.samples,0);assert.equal(all.sources,1);assert.equal(all.partial,true);
    const mixed=summarize([modelGateway('known',42,'codex:other')],[unknown]);
    assert.equal(mixed.rate,42);assert.equal(mixed.samples,1);assert.equal(mixed.sources,2);assert.equal(mixed.partial,true);
  }
  const measured={...nativeDesktop,phase,realtime:{...nativeDesktop.realtime,outputTokensPerSecond:12.5}};
  assert.equal(activityGroup('codex-desktop',[],[measured],false,now).speed,12.5);
  assert.equal(summarize([],[measured]).rate,12.5);
}
for(const rate of [0,20]){
  const legacy={...nativeDesktop,phase:'streaming',realtime:{state:'connected',firstTokenMs:1050,textTokensPerSecond:rate}};
  assert.equal(summarize([],[legacy]).rate,rate>0?rate:null);
}
console.log('通过：原生无增量不冒充零值，思考/工具/正文状态与 TTFT 保留，混合汇总仅计可测来源，网关真实零值不受影响。');
