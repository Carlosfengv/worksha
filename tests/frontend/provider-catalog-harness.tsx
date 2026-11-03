import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { ProviderCatalogControls } from "@/features/plugins/provider-catalog-controls";
import { providerCatalogApi } from "@/features/providers/api";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
import type { PluginStatus, Manifest } from "@/features/plugins/types";
import type {
  Binding,
  State,
} from "../../contracts/domain/provider-access/types";
import type { ProviderModelCatalog } from "@/features/providers/types";
import { fixtureProvider } from "./fixtures/tables/data";
import manifest from "../../sdk/plugins/examples/model-catalog/package/manifest.json";
import "@/styles/globals.css";
const mode = new URLSearchParams(location.search).get("mode") ?? "ready";
const providers = Array.from({ length: 35 }, (_, i) => fixtureProvider(i));
let bindings: Binding[] = providers.slice(0, 25).map((p) => ({
  bindingId: "binding-" + p.id,
  digest: "d".repeat(64),
  label: p.name,
  providerId: p.id,
  expectedRevision: "1",
  endpoint: p.endpoint,
  protocol: p.protocol,
  authorized: mode !== "disabled",
  allowPrivateNetwork: false,
  errorCode: mode === "disabled" ? "grant_revoked" : null,
}));
let recovered = false,
  finish: (() => void) | undefined;
const calls: string[] = [];
declare global {
  interface Window {
    catalogCalls: string[];
    finishCatalogRead: () => void;
    recoverCatalogBackend: () => void;
  }
}
window.catalogCalls = calls;
window.finishCatalogRead = () => finish?.();
window.recoverCatalogBackend = () => {
  recovered = true;
  notifyBackendRecovery();
};
const state = (): State => ({
  policy:
    mode === "disabled"
      ? null
      : { schemaVersion: 1, baseProtocol: "responses", path: "/catalog" },
  canBind: mode !== "disabled",
  bindings: structuredClone(
    bindings.map((b) => ({ ...b, authorized: b.authorized && !recovered })),
  ),
  errorCode: null,
});
providerCatalogApi.state = async () => {
  calls.push("metadata");
  return state();
};
providerCatalogApi.change = async (input) => {
  const action = input.action;
  calls.push(action.kind);
  if (action.kind === "unbind")
    bindings = bindings.filter((b) => b.bindingId !== action.bindingId);
  else {
    const target = action.target;
    if (target.kind !== "saved") throw Error("Unexpected fixture target");
    const p = providers.find((p) => p.id === target.providerId)!;
    bindings.push({
      bindingId: "new-" + p.id,
      digest: input.digest,
      label: p.name,
      providerId: p.id,
      expectedRevision: String(p.revision),
      endpoint: p.endpoint,
      protocol: p.protocol,
      authorized: true,
      allowPrivateNetwork: action.allowPrivateNetwork,
      errorCode: null,
    });
  }
  return state();
};
const catalog: ProviderModelCatalog = {
  sourceUrl: "http://127.0.0.1:9/v1/catalog",
  fetchedAt: "2026-11-01T00:00:00Z",
  truncated: false,
  models: Array.from({ length: 32 }, (_, i) => ({
    id: "model-" + i,
    name: "目录模型 " + i,
    contextWindow: i ? 8192 : null,
    maxInputTokens: null,
    maxOutputTokens: null,
    capabilities: {
      tools: i === 0 ? false : i === 1 ? null : true,
      vision: null,
      reasoning: null,
      structuredOutput: null,
      jsonOutput: null,
    },
  })),
};
providerCatalogApi.read = async () => {
  calls.push("read");
  if (mode === "error") throw Error("目录读取失败，原连接保持");
  if (mode === "late")
    await new Promise<void>((resolve) => {
      finish = resolve;
    });
  return mode === "empty" ? { ...catalog, models: [] } : catalog;
};
const status: PluginStatus = {
  manifest: manifest as Manifest,
  digest: "d".repeat(64),
  enabled: mode !== "disabled",
  runtimeStatus: "active",
  errorCode: null,
  activationEpoch: "1",
  grantRevision: "1",
  sourceAuthorized: false,
  connectionAuthorized: false,
  reportAuthorized: false,
  signingKey: "a".repeat(64),
  sourceLabel: null,
  connectionLabel: null,
  previousDigest: null,
  settings: {},
};
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <main className="p-4">
      <ProviderCatalogControls
        status={status}
        providers={providers}
        onChanged={() => calls.push("changed")}
      />
    </main>
  </IconProvider>,
);
