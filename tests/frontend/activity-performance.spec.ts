import { expect, test } from "@playwright/test";
import { openRecentActivities } from "./overview-helpers";
import type {} from "./activity-fixture";

test("five-second rates give way to timestamped history and turn totals without entering the live sum", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  const summary = page.getByRole("region", {
    name: "实时任务汇总",
    exact: true,
  });
  const rate = summary
    .locator('[data-slot="metric-card"]')
    .filter({ hasText: /(?:总|已测) Token 速率/ });
  const value = rate.locator('[data-slot="metric-card-value-row"]');
  const feed = page.getByRole("region", { name: "当前活动列表", exact: true });
  const row = feed.getByRole("row").filter({ hasText: "原生采集同一条消息" });
  await page.evaluate(() => {
    window.activityCase = "native-desktop-performance-live";
  });
  await openRecentActivities(page);
  await expect(row).toContainText("近5秒 24.0 Token/s");
  await expect(row).toContainText("本轮已报输出 1,200 Token");
  await expect(row).toContainText("其中思考 800 Token");
  await expect(row.locator("[data-activity-elapsed]")).toContainText("1 分");
  await expect(value).toContainText(/24\.0\s*Token\/s/);
  await row.getByRole("link").focus();
  let sampledAt: string | null = null;
  for (const mode of [
    "native-desktop-performance-idle",
    "native-desktop-performance-tools",
    "native-desktop-performance-restarted",
  ] as const) {
    await page.evaluate((mode) => {
      window.activityCase = mode;
    }, mode);
    await expect(row).toContainText("最近 68.2 Token/s");
    await expect(row).toContainText("历史采样");
    const time = row.locator("time").filter({ hasText: /测得/ });
    await expect(time).toBeVisible();
    const actual = await time.getAttribute("datetime");
    if (sampledAt) expect(actual).toBe(sampledAt);
    sampledAt = actual;
    await expect(value).toContainText(/—\s*Token\/s/);
    await expect(value).not.toContainText("68.2");
    await expect(rate).toContainText("暂无实时采样");
    await expect(rate).not.toContainText("部分可测");
    await expect(summary.locator("animateMotion")).toHaveCount(0);
    await expect(row).toContainText("本轮已报输出 1,200 Token");
    await expect(row).toContainText("TTFT 1.05 秒");
    await expect(row.getByRole("link")).toBeFocused();
    await expect(feed.getByRole("row")).toHaveCount(2);
  }
  await page.evaluate(() => {
    window.activityCase = "native-desktop-performance-next";
  });
  await expect(row).not.toContainText("68.2 Token/s");
  await expect(row).not.toContainText("1,200 Token");
  await expect(row).toContainText("本轮用量待上报");
  await page.evaluate(() => {
    window.activityCase = "native-desktop-performance-completed";
  });
  await expect(row).toContainText("均速 32.0 Token/s");
  await expect(value).toContainText(/0\.0\s*Token\/s/);
});

test("parallel totals exclude samples from a different window endpoint", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await page.evaluate(() => {
    window.activityCase = "native-desktop-performance-parallel";
  });
  const summary = page.getByRole("region", {
    name: "实时任务汇总",
    exact: true,
  });
  const rate = summary
    .locator('[data-slot="metric-card"]')
    .filter({ hasText: /(?:总|已测) Token 速率/ });
  await expect(rate).toContainText("1/2 个活动来源可测 · 部分可测");
  await expect(
    rate.locator('[data-slot="metric-card-value-row"]'),
  ).toContainText(/24\.0\s*Token\/s/);
  await expect(rate).not.toContainText("124.0");
});

test("gateway usage is counted once and historical gateway rates remain separate at narrow widths", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 1000 });
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await page.evaluate(() => {
    window.activityCase = "native-desktop-performance-gateway";
  });
  await openRecentActivities(page);
  const row = page
    .getByRole("region", { name: "当前活动列表", exact: true })
    .getByRole("row")
    .filter({ hasText: "原生采集同一条消息" });
  await expect(row).toContainText("最近 68.2 Token/s");
  await expect(row).toContainText("本轮已报输出 900 Token");
  await expect(row).toContainText("其中思考 500 Token");
  await expect(row).not.toContainText("1,400 Token");
  const summary = page.getByRole("region", {
    name: "实时任务汇总",
    exact: true,
  });
  expect(await summary.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(
    true,
  );
});
