import { expect, test, type Page } from "@playwright/test";
import type {} from "./provider-auto-connect-harness";

const oldRef = "9f765378-a884-4b50-b04c-cf1b14b4a82c";
const newRef = "54261267-b628-4be6-ab05-462c312a251c";
const oldRecord =
  "local-agent.credential-operation.v2.c9e027fe-4536-4d08-8a55-74e6cbaf6313.cf45b4a0-4dac-4bd5-b276-3181d3e1514e";
const oldRaw = JSON.stringify({
  version: 2,
  storeId: "c9e027fe-4536-4d08-8a55-74e6cbaf6313",
  operationId: "cf45b4a0-4dac-4bd5-b276-3181d3e1514e",
  label: "Another pending Key",
  cancelledPending: false,
});
const key = (page: Page) =>
  page.getByRole("textbox", { name: "API Key", exact: true });
const fetchModels = (page: Page) =>
  page.getByRole("button", { name: "获取模型", exact: true });
const save = (page: Page) =>
  page.getByRole("button", { name: "保存连接", exact: true });
async function open(page: Page, mode = "") {
  await page.goto(
    `/tests/frontend/provider-auto-connect.html?mode=${mode}&configured`,
  );
  await expect(page.getByRole("dialog")).toBeVisible();
}
async function seedPending(page: Page) {
  await page.addInitScript(
    ({ record, raw }) => localStorage.setItem(record, raw),
    { record: oldRecord, raw: oldRaw },
  );
}

test("an unrelated pending Key does not block an existing credential or change its recovery record", async ({
  page,
}) => {
  await seedPending(page);
  await open(page);
  await expect(fetchModels(page)).toBeEnabled();
  await expect(save(page)).toBeEnabled();
  await expect(page.getByText("凭据结果待核查", { exact: true })).toHaveCount(
    0,
  );
  await page
    .getByRole("textbox", { name: "连接名称（可选）", exact: true })
    .fill("Renamed provider");
  await fetchModels(page).click();
  await save(page).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const result = await page.evaluate(
    (record) => ({
      credentials: window.autoConnectFixture.credentials,
      queries: window.autoConnectFixture.credentialChecks,
      models: window.autoConnectFixture.modelRequests,
      saved: window.autoConnectFixture.manualSaves[0],
      raw: localStorage.getItem(record),
    }),
    oldRecord,
  );
  expect(result.credentials).toHaveLength(0);
  expect(result.queries).toHaveLength(0);
  expect(result.models[0].credentialRef).toBe(oldRef);
  expect(result.saved).toMatchObject({
    name: "Renamed provider",
    credentialRef: oldRef,
  });
  expect(result.saved.modelCatalog?.credentialRef).toBe(oldRef);
  expect(result.raw).toBe(oldRaw);
});

test("typing and clearing a replacement Key updates provenance and pending-record protection without saving secrets", async ({
  page,
}) => {
  await seedPending(page);
  await open(page);
  await expect(fetchModels(page)).toBeEnabled();
  await fetchModels(page).click();
  await expect(page.getByText(/已获取 2 个模型/)).toBeVisible();
  await key(page).fill("disposable-edit-replacement-key");
  await expect(page.getByText(/已获取 2 个模型/)).toHaveCount(0);
  await expect(page.getByText(/新 API Key 尚未保存/)).toBeVisible();
  await expect(page.getByText("凭据结果待核查", { exact: true })).toBeVisible();
  await expect(fetchModels(page)).toBeDisabled();
  await expect(save(page)).toBeDisabled();
  await key(page).fill("");
  await expect(fetchModels(page)).toBeEnabled();
  await expect(save(page)).toBeEnabled();
  await expect(page.getByText("凭据结果待核查", { exact: true })).toHaveCount(
    0,
  );
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(0);
});

test("an unknown replacement result cannot silently save the previous credential", async ({
  page,
}) => {
  await open(page, "credential-error");
  await key(page).fill("disposable-edit-unknown-key");
  await fetchModels(page).click();
  await expect(key(page)).toHaveValue("");
  await expect(page.getByText("凭据结果待核查", { exact: true })).toBeVisible();
  await expect(save(page)).toBeDisabled();
  await expect(fetchModels(page)).toBeDisabled();
  expect(
    await page.evaluate(() => window.autoConnectFixture.manualSaves),
  ).toHaveLength(0);
});

test("a receipt that cannot be published locally does not release replacement protection", async ({
  page,
}) => {
  await open(page);
  await page.evaluate(() => {
    const original = Storage.prototype.setItem;
    let writes = 0;
    Storage.prototype.setItem = function (name: string, value: string) {
      if (name.startsWith("local-agent.credential-operation.") && ++writes > 1)
        throw new DOMException(
          "fixture local update failure",
          "QuotaExceededError",
        );
      return original.call(this, name, value);
    };
  });
  await key(page).fill("disposable-edit-publication-key");
  await fetchModels(page).click();
  await expect(page.getByText("未能获取模型", { exact: true })).toBeVisible();
  await expect(page.getByText("凭据结果待核查", { exact: true })).toBeVisible();
  await expect(save(page)).toBeDisabled();
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(1);
  expect(
    await page.evaluate(() => window.autoConnectFixture.manualSaves),
  ).toHaveLength(0);
});

test("a replacement input cannot present the previous Key's freshly fetched catalog as current", async ({
  page,
}) => {
  await open(page);
  await fetchModels(page).click();
  await expect(page.getByText(/已获取 2 个模型/)).toBeVisible();
  await key(page).fill("disposable-edit-unsaved-key");
  await expect(page.getByText(/已获取 2 个模型/)).toHaveCount(0);
  await expect(page.getByText(/新 API Key 尚未保存/)).toBeVisible();
  await key(page).fill("");
  await expect(page.getByText(/已获取 2 个模型/)).toBeVisible();
  await save(page).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    await page.evaluate(
      () => window.autoConnectFixture.manualSaves[0].credentialRef,
    ),
  ).toBe(oldRef);
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(0);
});

test("returning to the original database retains an unknown replacement's ownership", async ({
  page,
}) => {
  await open(page, "credential-error");
  await key(page).fill("disposable-edit-unknown-key");
  await fetchModels(page).click();
  await expect(page.getByText("凭据结果待核查", { exact: true })).toBeVisible();
  await page.evaluate(() => {
    const other = "6087597b-0625-42ea-8fd0-e51f6ad16d01";
    window.providerOperationsFixture.switchStore(other);
    window.autoConnectFixture.sourceChange?.(other);
  });
  await expect(
    page.getByText("配置来源尚未核对", { exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    const original = "c9e027fe-4536-4d08-8a55-74e6cbaf6313";
    window.providerOperationsFixture.switchStore(original);
    window.autoConnectFixture.sourceChange?.(original);
  });
  await expect(page.getByText("配置来源尚未核对", { exact: true })).toHaveCount(
    0,
  );
  await expect(page.getByText("凭据结果待核查", { exact: true })).toBeVisible();
  await expect(save(page)).toBeDisabled();
  await expect(fetchModels(page)).toBeDisabled();
});

for (const change of ["endpoint", "protocol", "failed-key-fetch"] as const) {
  test(`${change}: editing never submits another source's catalog provenance`, async ({
    page,
  }) => {
    await open(page, change === "failed-key-fetch" ? "models-error" : "");
    if (change === "endpoint") {
      await page
        .getByRole("textbox", { name: "服务地址", exact: true })
        .fill("https://second.invalid/v1");
    } else if (change === "protocol") {
      await page.getByRole("button", { name: "高级设置", exact: true }).click();
      await page
        .getByRole("combobox", { name: "接口协议", exact: true })
        .click();
      await page
        .getByRole("option", { name: "Chat Completions", exact: true })
        .click();
    } else {
      await key(page).fill("disposable-edit-valid-key");
      await fetchModels(page).click();
      await expect(
        page.getByText("未能获取模型", { exact: true }),
      ).toBeVisible();
    }
    await save(page).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    const saved = await page.evaluate(
      () => window.autoConnectFixture.manualSaves[0],
    );
    expect(saved.modelCatalog).toBeNull();
    expect(saved.credentialRef).toBe(
      change === "failed-key-fetch" ? newRef : oldRef,
    );
    expect(saved.models).toContain("fixture-chat");
  });
}

test("replacement fetch and save reuse the new credential and bind the new catalog", async ({
  page,
}) => {
  await open(page);
  await key(page).fill("disposable-edit-valid-key");
  await fetchModels(page).click();
  await fetchModels(page).click();
  await save(page).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const result = await page.evaluate(() => window.autoConnectFixture);
  expect(result.credentials).toHaveLength(1);
  expect(result.modelRequests.map((r) => r.credentialRef)).toEqual([
    newRef,
    newRef,
  ]);
  expect(result.manualSaves[0].modelCatalog?.credentialRef).toBe(newRef);
  expect(result.manualSaves[0].credentialRef).toBe(newRef);
  expect(result.connections).toHaveLength(0);
});

test("editing a name with the existing Key preserves a matching saved catalog", async ({
  page,
}) => {
  await open(page, "credential-unverified");
  await page
    .getByRole("textbox", { name: "连接名称（可选）", exact: true })
    .fill("Renamed provider");
  await save(page).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const result = await page.evaluate(() => ({
    saved: window.autoConnectFixture.manualSaves[0],
    keys: window.autoConnectFixture.credentials,
    models: window.autoConnectFixture.modelRequests,
    init: window.providerOperationsFixture.commands.filter(
      (c) => c.name === "initialize_provider_credential_namespace",
    ),
  }));
  expect(result.saved.modelCatalog?.credentialRef).toBe(oldRef);
  expect(result.saved.credentialRef).toBe(oldRef);
  expect(result.keys).toHaveLength(0);
  expect(result.models).toHaveLength(0);
  expect(result.init).toHaveLength(0);
});
