import { expect, test } from "@playwright/test";
import { openRecentActivities } from "./overview-helpers";
import type {} from "./fixtures/tables/activity";

test("DSH direct activity reaches the primary feed, running count and recent activity", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&dshActivity=connected#/overview",
  );
  const feed = page.getByRole("region", { name: "当前活动列表", exact: true });
  const title = "DeepSeek Harness 桌面端任务";
  const row = feed.getByRole("row").filter({ hasText: title });
  const summary = page.getByRole("region", { name: "实时任务汇总" });
  await expect(row).toContainText("执行中");
  await openRecentActivities(page);
  await expect(row.getByText("来源未提供", { exact: true })).toHaveCount(2);
  await expect(summary).toContainText("含 1 个 Agent 上报的 Session");
  await expect(summary).not.toContainText("仅网关");
  await expect(row.getByRole("link", { name: title })).toHaveAttribute(
    "href",
    /#\/sessions\/.+\?scope=unified&source=/,
  );
  await page.getByRole("combobox", { name: "首页筛选 Agent" }).click();
  await page.getByRole("option", { name: "DeepSeek Harness 桌面端" }).click();
  await expect(row).toContainText("执行中");
  await openRecentActivities(page);
  await expect(page.getByText("调用分析筛选无效")).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "调用趋势与占比" }),
  ).toHaveCount(0);
  await page.evaluate(() => {
    window.dshActivity = "idle";
  });
  await expect(row).toContainText("等待输入", { timeout: 8000 });
  await expect(summary).toContainText("当前没有观测到正在执行的任务");

  await page.evaluate(() => {
    window.dshActivity = "expired";
  });
  await expect(row).toContainText("状态待更新", { timeout: 8000 });
  await expect(summary).toContainText("快照已过期");
  const attention = page.getByRole("complementary", { name: "需要关注" });
  await expect(attention).toContainText("来源数据已过期");

  await page.evaluate(() => {
    window.dshActivity = "revoked";
  });
  await expect(row).toHaveCount(0, { timeout: 8000 });
  await expect(feed).toContainText("尚未收到DeepSeek Harness 桌面端的活动");
  await expect(attention).toContainText("读取授权已撤销");
  await expect(
    attention.getByRole("link", { name: "读取授权已撤销" }),
  ).toHaveAttribute(
    "href",
    "#/agents/net.worksha.dsh-desktop%3Adesktop?tab=connection",
  );
  await page.evaluate(() => {
    window.removeReportedSource = true;
  });
  await expect(
    page.getByRole("combobox", { name: "首页筛选 Agent" }),
  ).toContainText("全部 Agents", { timeout: 8000 });
  await expect(page.getByText("调用分析筛选无效")).toHaveCount(0);
});

test("the overview agent filter uses the discovered source catalog", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&dshActivity=missing#/overview",
  );
  await page.getByRole("combobox", { name: "首页筛选 Agent" }).click();
  await expect(
    page.getByRole("option", { name: "DeepSeek Harness 桌面端" }),
  ).toHaveCount(0);
});

for (const source of [
  { name: "DeepSeek Harness 桌面端", query: "" },
  { name: "历史记录来源", query: "&reportedSource=other" },
]) {
  test(`${source.name} displays its reported timing and settled output`, async ({
    page,
  }) => {
    await page.goto(
      `/tests/frontend/tables.html?mode=activity&dshActivity=connected&dshMetrics=1${source.query}#/overview`,
    );
    const feed = page.getByRole("region", {
      name: "当前活动列表",
      exact: true,
    });
    const row = feed.getByRole("row").filter({ hasText: `${source.name}任务` });
    const summary = page.getByRole("region", { name: "实时任务汇总" });
    await expect(row).toContainText("执行中");
    await openRecentActivities(page);
    const elapsed = row.locator("[data-activity-elapsed]");
    await expect(elapsed).toHaveText(/^\d+ 秒$/);
    await expect(row).toContainText(/TTFT 0\.85/);
    await expect(row).toContainText(/均速 24(?:\.\d+)? Token\/s/);
    await expect(summary).toContainText(`${source.name} · 1 个会话已上报`);
    await expect(row).toContainText(`${source.name} · 上报当前步骤`);

    await page.evaluate(() => {
      window.dshActivity = "idle";
    });
    await expect(row).toContainText("等待输入", { timeout: 8000 });
    await expect(elapsed).toHaveText("12 秒");
    await expect(row).toContainText(/TTFT 0\.85/);
    await expect(row).toContainText(/均速 30(?:\.\d+)? Token\/s/);
    await expect(row).toContainText(`${source.name} · 上报已完成步骤`);
    await expect(row).toContainText("120");
    await expect(row).toContainText("deepseek-flash");
  });
}

test("the native overview keeps DSH activities fresh while the window is hidden", async ({
  page,
}) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, "isTauri", { value: true });
  });
  await page.clock.install();
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&dshActivity=connected#/overview",
  );
  const feed = page.getByRole("region", { name: "当前活动列表", exact: true });
  const row = feed
    .getByRole("row")
    .filter({ hasText: "DeepSeek Harness 桌面端任务" });
  await expect(row).toContainText("执行中");

  await openRecentActivities(page);
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "hidden",
    });
    document.dispatchEvent(new Event("visibilitychange"));
    window.dshActivity = "idle";
  });
  await page.clock.fastForward(2000);
  await expect(row).toContainText("等待输入");
});
