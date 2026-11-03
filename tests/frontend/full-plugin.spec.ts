import { expect, test } from "@playwright/test";

test("complete source installation pins the reviewed key and keeps lifecycle grants separate", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/full-plugin.html");
  await page
    .getByRole("button", { name: "选择完整来源插件包", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "安装完整来源插件", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("checkbox", { name: "我已核对并信任此发布者公钥", exact: true })
    .check();
  await page
    .getByRole("button", { name: "安装完整来源插件", exact: true })
    .click();
  await expect(
    page.getByLabel("完整来源插件详情", { exact: true }),
  ).toBeVisible();
  const install = (
    await page.evaluate(() => window.fullPluginFixture.calls)
  ).find((c) => c.operation === "install");
  expect(install?.input).toEqual({
    packageRef: "review-one",
    digest: "a".repeat(64),
    approvePublisher: true,
    expectedSigningKey: "b".repeat(64),
  });
  expect(
    (await page.evaluate(() => window.fullPluginFixture.calls)).some((c) =>
      ["authorize", "refresh", "consent"].includes(c.operation),
    ),
  ).toBe(false);
  await page.getByRole("button", { name: "启用此版本", exact: true }).click();
  await page.getByRole("button", { name: "确认操作", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "选择并授权来源 JSONL", exact: true }),
  ).toBeEnabled();
  await page
    .getByRole("button", { name: "选择并授权来源 JSONL", exact: true })
    .click();
  expect(
    (await page.evaluate(() => window.fullPluginFixture.calls)).some(
      (c) => c.operation === "refresh",
    ),
  ).toBe(false);
  await page
    .getByRole("button", { name: "手动读取完整来源", exact: true })
    .click();
  await expect(
    page.getByText(/已手动读取。重开后需要重新启用并选择资源/),
  ).toBeVisible();
  await page.getByRole("button", { name: "统一会话展示", exact: true }).click();
  await page
    .getByRole("button", { name: "确认加入统一展示", exact: true })
    .click();
  await expect(
    page.getByRole("link", { name: "查看统一会话", exact: true }),
  ).toHaveAttribute("href", /org.example.full-history%3Ahistory$/);
  expect(
    (await page.evaluate(() => window.fullPluginFixture.calls)).filter(
      (c) => c.operation === "refresh",
    ),
  ).toHaveLength(1);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});
