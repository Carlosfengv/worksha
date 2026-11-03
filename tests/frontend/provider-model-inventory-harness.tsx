import { createRoot } from "react-dom/client";
import { flushSync } from "react-dom";
import { useState } from "react";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
import { IconProvider } from "@/lib/icon-context";
import { ProvidersPage } from "@/features/providers/page";
import { providersApi, providerSourceApi } from "@/features/providers/api";
import { accountsApi } from "@/features/accounts/api";
import { pluginsApi } from "@/features/plugins/api";
import type { Snapshot } from "@/app/types";
import type {
  Provider,
  ProviderUsage,
  ProviderUsageTotals,
} from "@/features/providers/types";
import "@/styles/globals.css";

const mode = new URLSearchParams(location.search).get("mode");
const logoModels = [
  "deepseek-v4-pro[1M]",
  "anthropic/claude-sonnet-4",
  "openai/gpt-4.1",
  "google/gemini-2.5-pro",
  "qwen/qwen3-32b",
  "mistral/mistral-large",
  "xai/grok-4",
  "meta-llama/llama-4",
  "unknown/model-v1",
].map((id) => ({
  id,
  name: id === "unknown/model-v1" ? "GPT-labelled unknown alias" : id,
}));
const provider: Provider = {
  id: "a",
  name: "连接 A",
  vendor: "custom",
  endpoint: "http://localhost:12345/v1",
  protocol: "chat-completions",
  configured: true,
  model: "shared",
  models: ["shared", "disabled"],
  disabledModels: ["disabled"],
  credentialRef: null,
  credentialLabel: null,
  revision: 1,
  category: "local",
};
const other = {
  ...provider,
  id: "b",
  name: "连接 B",
  endpoint: "http://localhost:12346/v1",
  models: ["shared"],
  disabledModels: [],
};
type InventoryFixture = {
  catalogReads: string[];
  usageQueries: ProviderUsage["query"][];
  denied: boolean;
  usageFailed: boolean;
  finishCatalog: (() => void) | null;
  changeStore: (storeId: string | undefined) => void;
  recover: () => void;
  backendStoreId: string | undefined;
};
const fixture: InventoryFixture = Object.assign(window, {
  providerModelsFixture: {
    catalogReads: [] as string[],
    usageQueries: [] as ProviderUsage["query"][],
    denied: mode === "denied",
    usageFailed: false,
    finishCatalog: null as (() => void) | null,
    changeStore: () => {},
    recover: notifyBackendRecovery,
    backendStoreId:
      mode === "source-mismatch-before"
        ? "6087597b-0625-42ea-8fd0-e51f6ad16d01"
        : "c9e027fe-4536-4d08-8a55-74e6cbaf6313",
  },
}).providerModelsFixture;
providersApi.writeProtocol = async (expectedStoreId) => {
  if (!expectedStoreId || expectedStoreId !== fixture.backendStoreId)
    throw Error("草稿属于另一个本机数据库，请连接原后台后核查。");
  return {
    version: 1,
    storeId: expectedStoreId,
    sequence: 0,
    receiptLimit: 256,
    receiptBytes: 2097152,
    pendingLimit: 4,
    pendingOperationIds: [],
  };
};
declare global {
  interface Window {
    providerModelsFixture: InventoryFixture;
  }
}
providersApi.providerStatus = async () => ({
  configs: [],
  transactions: [],
  verifications: [],
  runtimes: [],
  backupProtection: "fixture",
});
accountsApi.accounts = async () => ({
  accounts: [],
  asOf: new Date().toISOString(),
});
pluginsApi.list = async () => ({
  items: [],
  apiVersion: "1.0.0",
  runtime: "fixture",
  developmentMode: false,
  contributions: { schemaVersion: 1, items: [] },
});
providersApi.providerModels = async (input) => {
  fixture.catalogReads.push(input.endpoint);
  if (mode === "source-mismatch-after")
    fixture.backendStoreId = "6087597b-0625-42ea-8fd0-e51f6ad16d01";
  if (
    ["loading", "source-hold"].includes(mode ?? "") &&
    input.endpoint === provider.endpoint &&
    fixture.catalogReads.length === 1
  )
    await new Promise<void>((resolve) => {
      fixture.finishCatalog = resolve;
    });
  if (fixture.denied && input.endpoint === other.endpoint)
    throw Error("模型目录鉴权被拒绝");
  return {
    models:
      input.endpoint === provider.endpoint
        ? [
            {
              id: "shared",
              name: "Shared A",
              contextWindow: 131072,
              maxOutputTokens: 8192,
              capabilities: {
                tools: true,
                vision: false,
                reasoning: true,
                structuredOutput: null,
              },
            },
            ...(mode === "logos"
              ? logoModels
              : Array.from({ length: 62 }, (_, i) => ({
                  id: `remote-model-${String(i).padStart(3, "0")}`,
                  name: `Remote ${i}`,
                }))),
          ]
        : [
            {
              id: "shared",
              name: "Shared B",
              maxInputTokens: 200000,
              capabilities: {
                tools: null,
                vision: true,
                reasoning: false,
                structuredOutput: true,
              },
            },
          ],
    fetchedAt: new Date().toISOString(),
    sourceUrl: `${input.endpoint}/models`,
    truncated: mode === "truncated",
  };
};
providerSourceApi.state = async (query) => {
  await providersApi.writeProtocol(query.storeId);
  return {
    providerId: query.providerId,
    expectedRevision: query.expectedRevision,
    sourceRevision: null,
    kind: "builtin",
    choice: null,
    selection: null,
    authorized: true,
    policy: null,
    errorCode: null,
  };
};
providerSourceApi.read = async (query) => {
  const p = [provider, other].find((p) => p.id === query.providerId);
  if (!p) throw Error("Unknown owned provider");
  return providersApi.providerModels({
    endpoint: p.endpoint,
    protocol: p.protocol,
    vendor: p.vendor ?? "custom",
    credentialRef: p.credentialRef,
  });
};
function totals(
  calls: number,
  tokens: number | null,
  partial = false,
): ProviderUsageTotals {
  return {
    calls,
    errors: 0,
    inProgress: 0,
    unknownOutcomes: 0,
    cost:
      tokens === 0
        ? {
            actual: [{ amount: 0, currency: "USD", calls }],
            estimated: [],
            unknownCalls: 0,
          }
        : { actual: [], estimated: [], unknownCalls: calls },
    usage: {
      inputTokens: tokens,
      outputTokens: tokens === null ? null : 0,
      totalTokens: tokens,
      cachedInputTokens: null,
      cacheWriteInputTokens: null,
      reasoningOutputTokens: null,
    },
    reported: {
      inputTokens: partial ? calls - 1 : calls,
      outputTokens: tokens === null ? 0 : calls,
      totalTokens: tokens === null ? 0 : partial ? calls - 1 : calls,
      cachedInputTokens: 0,
      cacheWriteInputTokens: 0,
      reasoningOutputTokens: 0,
    },
    lastSeen: null,
    latencyP95Ms: null,
    latencySamples: 0,
    ttftP95Ms: null,
    ttftSamples: 0,
  };
}
providersApi.providerUsage = async (query) => {
  fixture.usageQueries.push(query);
  if (fixture.usageFailed) throw Error("用量读取失败");
  const empty = query.source === "native" || query.agent === "opencode";
  return {
    query,
    asOf: new Date().toISOString(),
    sourceAsOf: new Date().toISOString(),
    sourceState:
      query.source === "native"
        ? "disabled"
        : mode === "partial"
          ? "partial"
          : "ready",
    summary: totals(89, 1000),
    providers: [],
    unattributed: totals(0, null),
    selected: null,
    modelOptions: [],
    models: [],
    ...(mode === "legacy"
      ? {}
      : {
          providerModels: empty
            ? []
            : [
                {
                  providerId: "a",
                  model: "shared",
                  totals: totals(85, 1000, true),
                },
                { providerId: "b", model: "shared", totals: totals(1, 0) },
                { providerId: "a", model: "disabled", totals: totals(3, null) },
              ],
        }),
    calls: [],
    totalCalls: 89,
    nextCursor: "not-the-full-dataset",
    missingTime: 0,
  };
};
const data: Snapshot = {
  version: "fixture",
  databasePath: "temporary",
  gatewayStatus: "ready",
  agents: [],
  providers: mode === "empty" ? [] : [provider, other],
  settings: { captureContent: false, retentionDays: 30 },
  sessionSources: [],
  observationBindings: [],
};
function Harness() {
  const [storeId, setStoreId] = useState<string | undefined>(
    mode === "source-missing"
      ? undefined
      : "c9e027fe-4536-4d08-8a55-74e6cbaf6313",
  );
  // A source replacement is observed once the new snapshot is committed.
  fixture.changeStore = (value) => {
    fixture.backendStoreId = value;
    flushSync(() => setStoreId(value));
  };
  return (
    <IconProvider>
      <main className="min-w-0 p-4">
        <ProvidersPage
          data={{
            ...data,
            providerWriteProtocol: storeId
              ? {
                  version: 1,
                  storeId,
                  sequence: 0,
                  receiptLimit: 256,
                  receiptBytes: 2097152,
                  pendingLimit: 4,
                  pendingOperationIds: [],
                }
              : undefined,
          }}
          refresh={async () => undefined}
        />
      </main>
    </IconProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
