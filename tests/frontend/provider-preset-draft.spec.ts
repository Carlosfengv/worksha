import { test, expect } from "@playwright/test";
import {
  providerEditorDraft,
  reduceProviderDraft,
} from "../../src/features/providers/editor-draft";
import {
  builtinPresetAdvice,
  presetCredentialLabel,
  validatePresetAdvice,
  type PresetAdvice,
} from "../../src/features/providers/preset-advice";
import { validCredentialLabel } from "../../src/features/providers/credential-recovery-records";
import type { Provider } from "../../src/features/providers/types";
const provider: Provider = {
  id: "custom-9989854e-9ccc-4cbd-b8ab-814843a9ba6a",
  name: "",
  vendor: "custom",
  endpoint: "",
  protocol: "responses",
  model: "",
  models: [],
  configured: false,
  credentialRef: null,
  credentialLabel: null,
  revision: 0,
  category: "custom",
};
const advice: PresetAdvice = {
  origin: {
    kind: "external",
    pluginId: "org.example.templates",
    digest: "a".repeat(64),
    presetId: "openai",
  },
  sourceName: "Templates",
  name: "External initial name",
  protocol: "responses",
  endpoints: { responses: "https://example.invalid/v1" },
  model: "starter",
  documents: null,
};
test("MiniMax M Plan uses the official endpoint and its own credential", () => {
  const plan = builtinPresetAdvice.find(
    (preset) =>
      preset.origin.kind === "builtin" &&
      preset.origin.presetId === "minimax-plan",
  )!;
  const state = providerEditorDraft({
    ...provider,
    endpoint: "https://custom.invalid/v1",
    credentialRef: "db01d992-bdc7-4af5-968e-e465f968871c",
  });
  const selected = reduceProviderDraft(state, { type: "preset", value: plan });
  expect(selected.provider).toMatchObject({
    vendor: "minimax-plan",
    protocol: "messages",
    endpoint: "https://api.minimax.io/anthropic",
    model: "MiniMax-M3.1-Flash",
    credentialRef: null,
  });
  const changed = reduceProviderDraft(selected, {
    type: "protocol",
    value: "responses",
  });
  expect(changed.provider.endpoint).toBe("https://api.minimax.io/v1");
});
test("static template fills a blank draft without changing its identity, revision or verification state", () => {
  const before = providerEditorDraft(provider),
    after = reduceProviderDraft(before, { type: "preset", value: advice });
  expect(after.provider).toMatchObject({
    id: provider.id,
    revision: 0,
    configured: false,
    vendor: "custom",
    name: advice.name,
    endpoint: advice.endpoints.responses,
    model: "starter",
    credentialRef: null,
  });
  expect(after.preset).toBe(advice);
  expect(after.endpointMode).toBe("default");
  expect(after.catalog).toBe("starter");
  expect(before.provider.endpoint).toBe("");
});
test("a copied template preserves explicit custom fields, registered models and original credential ownership", () => {
  let state = providerEditorDraft({
    ...provider,
    name: "User name",
    endpoint: "https://user.invalid/v1",
    model: "user-model",
    models: ["user-model"],
    credentialRef: "db01d992-bdc7-4af5-968e-e465f968871c",
    credentialLabel: "Existing Key",
    configured: true,
    revision: 7,
  });
  const old = structuredClone(state);
  state = reduceProviderDraft(state, { type: "preset", value: advice });
  expect(state.provider).toMatchObject({
    name: old.provider.name,
    endpoint: old.provider.endpoint,
    model: old.provider.model,
    models: old.provider.models,
    credentialRef: old.provider.credentialRef,
    revision: 7,
  });
  expect(state.label).toBe("Existing Key");
});
test("a long unicode preset name remains intact while its generated credential label fits the existing byte budget", () => {
  const name = "供应商".repeat(33);
  const result = reduceProviderDraft(providerEditorDraft(provider), {
    type: "preset",
    value: { ...advice, name },
  });
  expect(result.provider.name).toBe(name);
  expect(new TextEncoder().encode(result.label).length).toBeLessThanOrEqual(
    200,
  );
  expect(validCredentialLabel(result.label)).toBe(true);
  expect(presetCredentialLabel("DeepSeek")).toBe("DeepSeek API Key");
});
test("preset address modes use the copied origin without treating external templates as official builtin vendors", () => {
  let state = reduceProviderDraft(providerEditorDraft(provider), {
    type: "preset",
    value: advice,
  });
  state = reduceProviderDraft(state, {
    type: "endpoint",
    value: "https://user.invalid",
  });
  state = reduceProviderDraft(state, {
    type: "endpoint-source",
    value: "default",
  });
  expect(state.provider.endpoint).toBe(advice.endpoints.responses);
  state = reduceProviderDraft(state, { type: "protocol", value: "messages" });
  expect(state.endpointMode).toBe("custom");
  expect(state.provider.endpoint).toBe("");
  expect(state.provider.vendor).toBe("custom");
});
test("builtin advice preserves its original initial values and unknown or authority-bearing metadata is rejected", () => {
  for (const p of builtinPresetAdvice)
    expect(validatePresetAdvice(p)).toEqual(p);
  expect(() =>
    validatePresetAdvice({ ...advice, protocol: "future" }),
  ).toThrow();
  expect(() =>
    validatePresetAdvice({
      ...advice,
      endpoints: { responses: "https://user:secret@example.invalid" },
    }),
  ).toThrow();
  expect(() =>
    validatePresetAdvice({
      ...advice,
      documents: "https://example.invalid/doc",
    }),
  ).toThrow();
});
