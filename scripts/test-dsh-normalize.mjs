import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { apply } from "../sdk/plugins/examples/dsh-desktop/bridge/index.js";
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

const directory = await fs.mkdtemp(path.join(os.tmpdir(), "worksha-dsh-events-"));
const callbacks = new Map();
let running = true;
let stop;
const step = { turn: 1, step: 0 };
apply({
  on(name, callback) { callbacks.set(name, callback); },
  effect(start) { stop = start(); },
  sessionController: {
    async list() { return { items: [{ ...row, running }] }; },
    async events() {
      return {
        hasMore: false,
        records: [
          event(1, "step/start", step),
          event(2, "user/message", { content: [{ type: "image" }] }),
          ...(!running ? [
            event(10, "assistant/attempt", { ...step, stream: [{ type: "text-chunks", time0: row.updatedAt + 2, texts: ["", "retry"], dt: [2] }] }),
            event(24, "assistant/message", {
              ...step, stream: [{ type: "reasoning-chunks", time0: row.updatedAt + 8, texts: ["result"] }],
              usage: { outputTokens: 4 }, message: { source: { model: "deepseek-flash" } },
            }),
            event(25, "turn/end", { turn: 1 }),
          ] : []),
        ],
      };
    },
  },
}, { outputDirectory: directory });
const sessionRow = (rows) => rows.find((r) => r.fact?.kind === "session");
async function frame(phase, ready = () => true) {
  for (let i = 0; i < 25; i++) {
    const text = await fs.readFile(path.join(directory, "facts.jsonl"), "utf8").catch(() => "");
    try {
      const rows = text.trim().split("\n").map((line) => JSON.parse(line));
      if (rows[0].snapshot === rows.at(-1).commit && rows[0].records === rows.length - 2) {
        const activity = rows.find((row) => row.fact?.kind === "activity").fact.value;
        if (activity.phase === phase && ready(sessionRow(rows)?.activityTiming)) return { activity, rows };
      }
    } catch {
      // An in-place write may be in progress.
    }
    await delay(40);
  }
  assert.fail("DSH activity event was not published before the heartbeat");
}
try {
  const active = await frame("running");
  assert.equal(sessionRow(active.rows).conversation[0].text, "附件消息");
  assert.equal(sessionRow(active.rows).activityTiming.firstTokenMs, null);
  assert.equal(sessionRow(active.rows).activityTiming.messageStartedAtMs, row.updatedAt + 2);
  assert.equal(active.activity.durationMs, null);
  const stream = (frame) => callbacks.get("agent/assistant-stream")({
    agent: { session: { id: "s1", seq: 3 } }, frame: { ...frame, attemptId: "a1" },
  });
  stream({ type: "start", ...step });
  stream({ type: "chunk", time: row.updatedAt + 3, chunk: { type: "text-delta", text: "" } });
  stream({ type: "chunk", time: row.updatedAt + 4, chunk: { type: "reasoning-delta", text: "first" } });
  const first = await frame("running", (timing) => timing?.firstTokenMs === 3);
  assert.equal(sessionRow(first.rows).activityTiming.runningTokens, null);
  stream({ type: "chunk", time: row.updatedAt + 8, chunk: { type: "usage", usage: { outputTokens: 2 } } });
  const sampled = await frame("running", (timing) => timing?.runningTokens === "2");
  assert.equal(sessionRow(sampled.rows).activityTiming.runningDecodeMs, 4);
  running = false;
  stream({ type: "end" });
  callbacks.get("api-session/status")("s1", false);
  const completed = await frame("idle");
  assert.equal(completed.activity.occurredAt, new Date(row.updatedAt + 25).toISOString());
  assert.equal(completed.activity.durationMs, "23");
  assert.deepEqual(sessionRow(completed.rows).activityTiming, {
    messageNativeId: "2", messageStartedAtMs: row.updatedAt + 2,
    model: "deepseek-flash", firstTokenMs: 3, outputTokens: "4",
    completedTokens: "4", completedDecodeMs: 20, runningTokens: null, runningDecodeMs: null,
  });
  console.log("DSH bridge publishes TTFT before usage and preserves timed retry and completion measurements");
} finally {
  stop?.();
  await fs.rm(directory, { recursive: true, force: true });
}
