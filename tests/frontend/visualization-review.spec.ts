import { expect, test } from "@playwright/test";
import type {} from "./activity-fixture";

const overview =
  "/tests/frontend/tables.html?mode=activity&visuals=1#/overview";

test("completed activities stay visible with saved metrics and open the recorded session", async ({
  page,
}) => {
  await page.goto(overview);
  const activity = page.getByRole("region", {
    name: "当前活动列表",
    exact: true,
  });
  await expect(activity).toContainText("修复登录");
  await page.evaluate(() => {
    window.activityCase = "completed";
  });
  const retained = activity.getByRole("row").filter({ hasText: "修复登录" });
  await expect(retained).toContainText("请求已完成");
  await expect(retained).toContainText("均速 35.0 Token/s");
  await expect(activity.getByRole("combobox", { name: "每页条数" })).toHaveText(
    "5",
  );
  await expect(activity.getByRole("row")).toHaveCount(6);
  await retained.click();
  await expect(page).toHaveURL(/#\/sessions\/claude%3Aa/);
});

test("first activity read failure is distinct from loading and recovers when polling succeeds", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&activity=error#/overview",
  );
  const activity = page.getByRole("region", {
    name: "当前活动列表",
    exact: true,
  });
  await expect(activity).toContainText("暂时无法读取实时活动");
  await expect(activity).not.toContainText("正在读取实时活动");
  await expect(activity).not.toContainText("暂无活动记录");
  await page.evaluate(() => {
    window.activityCase = "parallel";
  });
  await expect(activity).toContainText("修复登录");
  await expect(activity.getByRole("combobox", { name: "每页条数" })).toHaveText(
    "5",
  );
  await expect(activity.getByRole("row")).toHaveCount(6);
});

test("expired snapshots retain records while clearing claims of running activity", async ({
  page,
}) => {
  await page.goto(overview);
  const activity = page.getByRole("region", {
    name: "当前活动列表",
    exact: true,
  });
  await expect(activity).toContainText("修复登录");
  await page.evaluate(() => {
    window.activityCase = "stale";
  });
  await expect(
    page.getByText("实时活动快照已过期，保留最近记录；进行中状态待确认。", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(activity).toContainText("状态待更新");
  await expect(activity.getByRole("combobox", { name: "每页条数" })).toHaveText(
    "5",
  );
  await expect(activity.getByRole("row")).toHaveCount(6);
  await page.evaluate(() => {
    window.activityCase = "parallel";
  });
  await expect(activity).not.toContainText("状态待更新");
  await expect(activity).toContainText("修复登录");
});

test("an Agent without observed activity offers source management without fabricating records", async ({
  page,
}) => {
  await page.goto(overview);
  await page.getByRole("combobox", { name: "首页筛选 Agent" }).click();
  await page.getByRole("option", { name: "Antigravity", exact: true }).click();
  const activity = page.getByRole("region", {
    name: "当前活动列表",
    exact: true,
  });
  await expect(activity).toContainText("尚未连接原生观测来源");
  await expect(activity).not.toContainText("最近观测：");
  await activity
    .getByRole("button", { name: "连接观测来源", exact: true })
    .click();
  await expect(page).toHaveURL(/#\/agents\/antigravity\?tab=connection$/);
});

test("disconnected native sources retain gateway history and explain a truly empty feed", async ({
  page,
}) => {
  await page.goto(
    `${overview.replace("#/overview", "")}&sourceState=disabled#/overview`,
  );
  await page.evaluate(() => {
    window.activityCase = "completed";
  });
  const activity = page.getByRole("region", {
    name: "当前活动列表",
    exact: true,
  });
  await expect(activity).toContainText("请求已完成");
  await page.evaluate(() => {
    window.activityCase = "empty";
  });
  await expect(activity).toContainText("尚未连接原生观测来源");
  await expect(activity).toContainText("当前范围未开启原生会话读取");
  await activity.getByRole("button", { name: "连接观测来源" }).click();
  await expect(page).toHaveURL(/#\/agents$/);
});

test("gateway activity remains visible without native source authorization", async ({
  page,
}) => {
  await page.goto(
    `${overview.replace("#/overview", "")}&sourceState=disabled#/overview`,
  );
  const activity = page.getByRole("region", {
    name: "当前活动列表",
    exact: true,
  });
  await expect(activity).toContainText("Token/s");
  await expect(activity.getByRole("combobox", { name: "每页条数" })).toHaveText(
    "5",
  );
  await expect(activity.getByRole("row")).toHaveCount(6);
  await expect(activity).not.toContainText("尚未连接原生观测来源");
});

test("native activity appears on polling and opens its recorded session without a running gateway request", async ({
  page,
}) => {
  await page.goto(overview);
  await page.evaluate(() => {
    window.activityCase = "completed";
  });
  await page.getByRole("combobox", { name: "首页筛选 Agent" }).click();
  await page.getByRole("option", { name: "OpenCode", exact: true }).click();
  const activity = page.getByRole("region", {
    name: "当前活动列表",
    exact: true,
  });
  await expect(activity).toContainText("请求已完成");
  await page.evaluate(() => {
    window.activityCase = "opencode-tool";
  });
  const task = activity
    .getByRole("row")
    .filter({ hasText: "OpenCode 已关联会话" })
    .filter({ hasText: "原生记录" });
  await expect(task).toContainText("工具调用");
  await expect(task).toContainText("原生记录");
  await expect(task).toContainText("TTFT 1.20 秒");
  await expect(activity.getByRole("row")).toHaveCount(2);
  await expect(task).not.toContainText("消息归属待确认");
  await task.click();
  await expect(page).toHaveURL(/#\/sessions\/opencode%3Ases_verified/);
});

for (const [metric, label] of [
  ["tokens", "Token 用量"],
  ["cost", "费用"],
]) {
  test(`overview model drilldown retains ${metric}, source, displayed window and selected Agent`, async ({
    page,
  }) => {
    await page.goto(overview);
    await page.getByRole("combobox", { name: "首页筛选 Agent" }).click();
    await page
      .getByRole("option", { name: "Claude Code CLI", exact: true })
      .click();
    await page.getByRole("tab", { name: label, exact: true }).click();
    const ranking = page.getByRole("region", {
      name: `模型占比 · ${label}`,
      exact: true,
    });
    await expect(
      ranking.getByRole("button", { name: /^fixture-model-1 / }),
    ).toBeVisible();
    await ranking.getByRole("button", { name: /^fixture-model-1 / }).click();
    const params = new URLSearchParams(new URL(page.url()).hash.split("?")[1]);
    expect(params.get("metric")).toBe(metric);
    expect(params.get("source")).toBe("native");
    expect(params.get("agent")).toBe("claude-code-cli");
    expect(params.get("model")).toBe("fixture-model-1");
    expect(params.get("dimension")).toBe("model");
    expect(params.get("range")).toBe("custom");
    expect(params.get("start")).toBe("2026-10-22T04:00:00.000Z");
    expect(params.get("end")).toBe("2026-10-23T04:00:00.000Z");
  });
}
