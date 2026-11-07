// DSH 0.2.2-rc.1 public session/list and session/events wire records only.
// No persistence files, credentials, Agent activation or execution operations.
import { createHash } from "node:crypto";
const PRODUCT = "net.worksha.dsh-desktop:desktop";
const decimal = (v) => Number.isSafeInteger(v) && v >= 0 ? String(v) : null;
const timestamp = (v) => {
  const n = typeof v === "number" ? v : Date.parse(v);
  return Number.isFinite(n) ? new Date(n).toISOString() : null;
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
export function normalizeSession(row, page, observedAt) {
  const values = row.projections?.values ?? {};
  const events = (page.records ?? []).filter((record) => record.type === "event").map((record) => record.event);
  const titleProjection = values.title;
  const title = typeof titleProjection === "string" ? titleProjection : titleProjection?.title;
  const conversation = [], responses = [];
  for (const event of events) {
    const data = event.data ?? {};
    const id = String(event.seq);
    // A durable assistant attempt is source evidence, never a host request ID.
    if (event.type === "assistant/attempt" || event.type === "assistant/message") responses.push({ fact: { kind: "response", value: {
      nativeId: `${row.sessionId}:response:${id}`, sessionRef: row.sessionId,
      productId: PRODUCT, occurredAt: timestamp(event.time),
      model: typeof data.message?.source?.model === "string" ? data.message.source.model : null,
      outcome: event.type === "assistant/message" && !data.interrupted ? "success" : "failure",
      usage: usageOf(data.usage), reportedResponseId: null, durationMs: null,
    } } });
    if (event.type === "user/message" || event.type === "assistant/message") {
      const content = data.content ?? data.message?.content;
      const text = typeof content === "string" ? content : Array.isArray(content) ? content.filter((part) => part.type === "text" && typeof part.text === "string").map((part) => part.text).join("\n") : null;
      if (text) conversation.push({ id, nativeId: id, turnId: id, kind: "message", role: event.type === "user/message" ? "user" : "assistant", status: "recorded", text: text.slice(0, 4096), input: null, output: null, model: null, responseId: null, usage: null });
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
  facts.push({ fact: { kind: "session", value: { nativeId: row.sessionId, productIds: [PRODUCT], unknownClient: false, title: title || `DSH ${row.sessionId}`, createdAt: null, updatedAt: timestamp(row.updatedAt), projectRef: projectId, usage: sessionUsage, partial: Boolean(page.hasMore) } }, conversation: conversation.slice(-40) });
  facts.push(...responses);
  facts.push({ fact: { kind: "activity", value: { nativeId: `${row.sessionId}:activity`, sessionRef: row.sessionId, responseRef: null, occurredAt: row.running ? observedAt : timestamp(row.updatedAt), phase: row.running ? "running" : "idle", durationMs: null } } });
  return facts;
}
