import { expect, test } from "@playwright/test";
import {
  responseProvider,
  responseTransaction,
} from "./provider-response-fixture";
import type {} from "./provider-response-harness";
import type { DynamicChange } from "../../src/features/providers/types";

test("native configuration previews correlate the Agent and original revision", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-response.html");
  const outcomes = await page.evaluate(
    async ({ transaction }) => {
      const fixture = window.providerResponseFixture;
      const original = {
        ...transaction,
        before: { ...transaction.before, revision: "original-revision" },
      };
      const outcomes = [];
      for (const reply of [
        original,
        { ...original, agentId: "codex" },
        {
          ...original,
          before: { ...original.before, revision: "other-revision" },
        },
      ]) {
        fixture.reply = reply;
        try {
          await fixture.client.prepareProviderSwitch(
            "claude",
            "original-revision",
            null,
          );
          outcomes.push("accepted");
        } catch (reason) {
          outcomes.push((reason as { code: string }).code);
        }
      }
      return outcomes;
    },
    { transaction: responseTransaction },
  );
  expect(outcomes).toEqual([
    "accepted",
    "transport_invalid_response",
    "transport_invalid_response",
  ]);
});

test("dynamic actions correlate their original snapshot and allow a new restore preview", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-response.html");
  const results = await page.evaluate(
    async ({ provider }) => {
      const fixture = window.providerResponseFixture;
      const beforeProvider = { ...provider, model: "previous-model" };
      const before = {
        id: "previous-version",
        bindingId: "binding",
        provider: beforeProvider,
        createdAt: "2026-10-30T00:00:00Z",
      };
      const change: DynamicChange = {
        id: "operation",
        bindingId: "binding",
        expectedGeneration: "3",
        state: "applied",
        before,
        target: provider,
        published: { ...before, id: "published-version", provider },
        createdAt: before.createdAt,
      };
      const restored: DynamicChange = {
        ...change,
        id: "new-restore-operation",
        expectedGeneration: "4",
        state: "pending",
        before: change.published!,
        target: beforeProvider,
        published: null,
      };
      const outcomes = [];
      for (const [action, reply] of [
        ["commit", change],
        ["commit", { ...change, id: "another-operation" }],
        ["cancel", { ...change, state: "cancelled" }],
        ["restore", restored],
        [
          "restore",
          {
            ...restored,
            bindingId: "another-binding",
            before: { ...before, bindingId: "another-binding" },
          },
        ],
        ["restore", { ...restored, target: provider }],
        [
          "commit",
          {
            ...change,
            published: { ...change.published!, bindingId: "another-binding" },
          },
        ],
      ] as const) {
        fixture.reply = reply;
        try {
          await fixture.client.dynamicRouteAction(change, action);
          outcomes.push("accepted");
        } catch (reason) {
          outcomes.push((reason as { code: string }).code);
        }
      }
      return { outcomes, calls: fixture.calls };
    },
    { provider: responseProvider },
  );
  expect(results.outcomes).toEqual([
    "accepted",
    "transport_invalid_response",
    "accepted",
    "accepted",
    "transport_invalid_response",
    "transport_invalid_response",
    "transport_invalid_response",
  ]);
  for (const call of results.calls)
    expect(call).toMatchObject({
      name: "dynamic_route_action",
      args: { changeId: "operation" },
    });
});

test("configuration actions correlate replies and accept a new restore transaction only for its original", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-response.html");
  const outcomes = await page.evaluate(
    async ({ transaction }) => {
      const fixture = window.providerResponseFixture;
      const outcomes = [];
      for (const [action, reply] of [
        ["apply", transaction],
        ["apply", { ...transaction, id: "another-transaction" }],
        [
          "restore",
          {
            ...transaction,
            id: "restored-transaction",
            restoreOf: transaction.id,
          },
        ],
        [
          "restore",
          {
            ...transaction,
            id: "restored-transaction",
            restoreOf: "another-transaction",
          },
        ],
      ] as const) {
        fixture.reply = reply;
        try {
          await fixture.client.providerSwitchAction(transaction.id, action);
          outcomes.push("accepted");
        } catch (reason) {
          const error = reason as { code: string; outcomeUnknown: boolean };
          outcomes.push(`${error.code}:${error.outcomeUnknown}`);
        }
      }
      fixture.reply = { ...transaction, id: "another-transaction" };
      try {
        await fixture.client.restartAgentProvider(transaction.id, "claude");
        outcomes.push("accepted");
      } catch (reason) {
        const error = reason as { code: string; outcomeUnknown: boolean };
        outcomes.push(`${error.code}:${error.outcomeUnknown}`);
      }
      return outcomes;
    },
    { transaction: responseTransaction },
  );
  expect(outcomes).toEqual([
    "accepted",
    "transport_invalid_response:true",
    "accepted",
    "transport_invalid_response:true",
    "transport_invalid_response:true",
  ]);
});

test("dynamic preparation rejects another operation, binding, generation, provider or model", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-response.html");
  const outcomes = await page.evaluate(
    async ({ provider }) => {
      const fixture = window.providerResponseFixture;
      const input = {
        operationId: "operation",
        bindingId: "binding",
        expectedGeneration: "3",
        providerId: provider.id,
        expectedProviderRevision: provider.revision,
        model: provider.model,
      };
      const version = {
        id: "version",
        bindingId: input.bindingId,
        provider,
        createdAt: "2026-10-30T00:00:00Z",
      };
      const change = {
        id: input.operationId,
        bindingId: input.bindingId,
        expectedGeneration: input.expectedGeneration,
        state: "pending",
        before: version,
        target: provider,
        published: null,
        createdAt: version.createdAt,
      };
      const outcomes = [];
      for (const reply of [
        change,
        { ...change, id: "another" },
        { ...change, bindingId: "another" },
        { ...change, expectedGeneration: "4" },
        { ...change, target: { ...provider, id: "another" } },
        { ...change, target: { ...provider, revision: 2 } },
        { ...change, target: { ...provider, model: "another" } },
      ]) {
        fixture.reply = reply;
        try {
          await fixture.client.prepareDynamicRoute(input);
          outcomes.push("accepted");
        } catch (reason) {
          const error = reason as { code: string; outcomeUnknown: boolean };
          outcomes.push(`${error.code}:${error.outcomeUnknown}`);
        }
      }
      return outcomes;
    },
    { provider: responseProvider },
  );
  expect(outcomes).toEqual([
    "accepted",
    ...Array<string>(6).fill("transport_invalid_response:true"),
  ]);
});
