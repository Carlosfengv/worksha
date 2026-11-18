import assert from "node:assert/strict";
import { elapsedLabel, messagePerformance, recentRate, samplingRate } from "../src/features/observation/activity-performance.ts";
import { summarizeActivity } from "../src/features/observation/activity-summary.ts";

const now = Date.parse("2026-10-26T10:00:00Z");
const at = ms => new Date(now+ms).toISOString();
const sample = { at: now-2000, windowMs: 5000, tokensPerSecond: 68.2 };
const sampling = { at: now, windowMs: 5000, tokensPerSecond: 42, recent: sample, reported: null };
assert.equal(samplingRate(sampling, now, at(-60000)), 42);
assert.equal(samplingRate({ ...sampling, at: now-6000 }, now), null);
assert.equal(samplingRate({ ...sampling, tokensPerSecond: Infinity }, now), null);
assert.equal(samplingRate(sampling, now, at(-1000)), null);
assert.equal(recentRate(sample, at(-1000), now), null);
assert.equal(recentRate(sample, at(-60000), now), sample);

const native = (id, windowAt=now) => ({ agentId: "codex", agentProductId: "codex-desktop", id, sessionId: `codex:${id}`, messageId: "one", messageStartedAt: at(-60000), eventAt: at(0), phase: "thinking", model: "gpt-native", stale: false,
  messageUsage: { outputTokens: 1200, reasoningTokens: 800, at: at(-5000) },
  realtime: { state: "connected", firstTokenMs: 1000, averageTextTokensPerSecond: 25, sampling: { ...sampling, at: windowAt, recent: { ...sample, at: windowAt-100 } } },
});
const a = native("a");
let result = messagePerformance([], [a], at(-60000), now);
assert.equal(result.outputTokens, 1200);
assert.equal(result.reasoningTokens, 800); // Already included in output, never 2000.
assert.equal(result.averageTextRate, 25);
assert.equal(messagePerformance([], [a], at(-1000), now).outputTokens, null);
const gateways = [
  { stale: false, phase: "streaming", sampling, request: { id: "r1", agentId: "claude", agentProductId: "claude-code", sessionId: null, model: "m", status: "in_progress", startedAt: at(-60000), ttftMs: 500, usage: { outputTokens: 100, reasoningOutputTokens: 40 } } },
  { stale: false, phase: "streaming", sampling, request: { id: "r2", agentId: "claude", agentProductId: "claude-code", sessionId: null, model: "m", status: "in_progress", startedAt: at(-50000), ttftMs: 500, usage: { outputTokens: 200, reasoningOutputTokens: 60 } } },
];
result = messagePerformance([gateways[0], gateways[0], gateways[1]], [a], at(-60000), now);
assert.equal(result.outputTokens, 300);
assert.equal(result.reasoningTokens, 100);
assert.equal(result.estimated, false);
result = messagePerformance([{ ...gateways[0], request: { ...gateways[0].request, usage: {}, estimatedTextTokens: 23.5 } }], [], at(-60000), now);
assert.equal(result.outputTokens, 23.5);
assert.equal(result.estimated, true);
assert.equal(result.reasoningTokens, null);
const first = { ...gateways[0], request: { ...gateways[0].request, status: "success", revision: 2,
  usage: {inputTokens: 1000, cachedInputTokens: 800, outputTokens: 100, reasoningOutputTokens: 40},
  reportedCost: {amount: 0.01, currency: "USD", source: "fixture"}
}};
const second = { ...gateways[1], request: { ...gateways[1].request, status: "success", ttftMs: 600,
  usage: {inputTokens: 2000, cachedInputTokens: 1100, outputTokens: 200, reasoningOutputTokens: 60},
  reportedCost: {amount: 0.03, currency: "USD", source: "fixture"}
}};
const stream = { ...gateways[1], request: { ...gateways[1].request, id: "r3", startedAt: at(-40000), usage: {outputTokens: null}, estimatedTextTokens: 45 }};
result = messagePerformance([second, first, gateways[0], stream], [a], at(-60000), now);
assert.equal(result.firstRequest.ttftMs, 500);
assert.equal(result.requestCount, 3);
assert.equal(result.inputTokens, 3000);
assert.equal(result.cachedInputTokens, 1900);
assert.equal(result.outputTokens, 345);
assert.equal(result.reasoningTokens, 100);
assert.deepEqual(result.cost.actual, [{amount: 0.04, currency: "USD", calls: 2}]);
assert.equal(result.cost.unknownCalls, 1);
assert.equal(result.partial, true);
assert.equal(result.streamingEstimated, true);
assert.equal(elapsedLabel(at(-65000), null, now), "1 分 5 秒");
assert.equal(elapsedLabel(at(-65000), at(-30000), now), "35 秒");

let summary = summarizeActivity([], [a, native("b")], "all", [], now);
assert.equal(summary.rate, 84);
assert.equal(summary.samples, 2);
assert.equal(summary.windowMs, 5000);
summary = summarizeActivity([], [a, native("b", now-3000)], "all", [], now);
assert.equal(summary.rate, 42); // Different window endpoints cannot be summed.
assert.equal(summary.samples, 1);
summary = summarizeActivity([], [{...a, realtime: {...a.realtime, sampling: {...sampling, tokensPerSecond: null}}}], "all", [], now);
assert.equal(summary.rate, null);
assert.equal(summary.recentSample.at, sample.at);
assert.equal(summary.samples, 0); // Historical values never become live totals.
const next = { ...a, messageId: "two", messageStartedAt: at(-1000), realtime: {...a.realtime, sampling} };
summary = summarizeActivity([], [next], "all", [], now);
assert.equal(summary.recentSample, null);
assert.equal(summary.rate, null);
console.log("通过：5 秒窗口与历史采样分离、消息边界、并行窗口对齐、累计用量去重与思考包含关系、耗时与估算来源。");
