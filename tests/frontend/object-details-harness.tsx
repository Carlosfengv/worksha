import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";
import { useState } from "react";
import { IconProvider } from "@/lib/icon-context";
import { ObjectContributionSlots } from "@/features/plugins/object-detail-slots";
import { objectContributionResource } from "@/features/plugins/queries";
import { sessionsApi } from "@/features/sessions/api";
import { pluginsApi } from "@/features/plugins/api";
import type { Manifest, PluginStatus } from "@/features/plugins/types";
import { PluginViewContent } from "@/features/plugins/view-content";
import objectManifest from "../../sdk/plugins/examples/object-summary/package/manifest.json";
import type {
  ObjectContributionAction as Action,
  ObjectContributionReply as Reply,
  ObjectContributionStatus,
  ObjectSelector,
  ObjectTarget,
  SummaryField,
  SummaryMetric,
} from "../../contracts/plugins/objects/types";
import "@/styles/globals.css";

const mode = new URLSearchParams(location.search).get("mode");
const pluginId = "org.example.objects";
const digest = "a".repeat(64);
const packageStatus: PluginStatus = {
  manifest: {
    ...objectManifest,
    id: pluginId,
  } as Manifest,
  digest,
  enabled: true,
  runtimeStatus: "active",
  errorCode: null,
  activationEpoch: "1",
  grantRevision: "1",
  signingKey: "a".repeat(64),
  reportAuthorized: false,
  sourceAuthorized: false,
  connectionAuthorized: false,
  settings: {},
  previousDigest: null,
  sourceLabel: null,
  connectionLabel: null,
};
pluginsApi.list = async () => ({
  apiVersion: "1.0.0",
  runtime: "wasmtime-36.0.19",
  developmentMode: false,
  items: [structuredClone(packageStatus)],
});
const targets: ObjectTarget[] = [
  "agent",
  "provider",
  "account",
  "project",
  "session",
  "request",
];
const fields: Record<ObjectTarget, SummaryField[]> = {
  agent: ["session-count"],
  provider: ["model-count", "configuration"],
  account: ["quota-window-count"],
  project: ["session-count"],
  session: ["input-tokens", "total-tokens"],
  request: ["total-tokens", "attempt-count", "duration-ms", "request-outcome"],
};
const contexts = new Map<string, number>();
const authorized = new Map<string, boolean>();
const refs = new Map<
  string,
  { slot: string; object: ObjectSelector; at: number }
>();
const latest = new Map<string, string>();
const held: (() => void)[] = [];
const hex = (n: number) => n.toString(16).padStart(64, "0");
const ctx = (slot: string) => hex(contexts.get(slot) ?? 1);
const calls: Action[] = [];
const commands: string[] = [];
let available = mode !== "degraded";
let sourceRevoked = false;
let hold = mode === "late-issue" || mode === "late-read";
const isAuthorized = (slot: string) =>
  authorized.get(slot) ?? !(mode === "unauthorized" && slot === "session-card");
const catalog = (): ObjectContributionStatus => ({
  pluginId,
  digest,
  enabled: true,
  active: true,
  activationEpoch: "1",
  grantRevision: "1",
  errorCode: null,
  dependencies: [
    {
      pluginId: "org.example.enhancer",
      version: "^1.0.0",
      views: ["enhanced"],
      available,
      selectedVersion: available ? "1.0.0" : null,
      reason: available ? null : "not-enabled",
    },
  ],
  slots: targets.flatMap((target) =>
    (["card", "tab"] as const).map((placement) => {
      const id = `${target}-${placement}`;
      return {
        authorized: isAuthorized(id),
        descriptorIdentity: "d".repeat(64),
        contextIdentity: ctx(id),
        errorCode: null,
        declaration: {
          id,
          target,
          placement,
          title: `${target} ${placement}`,
          viewId:
            target === "session" && placement === "tab"
              ? "enhanced"
              : `${target}-summary`,
          queryKind: "summary" as const,
          fields: fields[target],
          purpose: `显示当前${target}已记录的摘要字段。`,
        },
      };
    }),
  ),
});
function metrics(object: ObjectSelector): SummaryMetric[] {
  return fields[object.target].map((field) => {
    const value: SummaryMetric["value"] =
      field === "configuration"
        ? { kind: "configuration", configured: true }
        : field === "duration-ms"
          ? { kind: "milliseconds", value: 1.25 }
          : field === "request-outcome"
            ? { kind: "outcome", value: "succeeded" }
            : field === "attempt-count"
              ? null
              : field.endsWith("tokens")
                ? {
                    kind: "tokens",
                    value:
                      object.id === "session-second"
                        ? "42"
                        : "9007199254740993",
                  }
                : {
                    kind: "count",
                    value:
                      field === "model-count" || field === "quota-window-count"
                        ? "0"
                        : "2",
                  };
    return { field, value, missingReason: value ? null : "not-recorded" };
  });
}
(globalThis as { isTauri?: boolean }).isTauri = true;
mockIPC(async (command, args) => {
  commands.push(command);
  if (command === "set_session_source") {
    await new Promise((resolve) => setTimeout(resolve, 20));
    sourceRevoked = true;
    return;
  }
  if (command !== "perform_object_contribution")
    throw Error(`Unexpected command ${command}`);
  const action = (args as { input: Action }).input;
  calls.push(structuredClone(action));
  let reply: Reply;
  switch (action.kind) {
    case "catalog":
      reply = {
        kind: "catalog",
        catalog: { schemaVersion: 1, items: [catalog()] },
      };
      break;
    case "grant":
    case "revoke": {
      const request = action.request;
      if (
        request.expectedContextIdentity !== ctx(request.slot.slotId) ||
        request.expectedDescriptorIdentity !== "d".repeat(64) ||
        request.expectedActivationEpoch !== "1" ||
        request.expectedGrantRevision !== "1"
      )
        throw Error("授权上下文已变化");
      authorized.set(request.slot.slotId, action.kind === "grant");
      contexts.set(
        request.slot.slotId,
        (contexts.get(request.slot.slotId) ?? 1) + 1,
      );
      reply = { kind: "status", status: catalog() };
      break;
    }
    case "issue": {
      const request = action.request;
      if (
        request.expectedContextIdentity !== ctx(request.slot.slotId) ||
        !isAuthorized(request.slot.slotId)
      )
        throw Error("未授权摘要");
      const previous = latest.get(request.slot.slotId);
      if (previous) refs.delete(previous);
      const objectRef = crypto.randomUUID();
      refs.set(objectRef, {
        slot: request.slot.slotId,
        object: structuredClone(request.object),
        at: Date.now(),
      });
      latest.set(request.slot.slotId, objectRef);
      reply = {
        kind: "reference",
        reference: {
          slot: request.slot,
          contextIdentity: ctx(request.slot.slotId),
          objectRef,
          target: request.object.target,
          expiresAfterMs: "60000",
        },
      };
      if (
        hold &&
        mode === "late-issue" &&
        request.slot.slotId === "session-card" &&
        request.object.id === "session-first"
      )
        await new Promise<void>((resolve) => held.push(resolve));
      break;
    }
    case "read": {
      const saved = refs.get(action.query.objectRef);
      if (
        !saved ||
        saved.slot !== action.query.slot.slotId ||
        Date.now() - saved.at >= 60000
      )
        throw Error("对象引用已失效");
      if (sourceRevoked && saved.object.target === "session")
        throw Error("来源授权已撤销");
      reply = {
        kind: "summary",
        objectRef: action.query.objectRef,
        summary: {
          apiVersion: "1.0.0",
          target:
            mode === "wrong-target" && saved.slot === "session-card"
              ? "request"
              : saved.object.target,
          coverage: "complete",
          freshness: "current",
          capturedAt: new Date().toISOString(),
          metrics: metrics(saved.object),
        },
      };
      if (
        hold &&
        mode === "late-read" &&
        saved.slot === "session-card" &&
        saved.object.id === "session-first"
      )
        await new Promise<void>((resolve) => held.push(resolve));
      break;
    }
    case "release": {
      const saved = refs.get(action.query.objectRef);
      if (saved?.slot === action.query.slot.slotId)
        refs.delete(action.query.objectRef);
      if (latest.get(action.query.slot.slotId) === action.query.objectRef)
        latest.delete(action.query.slot.slotId);
      reply = { kind: "released", objectRef: action.query.objectRef };
      break;
    }
  }
  return structuredClone(reply);
});
const root = createRoot(document.getElementById("root")!);
const fixture = {
  calls,
  commands,
  held: () => held.length,
  release: () => {
    hold = false;
    for (const resolve of held.splice(0)) resolve();
  },
  dependency: async (ready: boolean) => {
    available = ready;
    await objectContributionResource.invalidate();
  },
  revokeSource: () => sessionsApi.setSessionSource("claude", false),
  changeContext: async () => {
    contexts.set("session-card", (contexts.get("session-card") ?? 1) + 1);
    await objectContributionResource.invalidate();
  },
  refs: () => refs.size,
  unmount: () => root.unmount(),
};
declare global {
  interface Window {
    objectHarness: typeof fixture;
  }
}
window.objectHarness = fixture;
function Harness() {
  const [session, setSession] = useState("session-first");
  const objects: ObjectSelector[] = targets.map((target) => {
    if (target === "agent" || target === "provider" || target === "account")
      return { target, id: `${target}-first` };
    if (target === "request")
      return {
        target,
        id: "request-first",
        unit:
          mode === "external-response" ? "model-response" : "logical-request",
        origin:
          mode === "external-response"
            ? {
                kind: "unified",
                scope: { kind: "external", sourceIds: ["org.example.history"] },
                sourceId: "org.example.history",
                snapshotToken: "owned-frozen-request-snapshot",
              }
            : { kind: "builtin" },
      };
    return {
      target,
      id: target === "session" ? session : "project-first",
      origin: { kind: "builtin" },
    };
  });
  if (mode === "standalone")
    return (
      <PluginViewContent
        status={packageStatus}
        viewId="session-summary"
        providers={[]}
        reloadDirectory={() => {}}
        refreshSnapshot={async () => {}}
        diagnosticsUnavailable={false}
      />
    );
  return (
    <>
      <h1>原有详情内容</h1>
      <button onClick={() => setSession("session-second")}>切换会话对象</button>
      {objects.map((object) => (
        <ObjectContributionSlots key={object.target} object={object} />
      ))}
    </>
  );
}
root.render(
  <IconProvider>
    <main className="space-y-6 p-6">
      <Harness />
    </main>
  </IconProvider>,
);
