import { expect, test } from "@playwright/test";
import { providerModelCatalog } from "../../src/features/providers/catalog-response";
import {
  providerStatus,
  providerTransaction,
} from "../../src/features/providers/status-response";
import { providerUsage } from "../../src/features/providers/usage-response";
import { providerTokens } from "../../src/features/providers/usage-display";
import {
  providerEditorDraft,
  reduceProviderDraft,
} from "../../src/features/providers/editor-draft";
import {
  responseCatalog,
  responseProvider,
  responseStatus,
  responseTotals,
  responseTransaction,
  responseUsage,
} from "./provider-response-fixture";
import type {} from "./provider-response-harness";

function changed(value: unknown, path: string[], replacement: unknown) {
  const result = structuredClone(value);
  let target = result as Record<string, unknown>;
  for (const field of path.slice(0, -1))
    target = target[field] as Record<string, unknown>;
  target[path.at(-1)!] = replacement;
  return result;
}
for (const [name, value, decode, cases] of [
  [
    "catalog",
    responseCatalog,
    providerModelCatalog,
    [
      [["models"], null],
      [["models", "0", "id"], ""],
      [["models", "0", "contextWindow"], -1],
      [["models", "0", "maxOutputTokens"], Number.MAX_SAFE_INTEGER + 1],
      [["models", "0", "capabilities"], { tools: "yes" }],
      [["fetchedAt"], "not-a-date"],
      [["fetchedAt"], "2026-02-30T00:00:00Z"],
      [["fetchedAt"], "2026-10-30T00:00:00"],
      [["fetchedAt"], "0"],
      [["sourceUrl"], "https://secret@fixture.invalid/models"],
      [["truncated"], "false"],
      [["models"], [responseCatalog.models[0], responseCatalog.models[0]]],
    ],
  ],
  [
    "usage",
    responseUsage,
    (value: unknown) => providerUsage(value, { source: "gateway" }),
    [
      [["summary", "calls"], "0"],
      [["summary", "reported", "totalTokens"], -1],
      [["summary", "usage", "totalTokens"], Infinity],
      [["summary", "cost", "actual"], null],
      [["providerModels"], null],
      [["calls"], {}],
      [["query", "source"], "native"],
    ],
  ],
  [
    "status",
    responseStatus,
    providerStatus,
    [
      [["configs"], null],
      [["transactions"], [{}]],
      [
        ["runtimes"],
        [
          {
            agentId: "claude",
            version: null,
            supported: "true",
            processes: null,
            reason: "",
          },
        ],
      ],
      [["dynamicChanges"], [{}]],
      [["localTokenProtocol"], { version: 2 }],
    ],
  ],
] as const) {
  for (const [index, [path, replacement]] of cases.entries())
    test(`${name} rejects malformed ${path.join(".")} case ${index + 1} without echoing payload`, () => {
      expect(() => decode(changed(value, [...path], replacement))).toThrow(
        /回复无法核查/,
      );
    });
}
test("old optional metadata and usage groups remain absent instead of becoming zero or unsupported", () => {
  const catalog = providerModelCatalog(responseCatalog);
  expect(catalog.models[0].capabilities).toBeUndefined();
  expect(catalog.models[0].contextWindow).toBeUndefined();
  const usage = providerUsage(
    {
      ...responseUsage,
      query: { ...responseUsage.query, agent: null, days: null },
    },
    { source: "gateway" },
  );
  expect(usage.providerModels).toBeUndefined();
  expect(usage.summary.reported.totalTokens).toBeUndefined();
  expect(providerTokens(usage.summary)).toBe("0");
  expect(
    providerTokens({
      ...responseTotals,
      calls: 1,
      usage: { ...responseTotals.usage, totalTokens: 8 },
    }),
  ).toBe("8 · 部分");
  expect(
    providerStatus({ ...responseStatus, localTokenProtocol: { version: 1 } })
      .localTokenProtocol,
  ).toBeUndefined();
});
test("read projections strip unknown root and nested fields", () => {
  const value = providerModelCatalog({
    ...responseCatalog,
    secret: "disposable-payload-sentinel",
    models: [
      { ...responseCatalog.models[0], secret: "disposable-payload-sentinel" },
    ],
  });
  expect(JSON.stringify(value)).not.toContain("disposable-payload-sentinel");
  expect(value.models[0].id).toBe("fixture-model");
});
test("unverifiable mutation replies retain the unknown-outcome classification", () => {
  try {
    providerTransaction({ ...responseTransaction, before: null });
    throw Error("accepted");
  } catch (reason) {
    expect(reason).toMatchObject({
      code: "transport_invalid_response",
      outcomeUnknown: true,
    });
  }
});
test("draft rules retain custom URLs, manual catalogs and disabled-model selection across modes", () => {
  let state = providerEditorDraft({
    ...responseProvider,
    id: "custom-fixture",
    disabledModels: ["fixture-model"],
  });
  state = reduceProviderDraft(state, {
    type: "endpoint",
    value: "https://custom.invalid/v1",
  });
  state = reduceProviderDraft(state, { type: "vendor", value: "deepseek" });
  state = reduceProviderDraft(state, { type: "protocol", value: "messages" });
  expect(state.provider.endpoint).toBe("https://custom.invalid/v1");
  state = reduceProviderDraft(state, {
    type: "endpoint-source",
    value: "default",
  });
  expect(state.provider.endpoint).toBe("https://api.deepseek.com/anthropic");
  state = reduceProviderDraft(state, {
    type: "endpoint-source",
    value: "custom",
  });
  state = reduceProviderDraft(state, { type: "mode", value: "automatic" });
  state = reduceProviderDraft(state, { type: "advanced" });
  state = reduceProviderDraft(state, {
    type: "select-model",
    value: "fixture-model",
  });
  expect(state).toMatchObject({
    mode: "catalog",
    advanced: true,
    provider: {
      endpoint: "https://custom.invalid/v1",
      models: ["fixture-model"],
      disabledModels: [],
    },
  });
});
test("verification replies must identify the requested provider and explicit model", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-response.html");
  const outcomes = await page.evaluate(async () => {
    const fixture = window.providerResponseFixture;
    const reply = {
      providerId: "fixture-provider",
      model: "fixture-model",
      fingerprint: "fixture",
      state: "verified",
      checkedAt: "2026-10-30T00:00:00Z",
      message: "verified",
    };
    const outcomes = [];
    for (const value of [
      reply,
      { ...reply, providerId: "other-provider" },
      { ...reply, model: "other-model" },
      { ...reply, model: undefined },
    ]) {
      fixture.reply = value;
      try {
        await fixture.client.verifyProvider(
          "fixture-provider",
          1,
          "fixture-model",
        );
        outcomes.push("verified");
      } catch (reason) {
        outcomes.push((reason as { code: string }).code);
      }
    }
    return outcomes;
  });
  expect(outcomes).toEqual([
    "verified",
    "transport_invalid_response",
    "transport_invalid_response",
    "transport_invalid_response",
  ]);
});
for (const [name, reply] of [
  ["models", responseCatalog],
  ["usage", responseUsage],
  ["status", responseStatus],
  ["imports", []],
] as const) {
  test(`${name}: actual IPC API validates malformed bytes and preserves the command contract`, async ({
    page,
  }) => {
    await page.goto("/tests/frontend/provider-response.html");
    const call = async (payload: unknown) =>
      page.evaluate(
        async ({ kind, payload }) => {
          const fixture = window.providerResponseFixture;
          fixture.reply = payload;
          try {
            const result =
              kind === "models"
                ? await fixture.client.providerModels({
                    endpoint: "https://fixture.invalid/v1",
                    protocol: "chat-completions",
                    vendor: "custom",
                    credentialRef: null,
                  })
                : kind === "usage"
                  ? await fixture.client.providerUsage({ source: "gateway" })
                  : kind === "status"
                    ? await fixture.client.providerStatus()
                    : await fixture.client.discoverProviderImports();
            return { result, error: null };
          } catch (reason) {
            const error = reason as {
              code: string;
              message: string;
              outcomeUnknown: boolean;
            };
            return {
              result: null,
              error: {
                code: error.code,
                message: error.message,
                outcomeUnknown: error.outcomeUnknown,
              },
            };
          }
        },
        { kind: name, payload },
      );
    expect((await call(reply)).error).toBeNull();
    const invalid = await call({ secret: "disposable-payload-sentinel" });
    expect(invalid.error).toMatchObject({
      code: "transport_invalid_response",
      outcomeUnknown: false,
    });
    expect(JSON.stringify(invalid)).not.toContain(
      "disposable-payload-sentinel",
    );
    const calls = await page.evaluate(
      () => window.providerResponseFixture.calls,
    );
    expect(calls).toHaveLength(2);
    expect(calls[0].name).toBe(
      {
        models: "fetch_provider_models",
        usage: "query_provider_usage",
        status: "get_provider_status",
        imports: "discover_provider_imports",
      }[name],
    );
  });
}
