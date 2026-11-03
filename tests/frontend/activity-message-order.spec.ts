import { expect, test, type Page } from "@playwright/test";
import type {} from "./activity-fixture";

const feed = (page: Page) =>
  page.getByRole("region", { name: "当前活动列表", exact: true });
const titles = (page: Page) =>
  feed(page)
    .getByRole("row")
    .evaluateAll((rows) =>
      rows.slice(1).map((row) => row.querySelector("p")?.textContent),
    );
const initial = [
  "检查新的请求",
  "网关会话",
  "检查较早的请求",
  "模型请求 · order-di",
  "可读摘要，消息开始时间未知",
];
async function load(
  page: Page,
  mode: Window["activityCase"] = "message-order",
) {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await page.evaluate((mode) => {
    window.activityCase = mode;
  }, mode);
}

test("message starts order sessions while thinking, tools, text and completion only update their rows", async ({
  page,
}) => {
  await load(page);
  await expect.poll(() => titles(page)).toEqual(initial);
  const older = feed(page).getByRole("row").filter({ hasText: "较早消息" });
  const gateway = feed(page).getByRole("row").filter({ hasText: "网关会话" });
  const timestamp = await older.locator("time").getAttribute("datetime");
  await expect(gateway.locator("time")).toHaveAttribute(
    "datetime",
    await page.evaluate(() =>
      new Date(window.activityMessageBase! - 30000).toISOString(),
    ),
  );
  await page.evaluate(() => {
    window.activityCase = "message-order-tools";
  });
  await expect(older).toContainText("Read、Grep");
  await expect.poll(() => titles(page)).toEqual(initial);
  await expect(older.locator("time")).toHaveAttribute("datetime", timestamp!);
  await page.evaluate(() => {
    window.activityCase = "message-order-completed";
  });
  await expect(older).toContainText("已结束 · 等待输入");
  await expect.poll(() => titles(page)).toEqual(initial);
  await expect(gateway.locator("time")).toHaveAttribute(
    "datetime",
    await page.evaluate(() =>
      new Date(window.activityMessageBase! - 30000).toISOString(),
    ),
  );
});

test("a new user message moves its session ahead, and subsequent tools keep that position", async ({
  page,
}) => {
  await load(page);
  await expect.poll(() => titles(page)).toEqual(initial);
  await page.evaluate(() => {
    window.activityCase = "message-order-new";
  });
  await expect(feed(page)).toContainText("开始下一条消息");
  await expect
    .poll(() => titles(page))
    .toEqual([
      "开始下一条消息",
      initial[0],
      initial[1],
      initial[3],
      initial[4],
    ]);
  await page.evaluate(() => {
    window.activityCase = "message-order-new-tools";
  });
  await expect(feed(page)).toContainText("已提出调用：Edit");
  await expect
    .poll(() => titles(page))
    .toEqual([
      "开始下一条消息",
      initial[0],
      initial[1],
      initial[3],
      initial[4],
    ]);
});

test("step updates keep pagination and the focused row even when missing message times receive fresh events", async ({
  page,
}) => {
  await load(page, "message-order-page");
  await expect(
    feed(page).getByRole("button", { name: "下一页", exact: true }),
  ).toBeEnabled();
  await feed(page).getByRole("button", { name: "下一页", exact: true }).click();
  await expect
    .poll(() => titles(page))
    .toEqual(["更早消息 80", "可读摘要，消息开始时间未知"]);
  const row = feed(page).getByRole("row").filter({ hasText: "消息时间未知" });
  await row.focus();
  await page.evaluate(() => {
    window.activityCase = "message-order-page-tools";
  });
  await expect(row).toContainText("Bash");
  await expect(row).toBeFocused();
  await expect
    .poll(() => titles(page))
    .toEqual(["更早消息 80", "可读摘要，消息开始时间未知"]);
  await expect(feed(page)).toContainText("第 2 / 2 页");
});
