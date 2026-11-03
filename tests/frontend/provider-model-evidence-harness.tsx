import { createRoot } from "react-dom/client";
import { ProviderModelInventory } from "@/features/providers/components/provider-model-inventory";
import { providersApi, providerSourceApi } from "@/features/providers/api";
import type {
  ModelDocumentation,
  Provider,
  ProviderModelInfo,
  ModelObservation,
} from "@/features/providers/types";
import "@/styles/globals.css";

const observationMode = new URLSearchParams(location.search).get(
  "observations",
);
providersApi.writeProtocol = async (storeId) => {
  if (storeId !== "c9e027fe-4536-4d08-8a55-74e6cbaf6313")
    throw Error("目录来源已变化");
  return {
    version: 1,
    storeId,
    sequence: 0,
    receiptLimit: 256,
    receiptBytes: 2097152,
    pendingLimit: 4,
    pendingOperationIds: [],
  };
};
const observation = (
  model: string,
  client: string,
  checks: ModelObservation["checks"],
): ModelObservation => ({
  model,
  client,
  clientVersion: client === "OpenCode" ? "1.18.13" : "2.1.310",
  checkedAt: "2026-10-26T05:39:24Z",
  checks,
});

const providers: Provider[] = (
  ["chat-completions", "responses", "messages"] as const
).map((protocol) => ({
  id: protocol,
  name: protocol,
  protocol,
  vendor: "deepseek",
  configured: true,
  endpoint:
    protocol === "messages"
      ? "https://api.deepseek.com/anthropic"
      : "https://api.deepseek.com",
  model: "deepseek-flash",
  models: [
    "deepseek-flash",
    "deepseek-v4-pro",
    "deepseek-flash[1M]",
    "deepseek-v4-pro[1m]",
    "unknown[1M]",
  ],
  credentialRef: null,
  credentialLabel: null,
  revision: 1,
  category: "cloud",
}));
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
  const provider = providers.find((p) => p.id === query.providerId);
  if (!provider) throw Error("Unknown owned evidence provider");
  return providersApi.providerModels({
    endpoint: provider.endpoint,
    protocol: provider.protocol,
    vendor: provider.vendor ?? "custom",
    credentialRef: provider.credentialRef,
  });
};
const checkedAt = "2026-10-26T00:00:00Z";
const expiresAt = "2026-11-09T00:00:00Z";
const evidence = (
  fields: ModelDocumentation["fields"],
  label: string,
  url: string,
): ModelDocumentation => ({ fields, label, url, checkedAt, expiresAt });
providersApi.providerModels = async ({ protocol }) => {
  const sources = [
    evidence(
      ["tools"],
      "工具支持",
      "https://api-docs.deepseek.com/quick_start/pricing/",
    ),
    evidence(
      protocol === "messages"
        ? ["structuredOutput"]
        : ["structuredOutput", "jsonOutput"],
      "输出格式",
      `https://api-docs.deepseek.com/${protocol === "messages" ? "guides/anthropic_api/" : protocol === "responses" ? "api/create-response/" : "api/create-chat-completion/"}`,
    ),
  ];
  const models: ProviderModelInfo[] = ["deepseek-flash", "deepseek-v4-pro"].map(
    (id) => ({
      id,
      name: id,
      contextWindow: 1048576,
      maxOutputTokens: 393216,
      capabilities: {
        tools: true,
        structuredOutput: protocol === "responses",
        jsonOutput: protocol === "messages" ? null : true,
        vision: id === "deepseek-flash",
        reasoning: true,
      },
      documentation: sources,
      ...(protocol === "messages"
        ? {
            contextAlias: {
              contextWindow: 1000000,
              upstreamContextWindow: 1048576,
              documentation: [
                evidence(
                  ["contextWindow"],
                  "DeepSeek 接入说明",
                  "https://api-docs.deepseek.com/quick_start/agent_integrations/claude_code/",
                ),
                evidence(
                  ["contextWindow"],
                  "Claude Code 上下文标记",
                  "https://code.claude.com/docs/en/model-config#context-window-and-auto-compaction",
                ),
              ],
            },
          }
        : {}),
    }),
  );
  return {
    models,
    fetchedAt: new Date().toISOString(),
    sourceUrl: "https://api.deepseek.com/models",
    truncated: false,
    ...(observationMode
      ? {
          observations: {
            state:
              observationMode === "unavailable"
                ? ("unavailable" as const)
                : ("ready" as const),
            models:
              observationMode === "unavailable"
                ? []
                : protocol === "chat-completions"
                  ? [
                      observation("deepseek-v4-pro", "OpenCode", [
                        {
                          field: "tools",
                          status: "passed",
                          detail: "真实读取及随机值核对通过。",
                        },
                        {
                          field: "jsonOutput",
                          status: "passed",
                          detail:
                            "实际发送 json_object；3 次中 2 次有效、1 次无效。",
                        },
                        {
                          field: "jsonReliability",
                          status: "unknown",
                          detail:
                            "首次无效输出保留；后续两次有效，稳定性未知。",
                        },
                      ]),
                    ]
                  : protocol === "messages"
                    ? [
                        observation("deepseek-v4-pro[1M]", "Claude Code", [
                          {
                            field: "tools",
                            status: "passed",
                            detail: "真实读取完成。",
                          },
                          {
                            field: "contextMarker",
                            status: "passed",
                            detail:
                              "客户端窗口 1,000,000；移除标记发送基础模型。",
                          },
                          {
                            field: "jsonOutput",
                            status: "unknown",
                            detail: "实际请求没有原生 JSON 模式参数。",
                          },
                        ]),
                        observation("deepseek-v4-pro[1M]", "OpenCode", [
                          {
                            field: "tools",
                            status: "passed",
                            detail: "首次路径错误，复测通过；保留首次失败。",
                          },
                          {
                            field: "contextMarker",
                            status: "unknown",
                            detail: "原 ID 接受；未验证自动解释标记。",
                          },
                        ]),
                      ]
                    : [],
          },
        }
      : {}),
  };
};
providersApi.providerUsage = async (query) => ({
  query,
  asOf: new Date().toISOString(),
  sourceAsOf: new Date().toISOString(),
  sourceState: "ready",
  summary: null!,
  providers: [],
  unattributed: null!,
  selected: null,
  modelOptions: [],
  models: [],
  providerModels: [],
  calls: [],
  totalCalls: 0,
  nextCursor: null,
  missingTime: 0,
});
createRoot(document.getElementById("root")!).render(
  <main className="min-w-0 p-4">
    <ProviderModelInventory
      providers={providers}
      storeId="c9e027fe-4536-4d08-8a55-74e6cbaf6313"
    />
  </main>,
);
