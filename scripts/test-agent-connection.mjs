import assert from "node:assert/strict";
import { connectionView } from "../src/features/agents/connection-display.ts";

const now = Date.parse("2026-10-19T00:00:00Z");
const item = {
  agentId: "claude",
  mode: "gateway",
  providerName: "Saved provider",
  model: "model",
  endpoint: "http://127.0.0.1:9/gateway/fixture",
  reason: null,
  activeRequests: 0,
};
const connections = {
  checkedAt: new Date(now).toISOString(),
  error: null,
  items: [item],
};
const view = (changes = {}) =>
  connectionView({ agentId: "claude", connections, now, ...changes });
assert.equal(view().mode, "gateway");
assert.equal(
  view().confirmed,
  false,
  "default gateway configuration does not prove current traffic",
);
assert.equal(view().gatewayConfigured, true);
assert.equal(view().title, "网关路由已确认");
assert.match(view().description, /没有进行中的网关请求/);
assert.equal(view({ live: { count: 2, stale: false } }).confirmed, true);
assert.equal(view({ live: { count: 2, stale: true } }).confirmed, false);
assert.equal(view({ live: { count: 2, stale: true } }).gatewayConfigured, true);
assert.match(view({ live: { count: 2, stale: true } }).description, /状态待更新/);
assert.doesNotMatch(view({ live: { count: 2, stale: true } }).description, /没有进行中/);
assert.equal(view({ live: { count: 2, stale: false } }).title, "网关正在代理 · 2 个请求");
assert.equal(
  view({
    connections: {
      ...connections,
      checkedAt: new Date(now - 16000).toISOString(),
    },
  }).mode,
  "unknown",
);
assert.equal(
  view({ connections: { ...connections, error: "unavailable" } }).mode,
  "unknown",
);
assert.equal(view({ connections: undefined }).mode, "unknown");
assert.equal(view({ connections: undefined }).gatewayConfigured, false);
assert.equal(
  view({connections:{...connections,checkedAt:new Date(now-16000).toISOString()}}).gatewayConfigured,
  false,
  "expired configuration cannot confirm the default gateway route",
);
assert.equal(
  view({connections:{...connections,error:"unavailable"}}).gatewayConfigured,
  false,
);
assert.equal(view({ agentId: "codex" }).mode, "unknown");
assert.equal(
  view({
    connections: {
      ...connections,
      items: [{ ...item, mode: "official_login" }],
    },
  }).loginNote.includes("尚未核验"),
  true,
);
assert.equal(
  view({
    connections: { ...connections, items: [{ ...item, mode: "direct" }] },
  }).confirmed,
  false,
);
assert.equal(
  view({connections:{...connections,items:[{...item,mode:"direct"}]}}).gatewayConfigured,
  false,
  "a local endpoint is not evidence that this app owns its route",
);
assert.match(
  view({
    connections: { ...connections, items: [{ ...item, mode: "direct" }] },
  }).description,
  /TTFT.*Token\/s.*缺失/,
);
assert.equal(
  view({
    connections: {
      ...connections,
      items: [{ ...item, mode: "official_login", activeRequests: 1 }],
    },
  }).confirmed,
  true,
  "one observed instance can use the gateway while the default is official login",
);
assert.equal(
  view({
    connections: { ...connections, items: [{ ...item, activeRequests: 1 }] },
    live: { count: 0, stale: false },
  }).confirmed,
  false,
  "fresh live evidence supersedes the older snapshot",
);
console.log(
  "通过：默认连接与实际流量分离、官方登录未核验、直连采集缺口、过期与错误降级、多实例不同路由及实时状态覆盖。",
);
const shared = {checkedAt:new Date(now).toISOString(),error:null,items:[{...item,agentId:"codex",activeRequests:2,activeByProduct:{"codex-cli":1,"codex-unknown":1}}]};
assert.equal(connectionView({agentId:"codex",productId:"codex-cli",connections:shared,now}).active,1);
assert.equal(connectionView({agentId:"codex",productId:"codex-desktop",connections:shared,now}).confirmed,false,"shared config and unknown traffic cannot be attributed to desktop");
const separate={...connections,items:[item,{...item,agentId:"claude-desktop",mode:"official_login"}]};
assert.equal(connectionView({agentId:"claude",productId:"claude-code-cli",connections:separate,now}).gatewayConfigured,true);
assert.equal(connectionView({agentId:"claude-desktop",productId:"claude-code-desktop",connections:separate,now}).gatewayConfigured,false,"a verified CLI default must not confirm the desktop profile");
