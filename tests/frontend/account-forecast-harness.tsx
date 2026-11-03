import { createRoot } from "react-dom/client";
import { useState } from "react";
import { IconProvider } from "@/lib/icon-context";
import { accountsApi } from "@/features/accounts/api";
import type { BillingAccount } from "@/features/accounts/types";
import { ProviderDetail } from "@/features/providers/components/provider-detail";
import { providersApi } from "@/features/providers/api";
import type { Provider, ProviderUsage } from "@/features/providers/types";
import { pluginsApi } from "@/features/plugins/api";
import "@/styles/globals.css";

const now = Date.now();
const params = new URLSearchParams(location.search);
const mode = params.get("mode");
const provider: Provider = {
  id: "provider-a",
  name: "同名 Provider",
  vendor: "custom",
  endpoint: "https://example.invalid/v1",
  protocol: "messages",
  category: "custom",
  configured: true,
  model: "fixture-model",
  models: ["fixture-model"],
  credentialRef: null,
  credentialLabel: null,
  revision: 1,
};
const linked: BillingAccount = {
  id: "account-a",
  name: "已关联账户 A",
  accountType: "subscription",
  scope: "隔离账户快照",
  providerId: "provider-a",
  declaredFor: [],
  sourceRevision: "fixture",
  readable: true,
  officialUrl: null,
  state: "ready",
  error: null,
  snapshot: {
    accountType: "subscription",
    identity: null,
    plan: "pro",
    asOf: new Date(now - 1000).toISOString(),
    validForSeconds: mode === "expires" ? 3 : 300,
    source: "离线测试快照",
    windows: [
      {
        id: "five-hour",
        label: "5 小时窗口",
        usedPercent: 80,
        remainingPercent: 20,
        resetsAt: now / 1000 + 180 * 60,
        durationMins: 300,
      },
      {
        id: "seven-day",
        label: "7 天窗口",
        usedPercent: 0,
        remainingPercent: 100,
        resetsAt: now / 1000 + 3 * 86400,
        durationMins: 10080,
      },
      {
        id: "unknown",
        label: "未知用量窗口",
        usedPercent: null,
        remainingPercent: null,
        resetsAt: now / 1000 + 3600,
        durationMins: 300,
      },
      {
        id: "missing-duration",
        label: "缺少周期窗口",
        usedPercent: 42,
        remainingPercent: 58,
        resetsAt: now / 1000 + 3600,
        durationMins: null,
      },
    ],
    money: [],
    notes: [],
    resetCredits: {
      availableCount: null,
      detailsComplete: false,
      expirations: [],
      accessMode: "official",
      reason: "来源未提供券数量",
    },
  },
};
if (mode === "stale") linked.state = "stale";
if (mode === "reset") linked.snapshot!.windows[0].resetsAt = now / 1000 - 1;
if (mode === "unread") {
  linked.state = "not_loaded";
  linked.snapshot = null;
}
if (mode === "money" || mode?.startsWith("balance")) {
  linked.accountType = "api";
  linked.snapshot!.accountType = "api";
  linked.snapshot!.windows = [];
  linked.snapshot!.money = [
    {
      label: "账户余额",
      amount: mode === "money" ? "12.34" : "8.55",
      currency: mode === "money" ? "USD" : "CNY",
      kind: "balance",
      period: null,
      resetsAt: null,
    },
  ];
}
if (mode === "balance-stale") linked.state = "stale";
const other: BillingAccount = {
  ...structuredClone(linked),
  id: "account-b",
  name: "其他连接账户 B",
  providerId: "provider-b",
};
const native: BillingAccount = {
  ...structuredClone(linked),
  id: "native-account",
  name: "未关联本机订阅",
  providerId: null,
};
declare global {
  interface Window {
    accountForecastFixture: {
      refreshes: number;
      directoryFailed: boolean;
      finishLoad: (() => void) | null;
      usageQueries: ProviderUsage["query"][];
      usageFailed: boolean;
      finishUsage: (() => void) | null;
      accountLoaded: boolean;
    };
  }
}
const fixture = (window.accountForecastFixture = {
  refreshes: 0,
  directoryFailed: false,
  finishLoad: null as (() => void) | null,
  usageQueries: [] as ProviderUsage["query"][],
  usageFailed: false,
  finishUsage: null as (() => void) | null,
  accountLoaded: mode !== "balance-unread",
});
accountsApi.accounts = async () => {
  if (mode === "loading")
    await new Promise<void>((resolve) => {
      fixture.finishLoad = resolve;
    });
  if (fixture.directoryFailed || mode === "directory-error")
    throw new Error("账户目录暂不可用");
  return {
    accounts:
      mode === "empty"
        ? [other, native]
        : [
            !fixture.accountLoaded
              ? { ...linked, snapshot: null, state: "not_loaded" }
              : linked,
            other,
            native,
          ],
    asOf: new Date().toISOString(),
  };
};
accountsApi.refreshAccount = async () => {
  fixture.refreshes++;
  if (mode === "balance-unread") {
    fixture.accountLoaded = true;
    return {
      accounts: [linked, other, native],
      asOf: new Date().toISOString(),
    };
  }
  throw new Error("测试禁止外部账户读取");
};
providersApi.providerUsage = async (query) => {
  if (!mode?.startsWith("balance")) throw new Error("此夹具不读取调用记录");
  fixture.usageQueries.push(structuredClone(query));
  if (mode === "balance-loading")
    await new Promise<void>((resolve) => {
      fixture.finishUsage = resolve;
    });
  if (fixture.usageFailed) throw new Error("隔离调用统计读取失败");
  const stamp = new Date(Date.now() - 100).toISOString();
  const totals: NonNullable<ProviderUsage["selected"]> = {
    calls: mode === "balance-few" ? 1 : 2,
    errors: 0,
    inProgress: 0,
    unknownOutcomes: 0,
    usage: {
      inputTokens: 0,
      outputTokens: 0,
      cachedInputTokens: 0,
      cacheWriteInputTokens: 0,
      reasoningOutputTokens: 0,
      totalTokens: 0,
    },
    reported: {
      inputTokens: 2,
      outputTokens: 2,
      cachedInputTokens: 2,
      cacheWriteInputTokens: 2,
      reasoningOutputTokens: 2,
      totalTokens: 2,
    },
    lastSeen: stamp,
    latencyP95Ms: null,
    latencySamples: 0,
    ttftP95Ms: null,
    ttftSamples: 0,
    cost: {
      actual: [],
      estimated: [
        {
          amount: 7,
          currency: mode === "balance-mismatch" ? "USD" : "CNY",
          calls: mode === "balance-few" ? 1 : 2,
        },
      ],
      unknownCalls: 0,
    },
  };
  if (mode === "balance-unknown") {
    totals.cost.unknownCalls = 1;
    totals.calls = 3;
  }
  if (mode === "balance-cny") {
    totals.calls = 283;
    totals.cost = {
      actual: [],
      estimated: [{ amount: 11.90497846, currency: "CNY", calls: 265 }],
      unknownCalls: 18,
    };
    totals.unknownCostReasons = {
      missing_usage_upstream_rejected: 13,
      missing_usage_unsupported_content: 2,
      missing_usage_client_cancelled: 3,
    };
  }
  return {
    query: structuredClone(query),
    asOf: stamp,
    sourceAsOf: stamp,
    sourceState: "ready",
    summary: totals,
    providers: [{ providerId: query.providerId!, totals }],
    unattributed: totals,
    selected: totals,
    modelOptions: [],
    models: [],
    calls: [],
    totalCalls: totals.calls,
    nextCursor: null,
    missingTime: 0,
  };
};
pluginsApi.list = async () => ({
  items: [],
  apiVersion: "1.0.0",
  runtime: "wasmtime-36.0.19",
  developmentMode: false,
  contributions: { schemaVersion: 1, items: [] },
});
if (params.has("dark")) document.documentElement.classList.add("dark");
function Harness() {
  const [selected, setSelected] = useState(provider);
  return (
    <IconProvider>
      <main className="min-w-0 p-4">
        <button onClick={() => setSelected({ ...provider, id: "provider-b" })}>
          切换测试连接
        </button>
        <ProviderDetail
          key={selected.id}
          provider={selected}
          status={null}
          onEdit={() => undefined}
          onVerify={() => undefined}
          onUpdated={async () => undefined}
        />
      </main>
    </IconProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
