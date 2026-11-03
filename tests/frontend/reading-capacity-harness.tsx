import { useCallback, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { IconProvider } from "@/lib/icon-context";
import { ConversationReader } from "@/features/sessions/components/conversation-reader";
import { ReadingCache } from "@/features/sessions/reading-cache";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
import type {
  Conversation,
  ConversationEvent,
} from "@/features/sessions/types";
import { activitySessions } from "./activity-fixture";
import "@/styles/globals.css";

declare global {
  interface Window {
    readingCapacity: {
      reads: { id: string; cursor?: string; anchor?: string }[];
      hold: boolean;
      release: (() => void) | null;
      error: string | null;
      recover: () => void;
      measure: () => {
        sessions: number;
        events: number;
        serializedBytes: number;
        estimatedBytes: number;
        trail: number;
        bodies: number;
      };
    };
  }
}
Object.defineProperty(globalThis, "isTauri", {
  value: true,
  configurable: true,
});
const params = new URLSearchParams(location.search);
const text = "synthetic reading fixture ".repeat(
  Number(params.get("words") ?? 640),
);
window.readingCapacity = {
  reads: [],
  hold: false,
  release: null,
  error: null,
  recover: () => {},
  measure: () => ({
    sessions: 0,
    events: 0,
    serializedBytes: 0,
    estimatedBytes: 0,
    trail: 0,
    bodies: 0,
  }),
};
const fixture = window.readingCapacity;
let revision = 0;
fixture.recover = () => {
  revision++;
  notifyBackendRecovery();
};
function conversation(
  id: string,
  cursor?: string,
  anchor?: string,
): Conversation {
  const page = cursor
    ? Number(cursor.split(":")[1])
    : anchor
      ? Number(anchor.split("-page-")[1]?.split("-")[0] ?? 0)
      : 0;
  const session = {
    ...activitySessions()[0],
    id,
    agentId: "claude",
    title: id,
    nativeId: id,
    modelErrorCount: 0,
    latestFailureId: null,
  };
  return {
    session,
    total: Number(params.get("pages") ?? 20) * 40,
    nextCursor:
      page < Number(params.get("pages") ?? 20) - 1
        ? `snapshot:${page + 1}`
        : null,
    asOf: "2026-10-28T00:00:00Z",
    coverage: "native_history",
    warnings: [],
    items: Array.from({ length: 40 }, (_, index): ConversationEvent => ({
      id: `${id}-page-${page}-event-${index}`,
      nativeId: `${page}-${index}`,
      turnId: "fixture-turn",
      kind: index === 0 ? "tool" : "message",
      role: index === 0 ? null : "user",
      name: `page-${page}-event-${index}`,
      timestamp: null,
      endTime: null,
      status: "success",
      text: index === 0 ? null : `source-${revision} ${text}`,
      input: index === 0 ? "synthetic input" : null,
      output: index === 0 ? "synthetic output" : null,
      model: null,
      modelResponseId: null,
      provider: null,
      usage: null,
      rawUsage: null,
      callId: null,
      server: null,
      toolSource: null,
      durationMs: null,
      truncated: false,
      sourceLine: page * 40 + index,
      failure: null,
      agentProductId: null,
    })),
  };
}
mockIPC(async (name, args) => {
  if (name !== "get_conversation")
    throw Error(`unexpected fixture command: ${name}`);
  const input = (args ?? {}) as Record<string, unknown>;
  const id = String(input.sessionId);
  const cursor = input.cursor ? String(input.cursor) : undefined;
  const anchor = input.anchor ? String(input.anchor) : undefined;
  fixture.reads.push({ id, cursor, anchor });
  const response = conversation(id, cursor, anchor);
  if (fixture.hold)
    await new Promise<void>((resolve) => {
      fixture.release = resolve;
    });
  if (fixture.error)
    throw { code: fixture.error, message: "synthetic source unavailable" };
  return response;
});
function Harness() {
  const cache = useRef(new ReadingCache());
  const [id, setId] = useState("claude:fixture-0");
  const [visible, setVisible] = useState(true);
  const summary = useCallback(() => {}, []);
  fixture.measure = () => ({
    estimatedBytes: cache.current.usage().bytes,
    trail: Math.max(
      0,
      ...[...cache.current.values()].map(
        (state) => state.previous?.length ?? 0,
      ),
    ),
    bodies: cache.current.usage().bodies,
    sessions: cache.current.size,
    events: [...cache.current.values()].reduce(
      (sum, state) => sum + (state.data?.items.length ?? 0),
      0,
    ),
    serializedBytes: [...cache.current.values()].reduce(
      (sum, state) =>
        sum + new TextEncoder().encode(JSON.stringify(state)).byteLength,
      0,
    ),
  });
  return (
    <IconProvider>
      <main className="flex h-screen flex-col">
        <nav className="flex shrink-0 gap-2 p-2">
          <button onClick={() => setVisible((value) => !value)}>
            toggle-reader
          </button>
          {Array.from({ length: 10 }, (_, n) => (
            <button key={n} onClick={() => setId(`claude:fixture-${n}`)}>
              session-{n}
            </button>
          ))}
        </nav>
        {visible && (
          <ConversationReader
            key={id}
            id={id}
            anchor={params.get("anchor") ?? undefined}
            returnTo={params.get("returnTo") ?? undefined}
            cache={cache.current}
            onSummary={summary}
          />
        )}
      </main>
    </IconProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
