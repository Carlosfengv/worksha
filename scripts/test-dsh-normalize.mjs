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
console.log("DSH public-wire normalization: bounded messages, partial history, source-reported activity, nullable evidence and exact safe counters passed");
