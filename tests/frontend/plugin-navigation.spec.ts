import { expect, test } from "@playwright/test";
import type {} from "./fixtures/tables/plugin-navigation";

test("撤权后丢弃迟到的报表响应", async ({ page }) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=plugin-navigation#/settings",
  );
  const link = page.getByRole("link", { name: "Agent 会话报表", exact: true });
  await expect(link).toBeVisible();
  await page.evaluate(() => window.holdNavigationFixtureRead());
  await link.click();
  await expect
    .poll(() => page.evaluate(() => window.navigationFixtureReads))
    .toBe(1);
  await page.evaluate(() => window.setNavigationFixtureAccess(false));
  await expect
    .poll(() => page.evaluate(() => window.navigationFixtureReads))
    .toBe(2);
  await expect(page.getByText("报表来源授权已撤销")).toBeVisible();
  await page.evaluate(() => window.releaseNavigationFixtureRead());
  await expect(page.getByText("报表来源授权已撤销")).toBeVisible();
  await expect(
    page.getByText("DeepSeek Harness 桌面端 · 2 个会话 · 38 Token"),
  ).toHaveCount(0);
});

test("an active uploaded plugin gets a keyboard-accessible sidebar route and loses it on disable", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=plugin-navigation#/settings",
  );
  const navigation = page.getByRole("navigation", { name: "主导航" });
  const link = navigation.getByRole("link", {
    name: "Agent 会话报表",
    exact: true,
  });
  await expect(link).toHaveAttribute(
    "href",
    "#/extensions/net.worksha.source-report/sessions",
  );
  await link.focus();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(
    /#\/extensions\/net\.worksha\.source-report\/sessions$/,
  );
  await expect(
    page.getByText("DeepSeek Harness 桌面端 · 2 个会话 · 38 Token", {
      exact: true,
    }),
  ).toBeVisible();
  await page.evaluate(() => window.setNavigationFixtureAccess(false));
  await expect(page.getByText("报表来源授权已撤销")).toBeVisible();
  await expect(
    page.getByText("DeepSeek Harness 桌面端 · 2 个会话 · 38 Token"),
  ).toHaveCount(0);
  await expect(link).toBeVisible();
  await page.evaluate(() => window.setNavigationFixtureEnabled(false));
  await expect(link).not.toBeVisible();
  await expect(
    page.getByRole("heading", { name: "插件不可用", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("region", { name: "插件页面状态" }),
  ).toBeFocused();
  await page.getByRole("button", { name: "返回插件管理" }).click();
  await expect(page).toHaveURL(/#\/plugins$/);
  const reads = await page.evaluate(() => window.navigationFixtureReads);
  await page.goBack();
  await expect(
    page.getByText("插件已停用，可在插件中心查看状态。"),
  ).toBeVisible();
  expect(await page.evaluate(() => window.navigationFixtureReads)).toBe(reads);
});
