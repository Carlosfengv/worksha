import { expect, test } from "@playwright/test";
import { openRecentActivities } from "./overview-helpers";
import type {} from "./activity-fixture";

test("native thinking and tool phases update output rates before completion while retaining the message TTFT", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openRecentActivities(page);
  await page.evaluate(() => {
    window.activityCase = "native-desktop-output-thinking";
  });
  const feed = page.getByRole("region", { name: "当前活动列表", exact: true });
  const row = feed.getByRole("row").filter({ hasText: "原生采集同一条消息" });
  const summary = page.getByRole("region", {
    name: "实时任务汇总",
    exact: true,
  });
  await expect(row).toContainText("思考中");
  await expect(row).toContainText("TTFT 1.05 秒");
  await expect(row).toContainText("实时 37.0 Token/s");
  await expect(row).toContainText("最近 1 秒 · 输出估算");
  await expect(summary).toContainText("37.0 Token/s");
  await row.getByRole("link").focus();
  for (const [mode, phase, rateLabel] of [
    ["native-desktop-output-tools", "Read 工具执行中", "实时 12.5 Token/s"],
    ["native-desktop-output-tool-idle", "Read 工具执行中", "工具处理中"],
    [
      "native-desktop-output-returned",
      "Read 已返回，等待模型继续",
      "工具处理中",
    ],
  ] as const) {
    await page.evaluate((mode) => {
      window.activityCase = mode;
    }, mode);
    await expect(row).toContainText(phase);
    await expect(row.getByRole("cell").nth(4)).toContainText(rateLabel);
    await expect(summary).toContainText(rateLabel.replace("实时 ", ""));
    await expect(row).not.toContainText("0.0 Token/s");
    if (mode !== "native-desktop-output-tools")
      await expect(row).toContainText("原生未上报生成增量");
    await expect(row).toContainText("TTFT 1.05 秒");
    await expect(row.getByRole("link")).toBeFocused();
    await expect(feed.getByRole("row")).toHaveCount(2);
  }
  await page.evaluate(() => {
    window.activityCase = "native-desktop-output-reconnecting";
  });
  await expect(row).toContainText("原生重连中");
  await expect(row).not.toContainText("实时 37.0 Token/s");
  await expect(row).toContainText("TTFT 1.05 秒");
  await page.evaluate(() => {
    window.activityCase = "native-desktop-output-completed";
  });
  await expect(row).toContainText("均速 32.0 Token/s");
  await expect(row).not.toContainText("实时 32.0 Token/s");
  await page.evaluate(() => {
    window.activityCase = "native-desktop-output-next";
  });
  await expect(row).toContainText("等待首 Token");
  await expect(row).not.toContainText("TTFT 1.05 秒");
  await expect(row).not.toContainText("32.0 Token/s");
});

test("native silence stays unknown in the summary and table instead of displaying zero", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openRecentActivities(page);
  const summary = page.getByRole("region", { name: "实时任务汇总" });
  const row = page
    .getByRole("region", { name: "当前活动列表", exact: true })
    .getByRole("row")
    .filter({ hasText: "原生采集同一条消息" });
  for (const [mode, label] of [
    ["native-desktop-output-thinking-silent", "思考中"],
    ["native-desktop-output-thinking-unknown", "思考中"],
    ["native-desktop-output-streaming-silent", "原生未上报"],
  ] as const) {
    await page.evaluate((mode) => {
      window.activityCase = mode;
    }, mode);
    await expect(row.getByRole("cell").nth(4)).toContainText(label);
    await expect(row).toContainText("原生未上报生成增量");
    await expect(row).toContainText("TTFT 1.05 秒");
    await expect(summary).toContainText(label);
    await expect(summary).toContainText("0/1 个活动来源可测");
    await expect(summary).not.toContainText("0.0 Token/s");
    await expect(row).not.toContainText("0.0 Token/s");
  }
  await page.evaluate(() => {
    window.activityCase = "native-desktop-output-thinking";
  });
  await expect(row).toContainText("实时 37.0 Token/s");
  await expect(summary).toContainText("37.0 Token/s");
  await expect(summary).toContainText("1/1 个活动来源可测");
});

test("native desktop text updates summary and the same message row without a gateway", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openRecentActivities(page);
  await page.evaluate(() => {
    window.activityCase = "native-desktop-live";
  });
  const summary = page.getByRole("region", { name: "实时任务汇总" });
  const feed = page.getByRole("region", { name: "当前活动列表", exact: true });
  const row = feed.getByRole("row").filter({ hasText: "原生采集同一条消息" });
  await expect(summary).toContainText("20.0 Token/s");
  await expect(summary).not.toContainText("待采样");
  await expect(row).toContainText("TTFT 1.05 秒");
  await expect(row).toContainText("原生首 Token · 实时观测");
  await expect(row).toContainText("实时 20.0 Token/s");
  await expect(row).not.toContainText("来源未提供流式");
  await expect(feed.getByRole("row")).toHaveCount(2);
  await page.evaluate(() => {
    window.activityCase = "native-desktop-updated";
  });
  await expect(summary).toContainText("42.0 Token/s");
  await expect(row).toContainText("实时 42.0 Token/s");
  await expect(row).toHaveCount(1);
  await expect(
    feed.getByRole("row").filter({ hasText: "同会话上一条消息" }),
  ).toHaveCount(0);
  await page.evaluate(() => {
    window.activityCase = "native-desktop-thinking";
  });
  await expect(summary).toContainText("思考中");
  await expect(row.getByRole("cell").nth(4)).toContainText("思考中");
  await expect(summary).not.toContainText("42.0 Token/s");
  await expect(row).toContainText("TTFT 1.05 秒");
  await page.evaluate(() => {
    window.activityCase = "native-desktop-reconnecting";
  });
  await expect(summary).toContainText("原生重连中");
  await expect(row).toContainText("原生重连中");
  await expect(row).not.toContainText("实时 20.0 Token/s");
  await page.evaluate(() => {
    window.activityCase = "native-desktop-completed";
  });
  await expect(row).toContainText("TTFT 1.00 秒");
  await expect(row).toContainText("Agent 上报 · 本轮");
  await expect(row).toContainText("均速 32.0 Token/s");
  await expect(summary).toContainText("当前没有观测到正在执行的任务。");
});

test("the first native reasoning token updates TTFT while the same turn is still thinking", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openRecentActivities(page);
  await page.evaluate(() => {
    window.activityCase = "native-desktop-waiting";
  });
  const summary = page.getByRole("region", { name: "实时任务汇总" });
  const row = page
    .getByRole("region", { name: "当前活动列表", exact: true })
    .getByRole("row")
    .filter({ hasText: "原生采集同一条消息" });
  await expect(row).toContainText("等待首 Token");
  await expect(row).not.toContainText("首项输出");
  await expect(row).not.toContainText("TTFT 0.900");
  await page.evaluate(() => {
    window.activityCase = "native-desktop-first-token";
  });
  await expect(row).toContainText("TTFT 1.05 秒");
  await expect(row).toContainText("原生首 Token · 实时观测");
  await expect(row).toContainText("思考中");
  await expect(row).not.toContainText("请求已完成");
  await expect(summary).toContainText("当前平均 TTFT");
  await expect(summary).toContainText("1.05");
  await expect(summary).toContainText("1/1 个活动来源已记录");
  await page.evaluate(() => {
    window.activityCase = "native-desktop-updated";
  });
  await expect(row).toContainText("实时 42.0 Token/s");
  await expect(row).toContainText("TTFT 1.05 秒");
  await expect(row).toHaveCount(1);
});

test("joining after the native first token explicitly waits for attribution instead of timing later text", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openRecentActivities(page);
  await page.evaluate(() => {
    window.activityCase = "native-desktop-late";
  });
  const row = page
    .getByRole("region", { name: "当前活动列表", exact: true })
    .getByRole("row")
    .filter({ hasText: "原生采集同一条消息" });
  await expect(row).toContainText("TTFT 待补报");
  await expect(row).toContainText("中途接入 · 待原生补报");
  await expect(row).not.toContainText("TTFT 0.900");
  await expect(row).not.toContainText("TTFT 1.05");
  await expect(
    page.getByRole("region", { name: "实时任务汇总" }),
  ).toContainText("待原生补报");
});

test("restored native TTFT remains visible through reconnection and new text resumes live rates", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openRecentActivities(page);
  await page.evaluate(() => {
    window.activityCase = "native-desktop-restored";
  });
  const summary = page.getByRole("region", { name: "实时任务汇总" });
  const row = page
    .getByRole("region", { name: "当前活动列表", exact: true })
    .getByRole("row")
    .filter({ hasText: "原生采集同一条消息" });
  await expect(row).toContainText("TTFT 1.05 秒");
  await expect(row).toContainText("原生重连中");
  await expect(row).not.toContainText("待原生补报");
  await expect(row).not.toContainText("实时 42.0 Token/s");
  await expect(summary).toContainText("1/1 个活动来源已记录");
  await page.evaluate(() => {
    window.activityCase = "native-desktop-restored-live";
  });
  await expect(row).toContainText("TTFT 1.05 秒");
  await expect(row).toContainText("实时 42.0 Token/s");
  await expect(summary).toContainText("42.0 Token/s");
  await expect(row).not.toContainText("待原生补报");
  await expect(row).toHaveCount(1);
});

test("completed native averages survive restart and an offline owner without becoming live rates", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openRecentActivities(page);
  const feed = page.getByRole("region", { name: "当前活动列表", exact: true });
  const row = feed.getByRole("row").filter({ hasText: "原生采集同一条消息" });
  for (const scenario of [
    "native-desktop-restored-completed",
    "native-desktop-restored-completed-offline",
  ] as const) {
    await page.evaluate((value) => {
      window.activityCase = value;
    }, scenario);
    await expect(row).toContainText("均速 32.0 Token/s");
    await expect(row).toContainText("TTFT 1.05 秒");
    await expect(row).not.toContainText("速率未记录");
    await expect(row).not.toContainText("实时 32.0 Token/s");
    await expect(row).toHaveCount(1);
    const summary = page.getByRole("region", { name: "实时任务汇总" });
    await expect(summary).toContainText("当前没有观测到正在执行的任务。");
    await expect(summary).not.toContainText("32.0 Token/s");
  }
  await expect(
    feed.getByRole("row").filter({ hasText: "同会话上一条消息" }),
  ).toHaveCount(0);
});
