import { expect, test, type Page } from "@playwright/test";
import type {} from "./activity-fixture";

const summary = (page: Page) =>
  page.getByRole("region", { name: "实时任务汇总", exact: true });
const sessions = (page: Page) =>
  summary(page)
    .locator('[data-slot="metric-card"]')
    .filter({ hasText: "执行中的 Session" });
const rate = (page: Page) =>
  summary(page)
    .locator('[data-slot="metric-card"]')
    .filter({ hasText: /(?:总|已测) Token 速率/ });
async function scenario(page: Page, value: Window["activityCase"]) {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await page.evaluate((value) => {
    window.activityCase = value;
  }, value);
}

test("unconfigured native observation keeps an empty gateway feed unknown and offers the scoped consent entry", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&connection=official&sourceState=disabled&activity=empty#/overview",
  );
  await expect(summary(page)).toContainText("仅网关");
  await expect(summary(page)).toContainText("当前执行状态待确认");
  await expect(
    sessions(page).locator('[data-slot="metric-card-value-row"]'),
  ).toContainText("—");
  await expect(rate(page)).not.toContainText(/0\.0\s*Token\/s/);
  await expect(summary(page)).not.toContainText("空闲");
  await page.getByRole("combobox", { name: "首页筛选 Agent" }).click();
  await page.getByRole("option", { name: "Codex 桌面端", exact: true }).click();
  await summary(page).getByRole("link", { name: "开启原生观测" }).click();
  await expect(page).toHaveURL(/#\/agents\/codex-desktop\?tab=connection$/);
});

test("gateway measurements remain live while native observation is not enabled", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&sourceState=disabled#/overview",
  );
  await page.getByRole("combobox", { name: "首页筛选 Agent" }).click();
  await page
    .getByRole("option", { name: "Claude Code CLI", exact: true })
    .click();
  await expect(summary(page)).toContainText("仅网关");
  await expect(summary(page)).toContainText("当前仅统计经本应用的网关请求");
  await expect(
    sessions(page).locator('[data-slot="metric-card-value-row"]'),
  ).toHaveText(/\s*2\s*个\s*/);
  await expect(rate(page)).toContainText(/60\.0\s*Token\/s/);
});

test("summary counts Sessions once and sums simultaneous measured text rates", async ({
  page,
}) => {
  await scenario(page, "parallel");
  await expect(
    sessions(page).locator('[data-slot="metric-card-value-row"]'),
  ).toContainText(/10\s*个/);
  await expect(sessions(page)).toContainText("另有 2 个网关请求待关联 Session");
  await expect(rate(page)).toContainText("已测 Token 速率");
  await expect(rate(page)).toContainText(/1159\.0\s*Token\/s/);
  await expect(rate(page)).toContainText("6/12 个活动来源可测 · 部分可测");
  const models = summary(page).getByRole("list", { name: "执行中的模型" });
  await expect(models.getByRole("listitem")).toHaveCount(2);
  await expect(
    models.getByRole("listitem").filter({ hasText: "deepseek-v4-pro" }),
  ).toContainText("40.0 Token/s");
  await expect(summary(page).locator("animateMotion").first()).toBeAttached();
  await summary(page).getByRole("button", { name: "暂停活动流动效果" }).click();
  await expect(summary(page).locator("animateMotion")).toHaveCount(0);
  await expect(
    summary(page).getByRole("button", { name: "播放活动流动效果" }),
  ).toHaveAttribute("aria-pressed", "true");
});

test("message steps remain one executing Session and completion clears the live summary", async ({
  page,
}) => {
  await scenario(page, "message-row-streaming");
  await expect(
    sessions(page).locator('[data-slot="metric-card-value-row"]'),
  ).toContainText(/1\s*个/);
  await expect(rate(page)).toContainText(/42\.0\s*Token\/s/);
  await page.evaluate(() => {
    window.activityCase = "message-row-completed";
  });
  await expect(
    sessions(page).locator('[data-slot="metric-card-value-row"]'),
  ).toContainText(/0\s*个/);
  await expect(rate(page)).toContainText(/0\.0\s*Token\/s/);
  await expect(
    summary(page).getByText("当前没有观测到正在执行的任务。", { exact: true }),
  ).toBeVisible();
  await expect(
    page
      .getByRole("region", { name: "当前活动列表", exact: true })
      .getByRole("row"),
  ).toHaveCount(2);
});

test("native-only running tasks keep unknown rates distinct from zero", async ({
  page,
}) => {
  await scenario(page, "table-native-metrics");
  await expect(
    sessions(page).locator('[data-slot="metric-card-value-row"]'),
  ).toContainText(/1\s*个/);
  await expect(rate(page)).toContainText(/—\s*Token\/s/);
  await expect(rate(page)).toContainText("当前来源未提供可测的 Token 增量");
  await expect(rate(page)).not.toContainText(/0\.0\s*Token\/s/);
  await expect(summary(page).locator("animateMotion")).toHaveCount(0);
});

test("rate metrics keep numbers and units separate from thinking and tool states", async ({
  page,
}) => {
  await scenario(page, "native-desktop-output-thinking-silent");
  const value = rate(page).locator('[data-slot="metric-card-value-row"]');
  const models = summary(page).getByRole("list", { name: "执行中的模型" });
  for (const mode of [
    "native-desktop-output-thinking-silent",
    "native-desktop-output-thinking-unknown",
    "native-desktop-output-tool-idle",
    "native-desktop-output-returned",
    "native-desktop-output-reconnecting",
  ] as const) {
    await page.evaluate((mode) => {
      window.activityCase = mode;
    }, mode);
    await expect(value).toContainText(/—\s*Token\/s/);
    await expect(rate(page)).not.toContainText(
      /思考中|执行中|工具处理中|待采样/,
    );
    await expect(rate(page)).not.toContainText("0.0 Token/s");
    await expect(models).toContainText("— Token/s");
    await expect(summary(page).locator("animateMotion")).toHaveCount(0);
  }
  await page.evaluate(() => {
    window.activityCase = "native-desktop-output-thinking";
  });
  await expect(value).toContainText(/37\.0\s*Token\/s/);
  await expect(rate(page)).not.toContainText("当前来源未提供可测的 Token 增量");
  await page.evaluate(() => {
    window.activityCase = "native-desktop-output-completed";
  });
  await expect(value).toContainText(/0\.0\s*Token\/s/);
  await expect(rate(page)).not.toContainText(/思考中|执行中|工具处理中|待采样/);
});

test("Agent filtering changes both the summary and model branches", async ({
  page,
}) => {
  await scenario(page, "parallel");
  await page.getByRole("combobox", { name: "首页筛选 Agent" }).click();
  await page
    .getByRole("option", { name: "Claude Code CLI", exact: true })
    .click();
  await expect(
    sessions(page).locator('[data-slot="metric-card-value-row"]'),
  ).toContainText(/2\s*个/);
  await expect(rate(page)).toContainText("总 Token 速率");
  await expect(rate(page)).toContainText(/60\.0\s*Token\/s/);
  await expect(rate(page)).not.toContainText("部分可测");
});

test("stale and failed snapshots never present cached counts or rates as live", async ({
  page,
}) => {
  await scenario(page, "parallel");
  await expect(rate(page)).toContainText(/1159\.0\s*Token\/s/);
  await page.evaluate(() => {
    window.activityCase = "stale";
  });
  await expect(summary(page)).toContainText(
    "快照已过期，当前任务与速率待更新。",
  );
  await expect(rate(page)).not.toContainText("1159.0");
  await expect(
    sessions(page).locator('[data-slot="metric-card-value-row"]'),
  ).toContainText("—");
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&activity=error#/overview",
  );
  await expect(summary(page)).toContainText(
    "暂时无法读取当前任务，恢复后自动更新。",
  );
  await expect(
    sessions(page).locator('[data-slot="metric-card-value-row"]'),
  ).toContainText("—");
});

test("narrow layout and reduced motion preserve readable model numbers", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 900 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await scenario(page, "parallel");
  await expect(rate(page)).toContainText(/1159\.0\s*Token\/s/);
  await expect(summary(page).locator("animateMotion")).toHaveCount(0);
  expect(
    await summary(page).evaluate((el) => el.scrollWidth <= el.clientWidth),
  ).toBe(true);
});
