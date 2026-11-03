import { expect, test } from "@playwright/test";
import type {} from "./activity-fixture";

test("native backfills replace the session headline and each next message keeps its own restored title", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  const feed = page.getByRole("region", { name: "当前活动列表", exact: true });
  const row = feed.getByRole("row").filter({ has: page.getByRole("cell") });
  const title = row.getByRole("cell").first().locator("p").first();
  await page.evaluate(() => {
    window.activityCase = "native-desktop-input-missing";
  });
  await expect(row).toHaveCount(1);
  await expect(row).toContainText("最近输入：来源未提供");
  await row.focus();
  for (const [mode, text] of [
    ["native-desktop-input-recovered", "第一条原生消息内容"],
    ["native-desktop-input-next", "第二条原生消息内容"],
    ["native-desktop-input-restarted", "第二条原生消息内容"],
  ] as const) {
    await page.evaluate((mode) => {
      window.activityCase = mode;
    }, mode);
    await expect(row).toHaveCount(1);
    await expect(title).toHaveText(text);
    await expect(row).toContainText("会话：固定的会话标题");
    await expect(row).not.toContainText("来源未提供");
    await expect(row).not.toContainText("更早一条消息内容");
    await expect(row).toContainText("TTFT 1.00 秒");
    await expect(row).toContainText("均速 32.0 Token/s");
    await expect(row).toBeFocused();
  }
});
