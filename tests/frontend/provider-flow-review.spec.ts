import { expect, test, type Page } from "@playwright/test";
import type {} from "./provider-auto-connect-harness";
const store = "c9e027fe-4536-4d08-8a55-74e6cbaf6313";
const operation = "cf45b4a0-4dac-4bd5-b276-3181d3e1514e";
const oldReference = "e1b71318-84c0-4f5d-b399-aee182f69ade";
const keyInput = (page: Page) =>
  page.getByRole("textbox", { name: "API Key", exact: true });
async function seedPending(page: Page) {
  await page.addInitScript(
    ({ store, operation }) => {
      localStorage.setItem(
        `local-agent.credential-operation.v2.${store}.${operation}`,
        JSON.stringify({
          version: 2,
          storeId: store,
          operationId: operation,
          label: "新连接 API Key",
          cancelledPending: false,
        }),
      );
    },
    { store, operation },
  );
}
async function selectVendor(page: Page) {
  await page.getByRole("combobox", { name: "供应商", exact: true }).click();
  await page.getByRole("option", { name: "DeepSeek", exact: true }).click();
}
for (const input of [
  "blank-key",
  "non-ascii-key",
  "blank-label",
  "key-in-endpoint",
] as const) {
  test(`${input}: invalid input cannot initialize new credential storage or send the Key`, async ({
    page,
  }) => {
    await page.goto(
      "/tests/frontend/provider-auto-connect.html?mode=credential-unverified",
    );
    await selectVendor(page);
    const secret =
      input === "blank-key"
        ? "   "
        : input === "non-ascii-key"
          ? "disposable-测试-key"
          : "disposable-review-key";
    await keyInput(page).fill(secret);
    if (input === "blank-label") {
      await page.getByRole("button", { name: "高级设置", exact: true }).click();
      await page
        .getByRole("textbox", { name: "凭据名称", exact: true })
        .fill("");
    }
    if (input === "key-in-endpoint")
      await page
        .getByRole("textbox", { name: "服务地址", exact: true })
        .fill(`https://fixture.invalid/${secret}`);
    await page.getByRole("button", { name: "获取模型", exact: true }).click();
    await expect(page.getByText("未能获取模型", { exact: true })).toBeVisible();
    const calls = await page.evaluate(() => ({
      init: window.providerOperationsFixture.commands.filter(
        (row) => row.name === "initialize_provider_credential_namespace",
      ).length,
      keys: window.autoConnectFixture.credentials.length,
      models: window.autoConnectFixture.modelRequests.length,
      state: Object.values(
        window.providerOperationsFixture.snapshot().stores,
      )[0].namespaceState,
    }));
    expect(calls).toEqual({ init: 0, keys: 0, models: 0, state: "unverified" });
    await expect(keyInput(page)).toHaveValue(secret);
  });
}
test("a concealed unresolved record explains disabled actions and links to the recovery entry", async ({
  page,
}) => {
  await seedPending(page);
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=credential-restored",
  );
  await selectVendor(page);
  await expect(
    page.getByRole("button", { name: "获取模型", exact: true }),
  ).toBeDisabled();
  await expect(page.getByText("凭据结果待核查", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "核查保存结果", exact: true }),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "查看待核查记录", exact: true })
    .click();
  await expect(
    page.getByRole("region", { name: "API Key 保存与恢复", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "核查保存结果", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "获取模型", exact: true }),
  ).toBeEnabled();
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(0);
});
test("adopting a recovered Key replaces unsaved input and clears another credential's catalog provenance", async ({
  page,
}) => {
  await seedPending(page);
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=credential-restored&configured",
  );
  await keyInput(page).fill("disposable-not-adopted-key");
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  await page.getByRole("button", { name: "核查保存结果", exact: true }).click();
  await page
    .getByRole("button", { name: "采用到当前连接", exact: true })
    .click();
  await expect(keyInput(page)).toHaveValue("");
  await page
    .getByRole("button", { name: "收起 API Key 恢复选项", exact: true })
    .click();
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const result = await page.evaluate(() => ({
    keys: window.autoConnectFixture.credentials,
    provider: window.autoConnectFixture.manualSaves[0],
  }));
  expect(result.keys).toHaveLength(0);
  expect(result.provider.credentialRef).toBe(oldReference);
  expect(result.provider.modelCatalog).toBeNull();
});
