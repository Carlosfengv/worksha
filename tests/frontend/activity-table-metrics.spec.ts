import { expect, test, type Page } from "@playwright/test";
import type {} from "./activity-fixture";

const feed = (page: Page) =>
  page.getByRole("region", { name: "当前活动列表", exact: true });
const ttft = (row: ReturnType<Page["getByRole"]>) =>
  row.getByRole("cell").nth(2);
const rate = (row: ReturnType<Page["getByRole"]>) =>
  row.getByRole("cell").nth(3);
async function load(page: Page, value: Window["activityCase"]) {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await page.evaluate((value) => {
    window.activityCase = value;
  }, value);
}

test("reasoning and tool arguments stream before the conclusion, and continuations keep the message TTFT", async ({
  page,
}) => {
  await load(page, "table-metrics-output-reasoning");
  const row = feed(page).getByRole("row").filter({ hasText: "实时请求指标" });
  const summary = page.getByRole("region", {
    name: "实时任务汇总",
    exact: true,
  });
  const firstOutput = summary
    .locator('[data-slot="metric-card"]')
    .filter({ hasText: "当前平均 TTFT" });
  const totalRate = summary
    .locator('[data-slot="metric-card"]')
    .filter({ hasText: /(?:总|已测) Token 速率/ });
  await expect(rate(row)).toContainText("实时 37.0 Token/s");
  await expect(rate(row)).toContainText("最近 1 秒 · 输出估算");
  await expect(ttft(row)).toContainText("TTFT 0.900 秒");
  await expect(ttft(row)).toContainText("消息首出 · 网关实测");
  await expect(firstOutput).toContainText("0.450"); // Includes the other request's real zero.
  await expect(totalRate).toContainText("37.0");
  await row.focus();
  await page.evaluate(() => {
    window.activityCase = "table-metrics-output-tools";
  });
  await expect(rate(row)).toContainText("实时 64.0 Token/s");
  await expect(totalRate).toContainText("64.0");
  await expect(ttft(row)).toContainText("TTFT 0.900 秒");
  await expect(row).toBeFocused();
  await page.evaluate(() => {
    window.activityCase = "table-metrics-output-waiting";
  });
  await expect(rate(row)).toContainText("等待模型响应");
  await expect(rate(row)).not.toContainText("64.0 Token/s");
  await expect(ttft(row)).toContainText("TTFT 0.900 秒");
  await expect(firstOutput).toContainText("0.450");
  await page.evaluate(() => {
    window.activityCase = "table-metrics-output-new";
  });
  await expect(ttft(row)).not.toContainText("0.900 秒");
  await expect(ttft(row)).toContainText("等待首出");
  await expect(row).toHaveCount(1);
});

test("table shows per-request live TTFT and Token/s, including real zeros, and refreshes the same row", async ({
  page,
}) => {
  await load(page, "table-metrics");
  const row = feed(page).getByRole("row").filter({ hasText: "实时请求指标" });
  await expect(
    feed(page).getByRole("columnheader", { name: "TTFT", exact: true }),
  ).toBeVisible();
  await expect(
    feed(page).getByRole("columnheader", { name: "Token 速率", exact: true }),
  ).toBeVisible();
  await expect(ttft(row)).toContainText("TTFT 1.20 秒");
  await expect(ttft(row)).toContainText("网关实测 · 当前请求");
  await expect(rate(row)).toContainText("实时 20.0 Token/s");
  await expect(rate(row)).toContainText("最近 1 秒 · 文本估算");
  const zero = feed(page).getByRole("row").filter({ hasText: "table-ze" });
  await expect(ttft(zero)).toContainText("TTFT 0.000 秒");
  await expect(rate(zero)).toContainText("实时 0.0 Token/s");
  await page.evaluate(() => {
    window.activityCase = "table-metrics-updated";
  });
  await expect(rate(row)).toContainText("实时 42.0 Token/s");
  await expect(ttft(row)).toContainText("TTFT 1.20 秒");
  await expect(row).toHaveCount(1);
});

test("native-only table keeps metric columns and shows reported first-output timing without inventing speed", async ({
  page,
}) => {
  await load(page, "table-native-metrics");
  const row = feed(page).getByRole("row").filter({ hasText: "原生首出已上报" });
  await expect(ttft(row)).toContainText("TTFT 1.23 秒");
  await expect(ttft(row)).toContainText("Agent 上报 · 本轮");
  await expect(rate(row)).toContainText("原生未提供流式时序");
  await expect(rate(row)).not.toContainText("Token/s");
  const zero = feed(page).getByRole("row").filter({ hasText: "原生零首出" });
  await expect(ttft(zero)).toContainText("TTFT 0.000 秒");
  const unknown = feed(page)
    .getByRole("row")
    .filter({ hasText: "原生时序未知" });
  await expect(ttft(unknown)).toContainText("来源未提供");
  await expect(ttft(unknown)).not.toContainText(" 秒");
  await expect(
    feed(page).getByRole("columnheader", { name: "Token 速率", exact: true }),
  ).toBeVisible();
  await page.getByRole("link", { name: "查看连接与观测", exact: true }).click();
  await expect(page).toHaveURL(/#\/agents$/);
});

test("latest native completion keeps its own metrics and hides older requests in the same Session", async ({
  page,
}) => {
  await load(page, "table-history-metrics");
  const merged = feed(page)
    .getByRole("row")
    .filter({ hasText: "已结束 · 等待输入" });
  await expect(merged).toContainText("原生记录");
  await expect(ttft(merged)).toContainText("TTFT 0.900 秒");
  await expect(ttft(merged)).toContainText("最近请求");
  await expect(rate(merged)).toContainText("均速 35.0 Token/s");
  await expect(rate(merged)).toContainText("完成正文均速 · 估算");
  await expect(rate(merged)).not.toContainText("实时");
  const older = feed(page).getByRole("row").filter({ hasText: "请求已完成" });
  await expect(older).toHaveCount(0);
  const other = feed(page)
    .getByRole("row")
    .filter({ hasText: "另一会话没有指标" });
  await expect(ttft(other)).not.toContainText("0.900 秒");
  await expect(rate(other)).not.toContainText("35.0 Token/s");
  await expect(feed(page).getByRole("row")).toHaveCount(3);
});

test("native tool progress stays in one message row with the prior request metrics clearly historical", async ({
  page,
}) => {
  await load(page, "table-tool-metrics");
  const current = feed(page).getByRole("row").filter({ hasText: "原生记录" });
  await expect(current).toContainText("已提出工具调用");
  await expect(ttft(current)).toContainText("TTFT 0.900 秒");
  await expect(ttft(current)).toContainText("最近请求 · 已记录");
  await expect(rate(current)).toContainText("均速 35.0 Token/s");
  await expect(rate(current)).toContainText("完成正文均速");
  await expect(rate(current)).not.toContainText("实时");
  await expect(feed(page).getByRole("row")).toHaveCount(2);
});

test("stale live metrics clear Token/s but retain the measured first-output timing", async ({
  page,
}) => {
  await load(page, "table-metrics");
  const row = feed(page).getByRole("row").filter({ hasText: "实时请求指标" });
  await expect(rate(row)).toContainText("实时 20.0 Token/s");
  await page.evaluate(() => {
    window.activityCase = "table-metrics-stale";
  });
  await expect(row).toContainText("状态待更新");
  await expect(rate(row)).toContainText("速度待更新");
  await expect(rate(row)).not.toContainText("20.0 Token/s");
  await expect(ttft(row)).toContainText("TTFT 1.20 秒");
  await expect(ttft(row)).not.toContainText("当前请求");
});

test("public reasoning and tool arguments do not become text Token/s while TTFT remains measurable", async ({
  page,
}) => {
  await load(page, "table-metrics-reasoning");
  const row = feed(page).getByRole("row").filter({ hasText: "实时请求指标" });
  await expect(rate(row)).toContainText("推理中");
  await expect(rate(row)).not.toContainText("20.0 Token/s");
  await expect(ttft(row)).toContainText("TTFT 1.20 秒");
  await page.evaluate(() => {
    window.activityCase = "table-metrics-tools";
  });
  await expect(rate(row)).toContainText("生成工具参数");
  await expect(rate(row)).not.toContainText("20.0 Token/s");
});

test("negative, nonfinite and invalid-timing measurements stay unknown instead of zero", async ({
  page,
}) => {
  await load(page, "table-invalid-metrics");
  const issue = feed(page)
    .getByRole("row")
    .filter({ hasText: "模型请求 · timing-i" });
  await expect(ttft(issue)).toContainText("首出未记录");
  await expect(rate(issue)).toContainText("采样恢复中");
  await expect(feed(page)).not.toContainText("99.0 Token/s");
  await expect(feed(page)).not.toContainText("-20.0 Token/s");
  await expect(feed(page)).not.toContainText("Infinity");
  await expect(feed(page)).not.toContainText("NaN");
  await expect(feed(page)).not.toContainText("0.000 秒");
  await expect(feed(page)).not.toContainText("0.0 Token/s");
});

test("a partial stream keeps measured TTFT and restores speed without replacing the row", async ({
  page,
}) => {
  await load(page, "table-metrics");
  const row = feed(page).getByRole("row").filter({ hasText: "实时请求指标" });
  await expect(ttft(row)).toContainText("TTFT 1.20 秒");
  await page.evaluate(() => {
    window.activityCase = "table-metrics-gap";
  });
  await expect(rate(row)).toContainText("采样恢复中");
  await expect(rate(row)).toContainText("部分流事件缺失");
  await expect(ttft(row)).toContainText("TTFT 1.20 秒");
  await expect(feed(page)).not.toContainText("不可测");
  await page.evaluate(() => {
    window.activityCase = "table-metrics-recovered";
  });
  await expect(rate(row)).toContainText("实时 42.0 Token/s");
  await expect(ttft(row)).toContainText("TTFT 1.20 秒");
  await expect(row).toHaveCount(1);
});

test("non-streaming timings explain the missing first output while keeping measured throughput", async ({
  page,
}) => {
  await load(page, "table-nonstream-metrics");
  const row = feed(page).getByRole("row").filter({ hasText: "nonstrea" });
  await expect(ttft(row)).toContainText("首出未记录");
  await expect(ttft(row)).toContainText("非流式响应未提供时序");
  await expect(rate(row)).toContainText("吞吐 8.0 Token/s");
  await expect(rate(row)).not.toContainText("不可测");
  await expect(row.locator("time")).toHaveText(/\d+ 秒前请求/);
});

test("request ages advance through seconds, minutes and hours while an overview read is pending", async ({
  page,
}) => {
  const now = new Date("2026-10-24T02:00:00Z");
  await page.clock.install({ time: now });
  await load(page, "table-metrics");
  await expect(
    feed(page).getByRole("columnheader", { name: "TTFT", exact: true }),
  ).toBeVisible();
  await page.clock.pauseAt(new Date(now.getTime() + 60000));
  await page.evaluate(
    (startedAt) => {
      window.activityRequestAt = startedAt;
      document.dispatchEvent(new Event("visibilitychange"));
    },
    new Date(now.getTime() + 1000).toISOString(),
  );
  const row = feed(page).getByRole("row").filter({ hasText: "实时请求指标" });
  const time = row.locator("time");
  await expect(time).toHaveText("59 秒前请求");
  await expect(time).toHaveAttribute("datetime", "2026-10-24T02:00:01.000Z");
  await expect(time).toHaveAttribute("title", /2026\/10\/24.*10:00:01/);
  await page.evaluate(() => {
    window.overviewDelay = 3600000;
  });
  await page.clock.fastForward(2000);
  await expect(time).toHaveText("1 分钟前请求");
  await page.clock.fastForward(3540000);
  await expect(time).toHaveText("1 小时前请求");
  await expect(rate(row)).not.toContainText("20.0 Token/s");
  await expect(ttft(row)).toContainText("TTFT 1.20 秒");
  await expect(row).toHaveCount(1);
});

test("native-only rows label the event age as activity and never invent a request timestamp", async ({
  page,
}) => {
  await load(page, "table-native-metrics");
  const row = feed(page).getByRole("row").filter({ hasText: "原生首出已上报" });
  await expect(row.locator("time")).toHaveText(/\d+ 秒前活动/);
  await expect(row.locator("time")).not.toContainText("请求");
});
