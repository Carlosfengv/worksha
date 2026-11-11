// DSH 0.2.2-rc.1 public session/list and session/events wire records only.
// No persistence files, credentials, Agent activation or execution operations.
import { createHash } from "node:crypto";
import { taskTiming } from "./timing.js";
const PRODUCT = "net.worksha.dsh-desktop:desktop";
const decimal = (v) => Number.isSafeInteger(v) && v >= 0 ? String(v) : null;
const timestamp = (v) => {
  const date = new Date(typeof v === "number" ? v : Date.parse(v));
  return Number.isFinite(date.getTime()) ? date.toISOString() : null;
};
const unknownUsage = () => ({ inputTokens: null, outputTokens: null, cachedInputTokens: null, cacheWriteInputTokens: null, reasoningOutputTokens: null, totalTokens: null });
function usageOf(value) {
  const usage = unknownUsage();
  if (!value || typeof value !== "object") return usage;
  for (const key of Object.keys(usage)) usage[key] = decimal(value[key]);
  if (usage.totalTokens === null && usage.inputTokens !== null && usage.outputTokens !== null) {
    const total = BigInt(usage.inputTokens) + BigInt(usage.outputTokens);
    if (total <= 18446744073709551615n) usage.totalTokens = String(total);
  }
  return usage;
}
export function normalizeSession(row, page, observedAt, live) {
  const values = row.projections?.values ?? {};
  const events = (page.records ?? []).filter((record) => record.type === "event").map((record) => record.event);
  const titleProjection = values.title;
  const title = typeof titleProjection === "string" ? titleProjection : titleProjection?.title;
  const conversation = [], responses = [];
  const mcp = new Map(), responseSteps = new Map();
  const updatedAt = timestamp(row.updatedAt);
  let lastActivity = updatedAt;
  for (const event of events) {
    const data = event.data ?? {};
    const id = String(event.seq);
    if (["assistant/message", "tool/result", "turn/end"].includes(event.type)) {
      const time = timestamp(event.time);
      if (time && (!lastActivity || time > lastActivity)) lastActivity = time;
    }
    // A durable assistant attempt is source evidence, never a host request ID.
    if (event.type === "assistant/message" && Number.isSafeInteger(data.turn) && Number.isSafeInteger(data.step))
      responseSteps.set(`${data.turn}:${data.step}`, `${row.sessionId}:response:${id}`);
    if (event.type === "assistant/attempt" || event.type === "assistant/message") responses.push({ fact: { kind: "response", value: {
      nativeId: `${row.sessionId}:response:${id}`, sessionRef: row.sessionId,
      productId: PRODUCT, occurredAt: timestamp(event.time),
      model: typeof data.message?.source?.model === "string" ? data.message.source.model : null,
      outcome: event.type === "assistant/message" && !data.interrupted ? "success" : "failure",
      usage: usageOf(data.usage), reportedResponseId: null, durationMs: null,
    } } });
    if (event.type === "tool/call" && typeof data.callId === "string" &&
        typeof data.name === "string" && /^mcp__[A-Za-z0-9_-]{1,32}__\S+$/.test(data.name) &&
        Buffer.byteLength(data.name) <= 200 && !mcp.has(data.callId)) {
      mcp.set(data.callId, {
        nativeId: `${row.sessionId}:mcp:${id}`,
        serverRef: null,
        sessionRef: row.sessionId,
        responseRef: responseSteps.get(`${data.turn}:${data.step}`) ?? null,
        toolName: data.name,
        occurredAt: timestamp(event.time),
        durationMs: null,
        outcome: "unknown",
      });
    }
    if (event.type === "tool/result" && typeof data.message?.isError === "boolean") {
      const execution = mcp.get(data.message.toolCallId);
      if (execution?.outcome === "unknown") {
        execution.outcome = data.message.isError ? "failure" : "success";
        const ended = timestamp(event.time);
        if (ended && execution.occurredAt)
          execution.durationMs = decimal(Date.parse(ended) - Date.parse(execution.occurredAt));
      }
    }
    if (event.type === "user/message" || event.type === "assistant/message") {
      const content = data.content ?? data.message?.content;
      const text = typeof content === "string" ? content : Array.isArray(content) ? content.filter((part) => part.type === "text" && typeof part.text === "string").map((part) => part.text).join("\n") : null;
      const runtime = event.type === "user/message" && data.source?.kind != null && data.source.kind !== "user";
      const attachments = event.type === "user/message" && !runtime && Array.isArray(content) &&
        content.some((part) => part.type === "image" || part.type === "file");
      const displayText = text?.trim() ? text : attachments ? "附件消息" : null;
      if (displayText) conversation.push({ id, nativeId: id, turnId: id, kind: runtime ? "runtime-context" : "message", role: runtime ? null : event.type === "user/message" ? "user" : "assistant", status: "recorded", text: displayText.slice(0, 4096), input: null, output: null, model: null, responseId: null, usage: null });
    }
  }
  const project = typeof row.cwd === "string" && row.cwd ? row.cwd : null;
  const projectId = project ? `cwd:${createHash("sha256").update(project).digest("hex")}` : null;
  const facts = [];
  if (project) facts.push({ fact: { kind: "project", value: { nativeId: projectId, name: project.split("/").filter(Boolean).at(-1) || project, pathHint: project } } });
  const sessionUsage = unknownUsage();
  if (!page.hasMore && responses.length) for (const key of Object.keys(sessionUsage)) {
    const values = responses.map((r) => r.fact.value.usage[key]);
    if (values.every((value) => value !== null)) {
      const total = values.reduce((sum, value) => sum + BigInt(value), 0n);
      if (total <= 18446744073709551615n) sessionUsage[key] = String(total);
    }
  }
  const recentConversation = conversation.slice(-40);
  const latestUser = recentConversation.findLast((event) => event.kind === "message" && event.role === "user");
  const timing = latestUser && taskTiming(events, latestUser.nativeId, row.running ? live : null);
  const { durationMs, ...activityTiming } = timing ?? {};
  facts.push({ fact: { kind: "session", value: { nativeId: row.sessionId, productIds: [PRODUCT], unknownClient: false, title: title || `DSH ${row.sessionId}`, createdAt: null, updatedAt, projectRef: projectId, usage: sessionUsage, partial: Boolean(page.hasMore) } }, conversation: recentConversation, ...(timing ? { activityTiming } : {}) });
  facts.push(...responses);
  for (const execution of mcp.values()) facts.push({ fact: { kind: "mcp-execution", value: execution } });
  facts.push({ fact: { kind: "activity", value: { nativeId: `${row.sessionId}:activity`, sessionRef: row.sessionId, responseRef: null, occurredAt: row.running ? observedAt : lastActivity, phase: row.running ? "running" : "idle", durationMs: row.running ? null : decimal(durationMs) } } });
  return facts;
}
