import assert from "node:assert/strict";
import { normalizeSession } from "../sdk/plugins/examples/dsh-desktop/bridge/normalize.js";
const row = { sessionId: "s1", cwd: "/owned/project", updatedAt: 1794016200000, running: true, projections: { values: { title: "Owned DSH fixture" } } };
const page = { hasMore: true, records: [
  { type: "event", event: { seq: 1, time: row.updatedAt, type: "user/message", data: { content: [{ type: "text", text: "Owned user message" }] } } },
  { type: "event", event: { seq: 2, time: row.updatedAt, type: "assistant/attempt", data: { model: "fixture", status: "success", usage: { inputTokens: 12, outputTokens: 3, totalTokens: 15 } } } },
  { type: "event", event: { seq: 3, time: row.updatedAt, type: "assistant/message", data: { content: "Owned assistant message" } } },
] };
const records = normalizeSession(row, page, "2026-11-07T01:50:00Z");
assert.equal(records.filter((r) => r.fact.kind === "session").length, 1);
const session = records.find((r) => r.fact.kind === "session");
assert.equal(session.fact.value.title, "Owned DSH fixture");
assert.equal(session.fact.value.partial, true);
assert.equal(session.fact.value.usage.totalTokens, null);
assert.equal(session.conversation.length, 2);
assert.deepEqual(session.conversation.map((message) => message.turnId), ["1", "3"]);
const response = records.find((r) => r.fact.kind === "response").fact.value;
assert.equal(response.usage.totalTokens, "15");
assert.equal(response.reportedResponseId, null);
assert.equal(response.durationMs, null);
assert.equal(records.find((r) => r.fact.kind === "activity").fact.value.phase, "running");
assert.equal(normalizeSession({ ...row, running: false }, { records: [], hasMore: false }, "2026-11-07T01:50:00Z").at(-1).fact.value.phase, "idle");
const invalidUsage = normalizeSession(row, { records: [{ type: "event", event: { seq: 1, type: "assistant/attempt", data: { usage: { totalTokens: Number.MAX_SAFE_INTEGER + 1 } } } }] }, "2026-11-07T01:50:00Z");
assert.equal(invalidUsage.find((r) => r.fact.kind === "response").fact.value.usage.totalTokens, null);
const event = (seq, type, data) => ({ type: "event", event: { seq, time: row.updatedAt + seq, type, data } });
const observed = normalizeSession(row, { hasMore: false, records: [
  event(1, "user/message", { source: { kind: "user" }, content: "Real prompt" }),
  event(2, "user/message", { source: { kind: "runtime-context" }, content: "Runtime context" }),
  event(3, "assistant/message", { turn: 1, step: 1, message: { content: [], source: { model: "deepseek-flash" } } }),
  event(4, "tool/call", { turn: 1, step: 1, callId: "probe", name: "mcp__flashqa__probe", arguments: "private input" }),
  event(5, "tool/result", { message: { toolCallId: "probe", isError: false, content: "private tool result" } }),
  event(6, "tool/call", { callId: "pending", name: "mcp__flashqa__pending" }),
  event(7, "tool/call", { callId: "plain", name: "read" }),
  event(8, "tool/result", { message: { toolCallId: "unknown", isError: true } }),
] }, "2026-11-07T01:50:00Z");
assert.deepEqual(observed.find((r) => r.fact.kind === "session").conversation.map(({kind, role, text}) => [kind, role, text]), [
  ["message", "user", "Real prompt"], ["runtime-context", null, "Runtime context"],
]);
assert.deepEqual(observed.filter((r) => r.fact.kind === "mcp-execution").map((r) => r.fact.value), [
  { nativeId: "s1:mcp:4", serverRef: null, sessionRef: "s1", responseRef: "s1:response:3", toolName: "mcp__flashqa__probe", occurredAt: new Date(row.updatedAt + 4).toISOString(), durationMs: "1", outcome: "success" },
  { nativeId: "s1:mcp:6", serverRef: null, sessionRef: "s1", responseRef: null, toolName: "mcp__flashqa__pending", occurredAt: new Date(row.updatedAt + 6).toISOString(), durationMs: null, outcome: "unknown" },
]);
assert(!JSON.stringify(observed).includes("private tool result"));
assert(!JSON.stringify(observed).includes("private input"));
assert(!observed.some((r) => r.fact.kind === "mcp-catalog"));
console.log("DSH public-wire normalization: bounded messages, partial history, source-reported activity, nullable evidence and exact safe counters passed");
