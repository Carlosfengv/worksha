import { expect, test } from "@playwright/test";
import type {} from "./provider-auto-connect-harness";

test("URL and API Key connect without choosing a protocol, supplier or model", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-auto-connect.html");
  await page
    .getByRole("button", { name: "自动识别并验证", exact: true })
    .click();
  await expect(
    page.getByRole("combobox", { name: "接口协议", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("combobox", { name: "供应商", exact: true }),
  ).toHaveCount(0);
  await page
    .getByRole("textbox", { name: "服务地址 URL", exact: true })
    .fill("https://fixture.invalid/v1/chat/completions");
  await page
    .getByRole("textbox", { name: "API Key", exact: true })
    .fill("disposable-fixture-key");
  await page.getByRole("button", { name: "连接并保存", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const result = await page.evaluate(() => window.autoConnectFixture);
  expect(result.saved).toBe(1);
  expect(result.connections).toHaveLength(1);
  expect(result.credentials).toHaveLength(1);
  expect(result.connections[0].credentialRef).toBe(
    "54261267-b628-4be6-ab05-462c312a251c",
  );
  expect(JSON.stringify(result.connections)).not.toContain(
    "disposable-fixture-key",
  );
  expect(result.manualSaves).toHaveLength(0);
});

test("unknown directory asks only for a model ID and reuses the saved credential on retry", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-auto-connect.html?mode=retry");
  await page
    .getByRole("button", { name: "自动识别并验证", exact: true })
    .click();
  await page
    .getByRole("textbox", { name: "服务地址 URL", exact: true })
    .fill("https://fixture.invalid/v1");
  await page
    .getByRole("textbox", { name: "API Key", exact: true })
    .fill("disposable-fixture-key");
  await page.getByRole("button", { name: "连接并保存", exact: true }).click();
  await expect(page.getByText("连接未完成", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("combobox", { name: "接口协议", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("textbox", { name: "API Key", exact: true }),
  ).toHaveValue("");
  expect(await page.evaluate(() => window.autoConnectFixture.saved)).toBe(0);
  await page
    .getByRole("textbox", { name: "指定模型 ID（可选）", exact: true })
    .fill("chosen-model");
  await page.getByRole("button", { name: "连接并保存", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(1);
  expect(
    await page.evaluate(
      () => window.autoConnectFixture.connections.at(-1)?.model,
    ),
  ).toBe("chosen-model");
});

test("advanced settings preserve typed URL and key; pending connection cannot submit twice or close", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-auto-connect.html?mode=hold");
  await page
    .getByRole("button", { name: "自动识别并验证", exact: true })
    .click();
  await page
    .getByRole("textbox", { name: "服务地址 URL", exact: true })
    .fill("https://fixture.invalid/v1");
  await page
    .getByRole("textbox", { name: "API Key", exact: true })
    .fill("disposable-fixture-key");
  await page.getByRole("button", { name: "高级设置", exact: true }).click();
  await expect(
    page.getByRole("combobox", { name: "接口协议", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("textbox", { name: "Endpoint", exact: true }),
  ).toHaveValue("https://fixture.invalid/v1");
  await expect(
    page.getByRole("textbox", { name: "API Key", exact: true }),
  ).toHaveValue("disposable-fixture-key");
  await page
    .getByRole("button", { name: "自动识别并验证", exact: true })
    .click();
  await page.getByRole("button", { name: "连接并保存", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "正在识别并验证…", exact: true }),
  ).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toBeVisible();
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections.length),
  ).toBe(1);
  await page.evaluate(() => window.autoConnectFixture.release?.());
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("narrow dark failure remains readable and never appears connected", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/provider-auto-connect.html?mode=unknown");
  await page
    .getByRole("button", { name: "自动识别并验证", exact: true })
    .click();
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  await page
    .getByRole("textbox", { name: "服务地址 URL", exact: true })
    .fill("http://127.0.0.1:11434");
  await page.getByRole("button", { name: "连接并保存", exact: true }).click();
  await expect(page.getByText("连接未完成", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => window.autoConnectFixture.saved)).toBe(0);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.getByText("连接未完成", { exact: true }).scrollIntoViewIfNeeded();
  await expect(page.getByText("连接未完成", { exact: true })).toBeInViewport();
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Tab");
  await expect(
    page.getByRole("button", { name: "连接并保存", exact: true }),
  ).toBeFocused();
  await expect(
    page.getByRole("button", { name: "连接并保存", exact: true }),
  ).toBeInViewport();
});

test("unconfirmed credential clears the key, preserves its operation and never starts probes", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=credential-error",
  );
  await page
    .getByRole("button", { name: "自动识别并验证", exact: true })
    .click();
  await page
    .getByRole("textbox", { name: "服务地址 URL", exact: true })
    .fill("https://fixture.invalid/v1");
  await page
    .getByRole("textbox", { name: "API Key", exact: true })
    .fill("disposable-fixture-key");
  await page.getByRole("button", { name: "连接并保存", exact: true }).click();
  await expect(
    page.getByText("钥匙串暂不可用，请重试。", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("textbox", { name: "API Key", exact: true }),
  ).toHaveValue("");
  await expect(page.getByText("凭据结果待核查", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "核查保存结果", exact: true }),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "核查保存结果", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections.length),
  ).toBe(0);
  expect(await page.evaluate(() => window.autoConnectFixture.saved)).toBe(0);
});
