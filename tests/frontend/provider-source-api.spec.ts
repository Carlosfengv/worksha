import { expect, test } from "@playwright/test";
import type {} from "./provider-response-harness";
import { responseCatalog } from "./provider-response-fixture";

test("source metadata validates identity and cannot turn invalid authority into a native choice", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-response.html");
  const result = await page.evaluate(async () => {
    const fixture = window.providerResponseFixture;
    const query = {
      storeId: "owned-store",
      providerId: "owned-provider",
      expectedRevision: "4",
    };
    const builtin = {
      providerId: query.providerId,
      expectedRevision: "4",
      sourceRevision: null,
      kind: "builtin",
      choice: null,
      selection: null,
      authorized: true,
      policy: null,
      errorCode: null,
    };
    const choice = { pluginId: "org.example.catalog", digest: "a".repeat(64) };
    const external = {
      ...builtin,
      kind: "external",
      sourceRevision: "b".repeat(64),
      choice,
      authorized: false,
      errorCode: "grant_revoked",
    };
    const saved = {
      ...external,
      authorized: true,
      errorCode: null,
      policy: { schemaVersion: 1, baseProtocol: "responses", path: "/catalog" },
      selection: {
        ...choice,
        bindingId: "owned-binding",
        target: {
          kind: "saved",
          providerId: query.providerId,
          expectedRevision: "4",
        },
      },
    };
    const replies = [
      builtin,
      external,
      saved,
      { ...builtin, providerId: "other-provider" },
      { ...builtin, expectedRevision: "3" },
      { ...builtin, choice },
      { ...saved, selection: null },
      { ...saved, sourceRevision: "unknown" },
      { ...saved, kind: "pending" },
      {
        ...saved,
        selection: {
          ...saved.selection,
          target: {
            kind: "draft",
            label: "owned",
            endpoint: "https://example.invalid",
            protocol: "responses",
            vendor: "custom",
            credentialRef: "private-sentinel",
          },
        },
      },
      { ...builtin, secret: "private-sentinel" },
    ];
    const outcomes = [];
    for (const reply of replies) {
      fixture.reply = reply;
      try {
        outcomes.push({ kind: (await fixture.source.state(query)).kind });
      } catch (reason) {
        const e = reason as {
          code: string;
          outcomeUnknown: boolean;
          message: string;
        };
        outcomes.push({
          code: e.code,
          unknown: e.outcomeUnknown,
          leak: e.message.includes("private-sentinel"),
        });
      }
    }
    return { outcomes, calls: fixture.calls };
  });
  expect(result.outcomes.slice(0, 3)).toEqual([
    { kind: "builtin" },
    { kind: "external" },
    { kind: "external" },
  ]);
  for (const outcome of result.outcomes.slice(3))
    expect(outcome).toEqual({
      code: "transport_invalid_response",
      unknown: false,
      leak: false,
    });
  expect(result.calls).toHaveLength(11);
  expect(
    result.calls.every((c) => c.name === "perform_provider_catalog_source"),
  ).toBe(true);
});

test("a malformed source selection reply stays uncertain and emits no success invalidation", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-response.html");
  const result = await page.evaluate(async () => {
    const fixture = window.providerResponseFixture;
    const query = {
      storeId: "owned-store",
      providerId: "owned-provider",
      expectedRevision: "4",
    };
    let changed = 0;
    window.addEventListener("provider-catalog-source-changed", () => changed++);
    fixture.reply = { kind: "builtin", private: "sentinel" };
    let error;
    try {
      await fixture.source.select(query, "b".repeat(64), null);
    } catch (reason) {
      const e = reason as { code: string; outcomeUnknown: boolean };
      error = { code: e.code, unknown: e.outcomeUnknown };
    }
    const before = changed;
    fixture.reply = {
      providerId: query.providerId,
      expectedRevision: "4",
      sourceRevision: "c".repeat(64),
      kind: "builtin",
      authorized: true,
    };
    await fixture.source.select(query, "b".repeat(64), null);
    return { error, before, changed, calls: fixture.calls };
  });
  expect(result.error).toEqual({
    code: "transport_invalid_response",
    unknown: true,
  });
  expect(result.before).toBe(0);
  expect(result.changed).toBe(1);
  expect(result.calls).toHaveLength(2);
  expect(result.calls[0]).toEqual({
    name: "perform_provider_catalog_source",
    args: {
      input: {
        kind: "select",
        query: {
          storeId: "owned-store",
          providerId: "owned-provider",
          expectedRevision: "4",
        },
        sourceRevision: "b".repeat(64),
        catalog: null,
      },
    },
  });
});

test("source reads keep the exact host command and never retry the old native IPC after invalid bytes", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-response.html");
  const result = await page.evaluate(async (catalog) => {
    const fixture = window.providerResponseFixture;
    const query = {
      storeId: "owned-store",
      providerId: "owned-provider",
      expectedRevision: "4",
    };
    fixture.reply = catalog;
    const accepted = await fixture.source.read(query, "b".repeat(64));
    fixture.reply = { models: null, secret: "private-sentinel" };
    let error;
    try {
      await fixture.source.read(query, "b".repeat(64));
    } catch (reason) {
      const e = reason as { code: string; message: string };
      error = { code: e.code, leak: e.message.includes("private-sentinel") };
    }
    return { accepted, error, calls: fixture.calls };
  }, responseCatalog);
  expect(result.accepted.models).toEqual(responseCatalog.models);
  expect(result.error).toEqual({
    code: "transport_invalid_response",
    leak: false,
  });
  expect(result.calls).toHaveLength(2);
  expect(
    result.calls.every((c) => c.name === "perform_provider_catalog_source"),
  ).toBe(true);
});
