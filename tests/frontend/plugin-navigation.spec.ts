import { expect, test } from "@playwright/test";
import type {} from "./fixtures/tables/plugin-navigation";

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
  await page.evaluate(() => window.setNavigationFixtureEnabled(false));
  await expect(link).not.toBeVisible();
  await expect(
    page.getByRole("heading", { name: "插件不可用", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "返回插件管理" }).click();
  await expect(page).toHaveURL(/#\/plugins$/);
});
