import { test, expect, type Page } from "@playwright/test";
import type {} from "./provider-auto-connect-harness";
import type {} from "./provider-presets-fixture";
const external = "OpenAI · 外部 Preset Fixture（org.example.preset-fixture）";
async function open(page: Page) {
  await page.goto("/tests/frontend/provider-auto-connect.html?presets=1");
}
async function choices(page: Page) {
  await page.getByRole("combobox", { name: "供应商", exact: true }).click();
  await expect(
    page.getByRole("option", { name: external, exact: true }),
  ).toBeVisible();
}
async function selectExternal(page: Page) {
  await choices(page);
  await page.getByRole("option", { name: external, exact: true }).click();
}
const endpoint = (page: Page) =>
  page.getByRole("textbox", { name: "服务地址", exact: true });
const key = (page: Page) =>
  page.getByRole("textbox", { name: "API Key", exact: true });
const snapshot = (page: Page) =>
  page.evaluate(() => ({
    credentials: window.autoConnectFixture.credentials,
    models: window.autoConnectFixture.modelRequests,
    saved: window.autoConnectFixture.manualSaves,
    connections: window.autoConnectFixture.connections,
    calls: window.providerPresetsFixture.calls,
    commands: window.providerOperationsFixture.commands,
  }));

test("builtin and external presets share one selector and external copy uses the original new-Key catalog/save flow", async ({
  page,
}) => {
  await open(page);
  await choices(page);
  await expect(
    page.getByRole("option", { name: "OpenAI", exact: true }),
  ).toBeVisible();
  await page.getByRole("option", { name: external, exact: true }).click();
  await expect(endpoint(page)).toHaveValue("https://preset-1.invalid/v1");
  let s = await snapshot(page);
  expect(s.calls.map((c) => c.kind)).toEqual(["list", "resolve"]);
  expect(s.credentials).toHaveLength(0);
  expect(s.models).toHaveLength(0);
  expect(s.saved).toHaveLength(0);
  expect(s.connections).toHaveLength(0);
  await key(page).fill("disposable-preset-key");
  await page.getByRole("button", { name: "获取模型", exact: true }).click();
  await expect(key(page)).toHaveValue("");
  await page
    .getByRole("combobox", { name: "默认模型", exact: true })
    .fill("fixture-chat");
  await page
    .getByRole("option", { name: "Fixture Chat · fixture-chat", exact: true })
    .click();
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  s = await snapshot(page);
  expect(s.credentials).toHaveLength(1);
  expect(s.models).toHaveLength(1);
  expect(s.connections).toHaveLength(0);
  expect(s.saved).toHaveLength(1);
  expect(s.saved[0]).toMatchObject({
    name: "OpenAI",
    vendor: "custom",
    endpoint: "https://preset-1.invalid/v1",
    protocol: "responses",
    model: "fixture-chat",
  });
  expect(JSON.stringify(s.saved)).not.toContain("disposable-preset-key");
  expect(s.commands.some((c) => c.name === "perform_plugin_action")).toBe(
    false,
  );
});

test("manual saving an external template prepares the new Key once without a catalog or model probe", async ({
  page,
}) => {
  await open(page);
  await selectExternal(page);
  await expect(endpoint(page)).toHaveValue("https://preset-1.invalid/v1");
  await key(page).fill("disposable-manual-key");
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const s = await snapshot(page);
  expect(s.credentials).toHaveLength(1);
  expect(s.models).toHaveLength(0);
  expect(s.connections).toHaveLength(0);
  expect(s.saved[0].model).toBe("preset-starter");
});

test("metadata failure preserves typed Key and original builtin selection remains usable", async ({
  page,
}) => {
  await open(page);
  await key(page).fill("disposable-preserved-key");
  await page.evaluate(() => {
    window.providerPresetsFixture.failList = true;
  });
  await page.getByRole("combobox", { name: "供应商", exact: true }).click();
  await page.getByRole("option", { name: "DeepSeek", exact: true }).click();
  await expect(endpoint(page)).toHaveValue("https://api.deepseek.com");
  await expect(key(page)).toHaveValue("disposable-preserved-key");
  await expect(page.getByText(/外部预设暂不可读取/)).toBeVisible();
  const s = await snapshot(page);
  expect(s.credentials).toHaveLength(0);
  expect(s.saved).toHaveLength(0);
});

for (const bad of ["identity", "protocol", "extra"] as const)
  test(`invalid ${bad} resolution cannot replace a draft or prepare credentials`, async ({
    page,
  }) => {
    await open(page);
    await endpoint(page).fill("https://user.invalid/v1");
    await key(page).fill("disposable-preserved-key");
    await choices(page);
    await page.evaluate((bad) => {
      window.providerPresetsFixture.bad = bad;
    }, bad);
    await page.getByRole("option", { name: external, exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("原草稿保持");
    await expect(endpoint(page)).toHaveValue("https://user.invalid/v1");
    await expect(key(page)).toHaveValue("disposable-preserved-key");
    const s = await snapshot(page);
    expect(s.credentials).toHaveLength(0);
    expect(s.models).toHaveLength(0);
    expect(s.saved).toHaveLength(0);
  });

for (const boundary of ["close", "backend", "store"] as const)
  test(`held resolution fences ${boundary} and never submits`, async ({
    page,
  }) => {
    await open(page);
    await choices(page);
    await page.evaluate(() => {
      window.providerPresetsFixture.hold = "resolve";
    });
    await page.getByRole("option", { name: external, exact: true }).click();
    await expect(
      page.getByText("正在核查所选预设，尚未替换草稿。", { exact: true }),
    ).toBeVisible();
    await expect(endpoint(page)).toBeDisabled();
    await expect(key(page)).toBeDisabled();
    await expect(
      page.getByRole("button", { name: "保存连接", exact: true }),
    ).toBeDisabled();
    if (boundary === "close")
      await page.getByRole("button", { name: "取消", exact: true }).click();
    else
      await page.evaluate((boundary) => {
        if (boundary === "backend")
          window.providerOperationsFixture.switchStore(
            window.providerOperationsFixture.snapshot().storeId,
          );
        else
          window.autoConnectFixture.sourceChange?.(
            "6087597b-0625-42ea-8fd0-e51f6ad16d01",
          );
      }, boundary);
    await page.evaluate(() => window.providerPresetsFixture.release?.());
    if (boundary === "close")
      await expect(page.getByRole("dialog")).toHaveCount(0);
    else await expect(endpoint(page)).toHaveValue("");
    const s = await snapshot(page);
    expect(s.credentials).toHaveLength(0);
    expect(s.models).toHaveLength(0);
    expect(s.saved).toHaveLength(0);
  });

test("copied external values and typed Key survive availability loss without granting or rewriting a source", async ({
  page,
}) => {
  await open(page);
  await selectExternal(page);
  await expect(endpoint(page)).toHaveValue("https://preset-1.invalid/v1");
  await key(page).fill("disposable-preserved-key");
  await page.evaluate(() => {
    window.providerPresetsFixture.disabled = true;
    window.providerPresetsFixture.invalidate();
  });
  await expect(endpoint(page)).toHaveValue("https://preset-1.invalid/v1");
  await expect(key(page)).toHaveValue("disposable-preserved-key");
  await expect(
    page.getByRole("combobox", { name: "供应商", exact: true }),
  ).toHaveValue(/来源待核查/);
  const s = await snapshot(page);
  expect(s.credentials).toHaveLength(0);
  expect(s.models).toHaveLength(0);
  expect(s.saved).toHaveLength(0);
});

test("a large complete inventory can search the final preset beyond the visible option budget", async ({
  page,
}) => {
  await open(page);
  await page.evaluate(() => {
    window.providerPresetsFixture.count = 600;
  });
  const picker = page.getByRole("combobox", { name: "供应商", exact: true });
  await picker.click();
  await expect
    .poll(() => page.evaluate(() => window.providerPresetsFixture.calls.length))
    .toBe(1);
  await picker.fill("外部供应商 0600");
  await page.getByRole("option", { name: /外部供应商 0600/ }).click();
  await expect(endpoint(page)).toHaveValue("https://preset-600.invalid/v1");
  expect((await snapshot(page)).credentials).toHaveLength(0);
});
