import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { BuiltinPluginManager } from "@/features/plugins/builtin-manager";
import { builtinPluginResource } from "@/features/plugins/queries";
import {
  builtinCatalog,
  builtinReply,
} from "@/features/plugins/builtin-contract";
import { toggleBuiltin } from "@/features/plugins/builtin-plan";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { createReadResource } from "@/shared/lib/read-resource";
import type {
  CapabilityAddress,
  CompositionCatalog,
  CompositionChange,
  CompositionReceipt,
  PluginInfo,
} from "../../contracts/plugins/builtin/types";
import "@/styles/globals.css";
const mode = new URLSearchParams(location.search).get("mode") ?? "ready";
const storeId = "10000000-0000-4000-8000-000000000001";
const key = (name: string): CapabilityAddress => ({
  name,
  version: 1,
  instance: "native",
});
function plugin(
  id: string,
  name: string,
  requires: CapabilityAddress[] = [],
): PluginInfo {
  return {
    id,
    displayName: name,
    version: "1.0.0",
    executionKind: "builtin",
    desiredEnabled: true,
    runtimeActive: true,
    activationEpoch: "1",
    capabilities: [
      {
        address: key(id),
        metadata: {
          kind: id === "activity" ? "stream" : "query",
          resourceScope: "plugin-instance",
        },
      },
    ],
    requires,
    optionalRequires: [],
    resources: [],
    problem: null,
  };
}
let catalog: CompositionCatalog = {
  schemaVersion: 1,
  storeId,
  revision: "0",
  composition: { profile: "desktop", disabledPluginIds: [] },
  effectiveProfile: "desktop",
  host: {
    generation: "20000000-0000-4000-8000-000000000001",
    cleanupFailed: false,
    plugins: [
      plugin("source.codex", "Codex 本机来源"),
      plugin("project.source", "项目来源读取"),
      plugin("project.assets", "项目资料读取", [key("project.source")]),
      plugin("activity", "实时活动采集"),
      ...Array.from({ length: 12 }, (_, i) =>
        plugin("account." + i, "测试账户 " + i),
      ),
    ],
  },
  problem: null,
};
catalog.host.plugins[3].capabilities[0].address = key("collector");
let receipts: Record<string, CompositionReceipt> = {};
const saved = sessionStorage.getItem("builtin-test-backend");
if (saved) ({ catalog, receipts } = JSON.parse(saved));
if (mode === "recovery") {
  catalog.composition.profile = "recovery";
  catalog.effectiveProfile = "recovery";
  for (const plugin of catalog.host.plugins) {
    plugin.desiredEnabled = false;
    plugin.runtimeActive = false;
  }
}
if (mode === "degraded") {
  catalog.problem = "来源启动失败，请恢复组合";
  catalog.host.plugins[0].runtimeActive = false;
  catalog.host.plugins[0].problem = catalog.problem;
}
if (mode === "background") {
  catalog.composition = {
    profile: "background",
    disabledPluginIds: ["source.codex"],
  };
  catalog.effectiveProfile = "background";
  for (const p of catalog.host.plugins) {
    if (p.id === "activity" || p.id === "source.codex")
      p.desiredEnabled = p.runtimeActive = false;
  }
}
// Keep an unrelated on-demand business reader mounted through the switch.
// It has no timer that could hide a missing lifecycle invalidation.
const businessRead = createReadResource(
  async () => {
    if (catalog.effectiveProfile === "recovery")
      throw Error("应用功能暂时暂停");
    return "桌面业务已恢复读取";
  },
  { interval: false },
);
function BusinessConsumer() {
  const read = useSharedRead(businessRead);
  return (
    <section aria-label="业务读取状态">
      {read.error ?? read.data ?? "读取中"}
    </section>
  );
}
const calls: { command: string; input?: unknown }[] = [];
const persist = () =>
  sessionStorage.setItem(
    "builtin-test-backend",
    JSON.stringify({ catalog, receipts }),
  );
let badReceipt = false;
declare global {
  interface Window {
    builtinCalls: typeof calls;
    bumpBuiltin: (change?: boolean) => Promise<void>;
    corruptBuiltinReceipt: () => void;
    builtinCheckReceipt: (
      request: CompositionChange,
      receipt: unknown,
    ) => boolean;
    builtinDraftCheck: () => boolean;
  }
}
window.builtinCalls = calls;
window.bumpBuiltin = async (change = true) => {
  if (change) {
    catalog.revision = String(BigInt(catalog.revision) + 1n);
    catalog.host.generation = crypto.randomUUID();
    persist();
  }
  await builtinPluginResource.reload();
};
window.corruptBuiltinReceipt = () => {
  badReceipt = true;
};
window.builtinCheckReceipt = (request, receipt) => {
  try {
    builtinReply({ receipt, catalog }, request);
    return true;
  } catch {
    return false;
  }
};
window.builtinDraftCheck = () => {
  const input = {
    profile: "background" as const,
    disabledPluginIds: ["activity"],
  };
  return toggleBuiltin(
    catalog.host.plugins,
    input,
    "source.codex",
    false,
  ).disabledPluginIds.includes("activity");
};
Object.defineProperty(globalThis, "isTauri", { value: true });
Object.defineProperty(window, "__TAURI_INTERNALS__", {
  value: {
    invoke: async (
      command: string,
      args: { input?: CompositionChange; query?: { operationId: string } } = {},
    ) => {
      calls.push({ command, input: structuredClone(args.input ?? args.query) });
      if (command === "get_builtin_plugins")
        return structuredClone(builtinCatalog(catalog));
      if (command === "configure_builtin_plugins") {
        const input = args.input!;
        if (mode === "rejected")
          throw {
            code: "stale_snapshot",
            message: "后台组合已变化",
            outcomeUnknown: false,
          };
        if (mode !== "unknown-before" && !receipts[input.operationId]) {
          catalog.revision = String(BigInt(catalog.revision) + 1n);
          catalog.composition = structuredClone(input.composition);
          catalog.effectiveProfile = input.composition.profile;
          catalog.host.generation = crypto.randomUUID();
          for (const p of catalog.host.plugins) {
            p.desiredEnabled =
              input.composition.profile !== "recovery" &&
              !(
                input.composition.profile === "background" &&
                p.id === "activity"
              ) &&
              !input.composition.disabledPluginIds.includes(p.id);
            p.runtimeActive = p.desiredEnabled;
            p.activationEpoch = String(BigInt(p.activationEpoch) + 1n);
          }
          receipts[input.operationId] = {
            operationId: input.operationId,
            storeId,
            revision: catalog.revision,
            composition: structuredClone(input.composition),
          };
          persist();
        }
        if (mode.startsWith("unknown"))
          throw {
            code: "unavailable",
            message: "响应中断，结果待核查",
            outcomeUnknown: true,
          };
        return structuredClone({
          receipt: receipts[input.operationId],
          catalog,
        });
      }
      if (command === "get_builtin_plugin_operation") {
        const receipt = receipts[args.query!.operationId] ?? null;
        return structuredClone({
          receipt:
            receipt && badReceipt
              ? { ...receipt, storeId: crypto.randomUUID() }
              : receipt,
          catalog,
        });
      }
      throw Error("unexpected IPC: " + command);
    },
  },
});
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <main className="p-4">
      <BuiltinPluginManager />
      {mode === "recovery" && <BusinessConsumer />}
    </main>
  </IconProvider>,
);
