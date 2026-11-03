import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { accountsApi } from "@/features/accounts/api";
import { providersApi } from "@/features/providers/api";
import { ProvidersPage } from "@/features/providers/page";
import type { Snapshot } from "@/app/types";
import type { BillingAccount } from "@/features/accounts/types";
import type {
  ProviderUsage,
  ProviderUsageTotals,
} from "@/features/providers/types";
import { pluginsApi } from "@/features/plugins/api";
import "@/styles/globals.css";

const params = new URLSearchParams(location.search);
const mode = params.get("mode");
const now = Date.now();
const provider = {
  id: "api-a",
  name: "MyDeepSeek",
  vendor: "deepseek",
  endpoint: "https://api.deepseek.com",
  protocol: "chat-completions" as const,
  configured: true,
  credentialRef: null,
  credentialLabel: "测试关联账户",
  models: ["deepseek-v4-pro"],
  model: "deepseek-v4-pro",
  revision: 1,
  category: "cloud" as const,
};
const other = { ...provider, id: "api-b", name: "另一个计费连接" };
const apiAccount: BillingAccount = {
  id: "account-a",
  name: provider.name,
  accountType: "api",
  scope: "连接 A 的独立测试账户",
  providerId: provider.id,
  declaredFor: [],
  sourceRevision: "a",
  readable: true,
  officialUrl: null,
  state: mode === "stale" ? "stale" : "ready",
  error: null,
  snapshot: {
    accountType: "api",
    identity: null,
    plan: null,
    windows: [],
    money: [
      {
        label: "余额",
        kind: "balance",
        amount: "8.52",
        currency: "CNY",
        period: null,
        resetsAt: null,
      },
    ],
    asOf: new Date(now).toISOString(),
    validForSeconds: 300,
    source: "隔离夹具",
    notes: [],
    resetCredits: {
      availableCount: null,
      detailsComplete: false,
      expirations: [],
      accessMode: "official",
      reason: "未知",
    },
  },
};
const subscription: BillingAccount = {
  ...structuredClone(apiAccount),
  id: "native-codex",
  name: "Codex 订阅",
  accountType: "subscription",
  providerId: null,
  scope: "本机订阅 · 没有 API 连接归属",
  snapshot: {
    ...structuredClone(apiAccount.snapshot!),
    accountType: "subscription",
    plan: "Pro",
    money: [],
    windows: [
      {
        id: "five",
        label: "5 小时窗口",
        usedPercent: 80,
        remainingPercent: 20,
        resetsAt: now / 1000 + 180 * 60,
        durationMins: 300,
      },
      {
        id: "seven",
        label: "7 天窗口",
        usedPercent: 0,
        remainingPercent: 100,
        resetsAt: now / 1000 + 3 * 86400,
        durationMins: 10080,
      },
    ],
  },
};
const otherAccount = {
  ...structuredClone(apiAccount),
  id: "account-b",
  name: other.name,
  providerId: other.id,
};
otherAccount.snapshot!.money[0] = {
  ...otherAccount.snapshot!.money[0],
  amount: "12.34",
  currency: "USD",
};
if (mode === "unsupported") {
  otherAccount.state = "unsupported";
  otherAccount.readable = false;
  otherAccount.snapshot = null;
  otherAccount.error = "此连接没有已接入的账户查询接口";
}
if (mode === "unread") {
  apiAccount.snapshot = null;
  apiAccount.state = "not_loaded";
  subscription.snapshot = null;
  subscription.state = "not_loaded";
}
if (mode === "expires") subscription.snapshot!.validForSeconds = 2;
if (mode === "quota-unknown") {
  subscription.snapshot!.windows[0].remainingPercent = null;
  subscription.snapshot!.windows[0].resetsAt = null;
  subscription.snapshot!.windows[1].remainingPercent = 0;
  subscription.snapshot!.windows[1].resetsAt = now / 1000 - 1;
}
if (params.has("dark")) document.documentElement.classList.add("dark");
declare global {
  interface Window {
    providerOverviewFixture: {
      refreshes: number;
      directoryFailed: boolean;
      usageFailed: boolean;
      usageQueries: ProviderUsage["query"][];
      finishAccounts: (() => void) | null;
      finishUsage: (() => void) | null;
    };
  }
}
const fixture = (window.providerOverviewFixture = {
  refreshes: 0,
  directoryFailed: false,
  usageFailed: false,
  usageQueries: [] as ProviderUsage["query"][],
  finishAccounts: null as (() => void) | null,
  finishUsage: null as (() => void) | null,
});
accountsApi.accounts = async () => {
  if (mode === "loading")
    await new Promise<void>((resolve) => {
      fixture.finishAccounts = resolve;
    });
  if (fixture.directoryFailed) throw Error("账户目录读取失败");
  return {
    accounts: [apiAccount, otherAccount, subscription],
    asOf: new Date().toISOString(),
  };
};
accountsApi.refreshAccount = async (id) => {
  fixture.refreshes++;
  if (id === apiAccount.id) {
    apiAccount.state = "ready";
    apiAccount.snapshot = structuredClone(otherAccount.snapshot);
  }
  return {
    accounts: [apiAccount, otherAccount, subscription],
    asOf: new Date().toISOString(),
  };
};
providersApi.providerStatus = async () => ({
  configs: [],
  transactions: [],
  verifications: [],
  runtimes: [],
  backupProtection: "测试",
});
function usageTotals(b: boolean): ProviderUsageTotals {
  const totals: ProviderUsageTotals = {
    calls: b ? 1 : 291,
    errors: b ? 0 : 15,
    inProgress: 0,
    unknownOutcomes: 0,
    usage: {
      inputTokens: 11425171,
      outputTokens: 94967,
      totalTokens: 11520138,
      cachedInputTokens: null,
      cacheWriteInputTokens: null,
      reasoningOutputTokens: null,
    },
    reported: {
      inputTokens: 273,
      outputTokens: 273,
      totalTokens: 273,
      cachedInputTokens: 0,
      cacheWriteInputTokens: 0,
      reasoningOutputTokens: 0,
    },
    lastSeen: new Date(now).toISOString(),
    latencyP95Ms: null,
    latencySamples: 0,
    ttftP95Ms: null,
    ttftSamples: 0,
    cost: b
      ? {
          actual: [{ amount: 0, calls: 1, currency: "USD" }],
          estimated: [],
          unknownCalls: 0,
        }
      : {
          actual: [],
          estimated: [
            {
              amount: 11.93452178,
              calls: 273,
              currency: mode === "mismatch" ? "USD" : "CNY",
            },
          ],
          unknownCalls: mode === "mismatch" ? 0 : 18,
        },
    unknownCostReasons: b
      ? {}
      : {
          missing_usage_upstream_rejected: 13,
          missing_usage_unsupported_content: 2,
          missing_usage_client_cancelled: 3,
        },
  };
  if (b) {
    totals.usage.inputTokens = 4;
    totals.usage.outputTokens = 6;
    totals.usage.totalTokens = 10;
    totals.reported.inputTokens = 1;
    totals.reported.outputTokens = 1;
    totals.reported.totalTokens = 1;
  }
  if (mode === "no-calls") {
    totals.calls = 0;
    totals.errors = 0;
    totals.cost = { actual: [], estimated: [], unknownCalls: 0 };
    totals.unknownCostReasons = {};
  }
  return totals;
}
providersApi.providerUsage = async (query) => {
  fixture.usageQueries.push(structuredClone(query));
  if (mode === "usage-loading" && query.providerId == null)
    await new Promise<void>((resolve) => {
      fixture.finishUsage = resolve;
    });
  if (fixture.usageFailed) throw Error("调用统计读取失败");
  const totals = usageTotals(query.providerId === "api-b");
  return {
    query:
      mode === "wrong-scope" && query.providerId == null
        ? { ...query, model: "filtered-model" }
        : query,
    asOf: new Date().toISOString(),
    sourceAsOf: new Date().toISOString(),
    sourceState: mode === "source-partial" ? "partial" : "ready",
    summary: totals,
    providers: [
      { providerId: provider.id, totals: usageTotals(false) },
      { providerId: other.id, totals: usageTotals(true) },
    ],
    unattributed: totals,
    selected: query.providerId ? totals : null,
    modelOptions: [],
    models: [],
    calls: [],
    totalCalls: 291,
    nextCursor: "page-2",
    missingTime: 0,
  };
};
pluginsApi.list = async () => ({
  items: [],
  apiVersion: "1.0.0",
  runtime: "fixture",
  developmentMode: false,
  contributions: { schemaVersion: 1, items: [] },
});
const data: Snapshot = {
  version: "fixture",
  databasePath: "temporary",
  gatewayStatus: "ready",
  agents: [],
  providers: [provider, other],
  settings: { captureContent: false, retentionDays: 30 },
  sessionSources: [],
  observationBindings: [],
};
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <main className="min-w-0 p-4">
      <ProvidersPage
        data={data}
        refresh={async () => undefined}
        selectedId={params.has("detail") ? provider.id : undefined}
      />
    </main>
  </IconProvider>,
);
