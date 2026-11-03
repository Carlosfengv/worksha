import { createRoot } from "react-dom/client";
import { useState } from "react";
import { IconProvider } from "@/lib/icon-context";
import { PluginDetailSlots } from "@/features/plugins/detail-slots";
import { pluginsApi, parsePluginList } from "@/features/plugins/api";
import type { Manifest, PluginStatus } from "@/features/plugins/types";
import raw from "../../sdk/plugins/examples/detail-report/package/manifest.json";
import c from "../../sdk/plugins/examples/detail-report/package/contributions.json";
import "@/styles/globals.css";
const mode = new URLSearchParams(location.search).get("mode") ?? "ready";
let available = !["degraded", "long-degraded", "resize-focus"].includes(mode),
  removed = false,
  version = 0;
const queries: string[] = [];
const mutations: string[] = [];
const p: PluginStatus = {
  manifest: raw as Manifest,
  digest: "d".repeat(64),
  enabled: true,
  runtimeStatus: "active",
  errorCode: null,
  activationEpoch: "1",
  grantRevision: "1",
  sourceAuthorized: false,
  connectionAuthorized: false,
  reportAuthorized: true,
  signingKey: "a".repeat(64),
  sourceLabel: null,
  connectionLabel: null,
  previousDigest: null,
  settings: {},
};
declare global {
  interface Window {
    slotQueries: string[];
    slotMutations: string[];
    setSlotDependency: (ready: boolean) => void;
    removeSlotPlugin: () => void;
    slotValidation: (data: unknown) => boolean;
  }
}
window.slotQueries = queries;
window.slotMutations = mutations;
window.setSlotDependency = (ready) => {
  available = ready;
  version++;
};
window.removeSlotPlugin = () => {
  removed = true;
};
const status = () =>
  structuredClone({ ...p, grantRevision: String(version + 1) });
pluginsApi.list = async () => {
  if (mode === "directory-error") throw Error("目录暂不可用");
  const packet = {
    items: removed ? [] : [status()],
    apiVersion: "1.0.0",
    runtime: "wasmtime-36.0.19",
    developmentMode: false,
    contributions:
      mode === "legacy"
        ? undefined
        : {
            schemaVersion: 1,
            items: removed
              ? []
              : [
                  {
                    pluginId: p.manifest.id,
                    digest: p.digest,
                    slots: structuredClone(c.detailSlots),
                    dependencies: [
                      {
                        ...c.optionalDependencies[0],
                        available,
                        selectedVersion: available ? "1.0.0" : null,
                        reason: available ? null : "not-enabled",
                      },
                    ],
                    errorCode: null,
                  },
                ],
          },
  };
  if (mode === "scoped" && packet.contributions)
    for (const slot of packet.contributions.items[0].slots)
      slot.objectIds = ["first-object"];
  if (mode === "long-degraded" && packet.contributions) {
    const contribution = packet.contributions.items[0];
    contribution.dependencies[0].pluginId = "net." + "a".repeat(96);
    for (const slot of contribution.slots)
      slot.title =
        slot.target + " " + (slot.placement === "card" ? "C" : "T").repeat(80);
    for (const target of ["agent", "provider", "account"] as const)
      contribution.slots.push({
        id: target + "-basic",
        target,
        objectIds: ["*"],
        placement: "tab",
        viewId: "summary",
        title: target + " " + "B".repeat(80),
      });
  }
  if (mode === "prototype" && packet.contributions)
    Object.defineProperty(packet.contributions.items[0].slots[0], "__proto__", {
      value: { html: "blocked" },
      enumerable: true,
    });
  if (mode === "invalid" && packet.contributions)
    packet.contributions.items[0].slots[0].viewId = "unknown-view";
  return parsePluginList(packet);
};
pluginsApi.query = async (q) => {
  queries.push(q.viewId);
  const index = queries.length;
  if (mode === "slow-object" && index === 1)
    await new Promise((r) => setTimeout(r, 1000));
  if (mode === "view-error") throw Error("此视图读取失败");
  const view = p.manifest.views!.find((v) => v.id === q.viewId)!;
  return {
    status:
      mode === "new-version-response"
        ? { ...status(), digest: "e".repeat(64) }
        : status(),
    view,
    confirmedEmpty: false,
    report: {
      requests: mode === "slow-object" && index === 1 ? "999" : "17",
      attempts: "17",
      inputTokens: "12",
      outputTokens: "3",
      totalTokens: "15",
      rangeStart: "2026-10-24T00:00:00Z",
      rangeEnd: "2026-10-24T01:00:00Z",
    },
  };
};
pluginsApi.action = async () => {
  mutations.push("action");
  throw Error("No automatic action permitted");
};
pluginsApi.change = async () => {
  mutations.push("change");
  throw Error("No automatic mutation permitted");
};
window.slotValidation = (data) => {
  try {
    const v = parsePluginList(data);
    return !v.contributionsError;
  } catch {
    return false;
  }
};
function Harness() {
  const [object, setObject] = useState("first-object");
  return (
    <>
      <h1>原有详情保持</h1>
      <button onClick={() => setObject("second-object")}>切换对象</button>
      {(["agent", "provider", "account"] as const).map((target) => (
        <PluginDetailSlots key={target} target={target} objectId={object} />
      ))}
    </>
  );
}
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    {mode === "resize-focus" ? (
      <div className="flex h-screen flex-col">
        <header className="shrink-0 px-6 py-4">受管窗口工具栏</header>
        <main className="min-h-0 flex-1 space-y-6 overflow-y-auto p-6">
          <Harness />
        </main>
      </div>
    ) : (
      <main className="space-y-6 p-6">
        <Harness />
      </main>
    )}
  </IconProvider>,
);
