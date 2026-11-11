import { expect, test, type Page } from "@playwright/test";
import type {} from "./activity-fixture";

const url =
  "/tests/frontend/tables.html?mode=activity&activity=recent-history#/overview";
const feed = (page: Page) =>
  page.getByRole("region", { name: "当前活动列表", exact: true });
const titles = (page: Page) =>
  feed(page)
    .getByRole("row")
    .evaluateAll((rows) =>
      rows.slice(1).map((row) => row.querySelector("p")?.textContent),
    );

test("activity rows expire after three hours of no business activity, including during read errors", async ({
  page,
}) => {
  const now = new Date("2026-10-24T04:00:00Z");
  await page.clock.install({ time: now });
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&activity=recent-history-window#/overview",
  );
  await expect(
    feed(page).getByRole("columnheader", { name: "TTFT", exact: true }),
  ).toBeVisible();
  await page.clock.pauseAt(new Date(now.getTime() + 60000));
  await page.evaluate(() =>
    document.dispatchEvent(new Event("visibilitychange")),
  );
  await expect
    .poll(() => titles(page))
    .toEqual(["较早原生活动", "模型请求 · recent-f", "持续任务"]);
  await expect(feed(page)).not.toContainText("较早网关活动");
  await expect(feed(page)).not.toContainText("最近原生已结束");
  // The request began four hours ago but has fresh output five seconds ago.
  await expect(
    feed(page).getByRole("row").filter({ hasText: "持续任务" }),
  ).toContainText("4 小时前请求");
  await page.evaluate(() => {
    window.activityCase = "error";
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await expect(
    page.getByText("实时活动快照已过期，保留最近记录；进行中状态待确认。", {
      exact: true,
    }),
  ).toBeVisible();
  await page.clock.fastForward(1000);
  await expect.poll(() => titles(page)).toEqual(["持续任务"]);
  await page.clock.fastForward(3 * 60 * 60 * 1000);
  await expect(feed(page).getByRole("row")).toHaveCount(2);
  await expect(feed(page)).not.toContainText("持续任务");
});

test("an expired-only snapshot does not show old activity through the empty-state fallback", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&activity=recent-history-expired#/overview",
  );
  await expect(feed(page)).toContainText("暂无活动记录");
  await expect(feed(page)).toContainText("仅显示最近 3 小时的活动");
  await expect(feed(page)).not.toContainText("最近观测：");
  await expect(
    feed(page).getByRole("button", { name: "查看最近记录", exact: true }),
  ).toHaveCount(0);
  await expect(feed(page)).not.toContainText("持续任务");
});

test("activity history sorts by message start, keeps that order on later steps and retains unknown clients", async ({
  page,
}) => {
  await page.goto(url);
  await expect
    .poll(() => titles(page))
    .toEqual([
      "最近原生已结束",
      "模型请求 · recent-f",
      "持续任务",
      "较早网关活动",
      "较早原生活动",
    ]);
  const ended = feed(page)
    .getByRole("row")
    .filter({ hasText: "最近原生已结束" });
  await expect(ended).toContainText("已结束 · 等待输入");
  await expect(ended).toContainText("查看会话上下文");
  await expect(ended).not.toContainText("10.0%");
  const old = feed(page).getByRole("row").filter({ hasText: "较早网关活动" });
  await expect(old).toContainText("均速 35.0 Token/s");
  const unknown = feed(page)
    .getByRole("row")
    .filter({ hasText: "较早原生活动" });
  await expect(unknown).toContainText("客户端来源未知");
  await expect(unknown).toContainText("状态待更新");
  await page.evaluate(() => {
    window.activityCase = "recent-history-reordered";
  });
  await expect
    .poll(() => titles(page))
    .toEqual([
      "最近原生已结束",
      "模型请求 · recent-f",
      "持续任务",
      "较早网关活动",
      "较早原生活动",
    ]);
  await page.getByRole("combobox", { name: "首页筛选 Agent" }).click();
  await page
    .getByRole("option", { name: "客户端来源未知", exact: true })
    .click();
  await expect
    .poll(() => titles(page))
    .toEqual(["模型请求 · recent-f", "较早原生活动"]);
  await expect(feed(page)).not.toContainText("持续任务");
});

test("a completed request keeps its message position with historical metrics and context link", async ({
  page,
}) => {
  await page.clock.install({ time: new Date("2026-10-24T04:00:00Z") });
  await page.goto(url);
  const task = feed(page).getByRole("row").filter({ hasText: "持续任务" });
  await expect(task).toContainText("正在输出正文");
  await expect(task).toContainText("20.0 Token/s");
  await page.evaluate(() => {
    window.activityCase = "recent-history-completed";
  });
  await page.clock.fastForward(1500);
  await expect(task).toContainText("请求已完成");
  await expect(task).toContainText("均速 36.0 Token/s");
  await expect
    .poll(() => titles(page))
    .toEqual([
      "最近原生已结束",
      "模型请求 · recent-f",
      "持续任务",
      "较早网关活动",
      "较早原生活动",
    ]);
  await task
    .getByRole("link", { name: "查看会话上下文：claude:running", exact: true })
    .click();
  await expect(page).toHaveURL(/#\/sessions\/claude%3Arunning\?view=context$/);
  await expect(
    page.getByRole("tab", { name: "上下文", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
});

test("a read failure preserves history, makes ongoing status uncertain and never labels the last live speed as a historical average", async ({
  page,
}) => {
  await page.goto(url);
  const task = feed(page).getByRole("row").filter({ hasText: "持续任务" });
  await expect(task).toContainText("20.0 Token/s");
  await page.evaluate(() => {
    window.activityCase = "error";
  });
  await expect(
    page.getByText("实时活动快照已过期，保留最近记录；进行中状态待确认。", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(task).toContainText("状态待更新");
  await expect(task).not.toContainText("20.0 Token/s");
  await expect(task).not.toContainText("正在输出正文");
  await expect(feed(page).getByRole("row")).toHaveCount(6);
  await expect(feed(page)).toContainText("均速 35.0 Token/s");
  await page.evaluate(() => {
    window.activityCase = "recent-history";
  });
  await expect(task).toContainText("正在输出正文");
  await page.evaluate(() => {
    window.activityCase = "empty";
  });
  await expect(feed(page)).toContainText("暂无活动记录");
});
