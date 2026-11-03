import { test, expect } from "@playwright/test";
import type { Snapshot } from "../../src/app/types";
import { agentProducts } from "../../src/shared/types/agent";
import { responseProvider } from "./provider-response-fixture";
import { checkedTableSnapshot } from "./fixtures/tables/context";

test("table fixture snapshots reject incomplete shapes and conflicting identities before publication", () => {
  const value: Snapshot = {
    version: "fixture",
    databasePath: "temporary",
    gatewayStatus: "ready",
    agents: agentProducts.map((p) => ({
      ...p,
      found: false,
      location: null,
      connected: false,
    })),
    providers: [responseProvider],
    settings: { captureContent: false, retentionDays: 30 },
    sessionSources: [],
    observationBindings: [],
  };
  expect(checkedTableSnapshot(value)).toBe(value);
  expect(() =>
    checkedTableSnapshot({
      ...value,
      providers: [responseProvider, responseProvider],
    }),
  ).toThrow(/identity/);
  expect(() =>
    checkedTableSnapshot({
      ...value,
      agents: [value.agents[0], value.agents[0]],
    }),
  ).toThrow(/identity/);
  expect(() =>
    checkedTableSnapshot({
      ...value,
      settings: { ...value.settings, retentionDays: NaN },
    } as unknown as Snapshot),
  ).toThrow(/snapshot/);
  expect(() =>
    checkedTableSnapshot({
      ...value,
      providers: [{ ...responseProvider, models: null }],
    } as unknown as Snapshot),
  ).toThrow();
});
