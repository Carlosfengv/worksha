import { expect, test } from "@playwright/test";
import type {} from "./activity-fixture";
import { openAgentHistory } from "./overview-helpers";

test("Agent list and details confirm a configured gateway while describing idle traffic accurately", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/agents");
  const cli = page.getByRole("row").filter({
    has: page.getByRole("link", { name: "详情 Claude Code CLI", exact: true }),
  });
  await expect(cli).toContainText("本地网关 · 默认");
  await expect(cli).toContainText("网关路由已确认");
  await expect(cli).not.toContainText("当前路由未确认");
  await expect(cli).not.toContainText("网关正在代理");
  await cli
    .getByRole("link", { name: "详情 Claude Code CLI", exact: true })
    .click();
  await expect(page).toHaveURL(/#\/agents\/claude-code-cli$/);
  const connection = page.getByLabel("连接方式与采集范围", { exact: true });
  await expect(connection).toHaveCount(1);
  await expect(connection).toContainText("网关路由已确认");
  await connection
    .getByRole("button", { name: "连接详情", exact: true })
    .click();
  await expect(connection).toContainText("默认配置已匹配本应用的网关路由");
  await expect(connection).toContainText("当前没有进行中的网关请求");
});

test("overview switches from proxying to confirmed idle without losing configuration confirmation on stale activity", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  const cli = page.getByRole("article", {
    name: "Claude Code CLI实时活动",
    exact: true,
  });
  await expect(cli).toContainText("网关正在代理 · 2 个请求");
  await page.evaluate(() => {
    window.activityCase = "completed";
  });
  const route = cli.getByTitle(/默认配置已匹配本应用的网关路由/);
  await expect(route).toBeVisible();
  await expect(route).toHaveAttribute("title", /没有进行中的网关请求/);
  await expect(cli).not.toContainText("网关正在代理");
  await expect(cli).not.toContainText("当前路由未确认");
  await page.evaluate(() => {
    window.activityCase = "stale";
  });
  await expect(route).toHaveAttribute("title", /实时请求状态待更新/);
  await expect(cli).not.toContainText("网关正在代理");
});

test("historical gateway requests cannot confirm a current official-login default", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&connection=official#/overview",
  );
  await openAgentHistory(page);
  const cli = page.getByRole("article", {
    name: "Claude Code CLI实时活动",
    exact: true,
  });
  await expect(cli).toContainText("网关正在代理 · 2 个请求");
  await page.evaluate(() => {
    window.activityCase = "completed";
  });
  await expect(cli).toContainText("暂无进行中会话");
  await expect(cli).toContainText("官方登录 · 默认");
  await expect(cli).toContainText("采集受限");
  await expect(cli.locator("dl")).toContainText("35.0 Token/s");
  await expect(cli).not.toContainText("网关路由已确认");
  await expect(cli).not.toContainText("网关正在代理");
});
