import { expect, test } from "@playwright/test";

test("complete source installation pins the reviewed key and keeps lifecycle grants separate", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/full-plugin.html");
  await page.getByRole("button", { name: "选择插件包", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "安装数据插件", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("checkbox", { name: "我已核对并信任此发布者公钥", exact: true })
    .check();
  await page.getByRole("button", { name: "安装数据插件", exact: true }).click();
  await expect(page.getByLabel("插件数据详情", { exact: true })).toBeVisible();
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
    page.getByRole("button", { name: "选择数据文件", exact: true }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "选择数据文件", exact: true }).click();
  expect(
    (await page.evaluate(() => window.fullPluginFixture.calls)).some(
      (c) => c.operation === "refresh",
    ),
  ).toBe(false);
  await page.getByRole("button", { name: "读取插件数据", exact: true }).click();
  await expect(page.getByText("已手动读取", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "统一会话展示", exact: true }).click();
  await expect(
    page.getByRole("checkbox", { name: /允许应用重启后/ }),
  ).toHaveCount(0);
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

test("persistent watch is explicit and independent of metadata storage", async ({
  page,
}) => {
  await page.goto("/tests/frontend/full-plugin.html?watch=1");
  await page.getByRole("row").filter({ hasText: "完整只读历史" }).click();
  await page.getByRole("button", { name: "统一会话展示", exact: true }).click();
  const watch = page.getByRole("checkbox", {
    name: "允许应用重启后继续自动读取并监听此文件",
  });
  const metadata = page.getByRole("checkbox", { name: "保存会话元数据" });
  await expect(watch).not.toBeChecked();
  await expect(metadata).not.toBeChecked();
  await page.getByRole("button", { name: "确认加入统一展示" }).click();
  await expect(page.getByText("已加入统一展示 · 仅本次运行")).toBeVisible();
  await watch.check();
  await page.getByRole("button", { name: "更新展示授权" }).click();
  await expect(page.getByText("已加入统一展示 · 重启后继续监听")).toBeVisible();
  await page.getByRole("button", { name: "停止统一展示" }).click();
  await expect(watch).not.toBeChecked();
  const calls = (await page.evaluate(() => window.fullPluginFixture.calls))
    .filter((call) => call.operation === "consent")
    .map((call) => call.input);
  expect(calls).toMatchObject([
    { enabled: true, persistMetadata: false, persistWatch: false },
    { enabled: true, persistMetadata: false, persistWatch: true },
    { enabled: false, persistMetadata: false, persistWatch: false },
  ]);
});
