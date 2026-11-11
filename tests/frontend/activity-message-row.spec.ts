import { expect, test, type Page } from "@playwright/test";
import { openRecentActivities } from "./overview-helpers";
import type {} from "./activity-fixture";
const feed = (page: Page) =>
  page.getByRole("region", { name: "当前活动列表", exact: true });
const rows = (page: Page) =>
  feed(page)
    .getByRole("row")
    .filter({ has: page.getByRole("cell") });
async function mode(page: Page, value: Window["activityCase"]) {
  await page.evaluate((value) => {
    window.activityCase = value;
  }, value);
}
async function load(page: Page) {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await mode(page, "message-row-thinking");
  await openRecentActivities(page);
  await expect(rows(page)).toHaveCount(1);
}
test("one message keeps the same row through thinking, tools, returns, seven requests and completion", async ({
  page,
}) => {
  await load(page);
  const row = rows(page).first();
  await expect(row).toContainText("思考中");
  const elapsed = row.locator("[data-activity-elapsed]");
  const stamp = await elapsed.getAttribute("title");
  await row.getByRole("link").focus();
  for (const [value, text] of [
    ["message-row-tools", "已提出调用：Read、Grep"],
    ["message-row-returned", "Read 已返回；仍待返回：Grep"],
    ["message-row-streaming", "实时 42.0 Token/s"],
    ["message-row-completed", "已结束 · 等待输入"],
  ] as const) {
    await mode(page, value);
    await expect(row).toContainText(text);
    await expect(rows(page)).toHaveCount(1);
    await expect(row.getByRole("link")).toBeFocused();
    await expect(elapsed).toHaveAttribute("title", stamp!);
  }
  await expect(row.getByRole("cell").first().locator("p").first()).toHaveText(
    "检查登录",
  );
  await expect(row).toContainText("会话：消息归并会话");
  await expect(row).toContainText("均速 36.0 Token/s");
  await expect(
    page.getByRole("tab", { name: /最近 3 小时.*1/ }),
  ).toHaveAttribute("aria-selected", "true");
});
test("the next message replaces the same Session row without reusing the previous metrics or losing focus", async ({
  page,
}) => {
  await load(page);
  await mode(page, "message-row-completed");
  await expect(rows(page)).toHaveCount(1);
  const row = rows(page).first();
  await expect(row).toContainText("TTFT 0.700 秒");
  await expect(row).toContainText("均速 36.0 Token/s");
  await row.getByRole("link").focus();
  await mode(page, "message-row-next");
  await expect(rows(page)).toHaveCount(1);
  await expect(row).toContainText("收到新的用户消息");
  await expect(row).not.toContainText("已结束 · 等待输入");
  await expect(row).not.toContainText("TTFT 0.700 秒");
  await expect(row).not.toContainText("36.0 Token/s");
  await expect(row.getByRole("link")).toBeFocused();
  await expect(row.getByRole("cell").first().locator("p").first()).toHaveText(
    "检查登录",
  );
});
test("a stale or failed read keeps one message row without a stale live rate", async ({
  page,
}) => {
  await load(page);
  await mode(page, "message-row-streaming");
  await expect(rows(page).first()).toContainText("实时 42.0 Token/s");
  await mode(page, "message-row-stale");
  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).first()).toContainText("状态待更新");
  await expect(rows(page).first()).not.toContainText("42.0 Token/s");
  await mode(page, "error");
  await expect(feed(page)).toContainText("状态待更新");
  await expect(rows(page)).toHaveCount(1);
});
test("legacy requests in the same Session display only the latest resolved user message", async ({
  page,
}) => {
  await load(page);
  await mode(page, "message-row-legacy");
  await expect(rows(page)).toHaveCount(1);
  await expect(
    rows(page).first().getByRole("cell").first().locator("p").first(),
  ).toHaveText("检查登录");
  await expect(
    rows(page).first().locator("[data-activity-elapsed]"),
  ).toHaveAttribute("title", /本轮开始/);
  await expect(feed(page)).not.toContainText("核对设置");
  await expect(feed(page)).not.toContainText("消息归属待确认");
});
