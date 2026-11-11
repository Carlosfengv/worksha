import assert from "node:assert/strict";
import {providerTokens, providerLatency, providerCosts, providerMoney} from "../src/features/providers/usage-display.ts";

assert.equal(providerTokens(undefined),"—");
assert.equal(providerTokens({calls:0}),"0");
assert.equal(providerTokens({calls:2,usage:{totalTokens:null},reported:{totalTokens:0}}),"未报告");
assert.equal(providerTokens({calls:2,usage:{totalTokens:0},reported:{totalTokens:2}}),"0");
assert.equal(providerTokens({calls:2,usage:{totalTokens:10},reported:{totalTokens:1}}),"10 · 部分");
assert.equal(providerLatency(null),"未知");
assert.equal(providerLatency(0),"0 秒");
assert.equal(providerCosts(undefined),"—");
assert.equal(providerCosts({actual:[],estimated:[],unknownCalls:3}),"3 次未知");
assert.equal(providerCosts({actual:[{currency:"USD",amount:0,calls:1}],estimated:[{currency:"CNY",amount:1.23,calls:2}],unknownCalls:1}),"0 USD · 实际 / 1.23 CNY · 估算 / 1 次未知");
assert.equal(providerMoney(0.0000002,"USD"),"< 0.000001 USD");
assert.equal(providerMoney(0.0000002,"USD",9),"0.0000002 USD");
import {
  providerModelIds,
  availableProviderModelIds,
  providerCatalog,
  registerProviderPresets,
} from "../src/features/providers/catalog.ts";
assert.deepEqual(Object.keys(providerCatalog), ["openai", "anthropic", "openrouter", "deepseek", "kimi", "minimax-plan", "ollama", "vllm", "custom"]);
const preset = { name: "Fixture vendor", protocol: "messages", endpoints: { messages: "https://fixture.invalid" } };
const contributed = registerProviderPresets([["fixture-vendor", preset]]);
assert.equal(contributed["fixture-vendor"].name, "Fixture vendor");
assert(Object.isFrozen(contributed["fixture-vendor"].endpoints));
assert.throws(() => registerProviderPresets([["duplicate", preset], ["duplicate", preset]]), /重复/);
assert.throws(() => registerProviderPresets([["unknown", { ...preset, protocol: "future" }]]), /无效/);

const legacy = { model: "default", models: ["b", "a", "default", "b"] };
assert.deepEqual(providerModelIds(legacy), ["default", "a", "b"]);
assert.deepEqual(availableProviderModelIds(legacy), ["default", "a", "b"]);
const disabled = { ...legacy, disabledModels: ["default", "b"] };
assert.deepEqual(providerModelIds(disabled), ["default", "a", "b"]);
assert.deepEqual(availableProviderModelIds(disabled), ["a"]);
assert.deepEqual(
  availableProviderModelIds({
    ...disabled,
    models: [...disabled.models, "new"],
  }),
  ["a", "new"],
);
assert.deepEqual(
  availableProviderModelIds({
    ...disabled,
    disabledModels: ["default", "a", "b"],
  }),
  [],
);
assert.deepEqual(availableProviderModelIds({ model: "", models: [] }), []);
console.log(
  "Provider model availability: legacy, disabled default, catalog retention, new models and all-disabled passed.",
);

import { catalogKey, inventoryCalls, modelInventory } from "../src/features/providers/model-inventory.ts";
const connection = { id: "a", endpoint: "http://localhost/v1", protocol: "responses", vendor: "custom", credentialRef: null, revision: 1, model: "same", models: ["same", "off"], disabledModels: ["off"], configured: true };
const second = { ...connection, id: "b", models: ["same"], disabledModels: [] };
const catalogs = { [catalogKey(connection)]: { data: { models: [{ id: "same", name: "A", contextWindow: 128000 }, { id: "remote", name: "Remote" }] } } };
const usage = { sourceState: "ready", providerModels: [{ providerId: "a", model: "same", totals: { calls: 85 } }, { providerId: "b", model: "same", totals: { calls: 1 } }] };
const inventory = modelInventory([connection, second, { ...connection, id: "preset", configured: false }], catalogs, usage);
assert.equal(inventory.length, 4);
assert.equal(inventoryCalls(inventory.find((r) => r.provider.id === "a" && r.model.id === "same")), "85");
assert.equal(inventoryCalls(inventory.find((r) => r.provider.id === "b")), "1");
assert.equal(inventory.find((r) => r.model.id === "off").state, "已停用");
assert.equal(inventory.find((r) => r.model.id === "remote").state, "目录可选");
assert.equal(inventoryCalls(inventory.find((r) => r.model.id === "remote")), "0");
for (const update of [{ revision: 2 }, { endpoint: "http://localhost/other" }, { credentialRef: "other" }, { protocol: "messages" }]) {
  const changed = modelInventory([{ ...connection, ...update }], catalogs, usage);
  assert.equal(changed.length, 2);
  assert.equal(changed[0].model.contextWindow, undefined, "changed connections cannot reuse old metadata");
}
assert.equal(inventoryCalls(modelInventory([connection], {}, { sourceState: "disabled", providerModels: usage.providerModels })[0]), "未知");
assert.equal(inventoryCalls(modelInventory([connection], {}, { sourceState: "ready" })[0]), "未知");
assert.equal(inventoryCalls(modelInventory([connection], {}, { sourceState: "partial", providerModels: [] })[0]), "未知");
assert.equal(inventoryCalls(modelInventory([connection], {}, { sourceState: "partial", providerModels: usage.providerModels })[0]), "85 · 部分");
console.log("Provider model inventory: identity isolation, metadata invalidation, full totals, disabled and unknown coverage passed.");

import { observationStatus } from '../src/features/providers/model-observations.ts';
const observed = { model: 'same', client: 'OpenCode', clientVersion: 'fixture', checkedAt: '2026-10-26T00:00:00Z', checks: [{ field: 'tools', status: 'passed', detail: 'Fixture read succeeded.' }, { field: 'jsonReliability', status: 'unknown', detail: 'One invalid result retained.' }] };
const observedCatalogs = { [catalogKey(connection)]: { data: { models: [{ id: 'same', name: 'same' }], observations: { state: 'ready', models: [observed, { ...observed, model: 'same[1M]' }] } } } };
const observedRows = modelInventory([{ ...connection, models: ['same', 'same[1m]', 'unverified'] }, second], observedCatalogs, usage);
assert.equal(observedRows.find(r => r.provider.id === 'a' && r.model.id === 'same').observations.length, 1);
assert.equal(observedRows.find(r => r.model.id === 'same[1m]').observations.length, 1);
assert.equal(observedRows.find(r => r.model.id === 'unverified').observations.length, 0);
assert.equal(observedRows.find(r => r.provider.id === 'b').observations.length, 0);
for (const update of [{ revision: 2 }, { endpoint: 'http://localhost/other' }, { credentialRef: 'other' }, { protocol: 'messages' }]) {
  assert.equal(modelInventory([{ ...connection, ...update }], observedCatalogs, usage)[0].observations.length, 0);
}
assert.equal(observationStatus([observed], 'tools'), '通过（样本）');
assert.equal(observationStatus([observed], 'jsonReliability'), '未知');
assert.equal(observationStatus([observed], 'contextCapacity'), '未知');
assert.equal(observationStatus([observed, { ...observed, checks: [{ field: 'tools', status: 'failed' }] }], 'tools'), '未通过');
assert.equal(observationStatus([{ ...observed, checks: [{ field: 'tools', status: 'unknown' }] }, observed], 'tools'), '未知');
console.log('Provider model observations: provider isolation, configuration invalidation, suffix spelling, missing and mixed outcomes passed.');
