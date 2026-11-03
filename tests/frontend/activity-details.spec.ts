import { expect, test, type Page } from "@playwright/test";
import type {} from "./activity-fixture";

const feed = (page: Page) =>
  page.getByRole("region", { name: "当前活动列表", exact: true });
async function load(page: Page, value: Window["activityCase"]) {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await page.evaluate((value) => {
    window.activityCase = value;
  }, value);
}

test("activity shows the latest input, observed thinking and named native or gateway tools", async ({
  page,
}) => {
  await load(page, "activity-details");
  const thinking = feed(page)
    .getByRole("row")
    .filter({ hasText: "登录修复会话" });
  await expect(
    thinking.getByRole("cell").first().locator("p").first(),
  ).toHaveText("请检查最新登录失败，再补充回归测试。");
  await expect(thinking.getByRole("cell").nth(1)).toContainText("思考中");
  const tools = feed(page).getByRole("row").filter({ hasText: "检查工具链" });
  await expect(tools).toContainText("已提出调用：Read、Grep");
  await expect(tools).toContainText("先读取项目说明");
  const gateway = feed(page)
    .getByRole("row")
    .filter({ hasText: "核对请求参数，不要修改配置。" });
  await expect(gateway).toContainText("核对请求参数");
  await expect(gateway.getByRole("cell").nth(1)).toContainText("生成工具参数");
  await expect(gateway.getByRole("cell").nth(1)).toContainText(
    "mcp__files__read",
  );
  await expect(gateway).toContainText("TTFT 0.700 秒");
});

test("a new input and tool results replace the visible step on the same activity", async ({
  page,
}) => {
  await load(page, "activity-details");
  const row = feed(page).getByRole("row").filter({ hasText: "登录修复会话" });
  await expect(row).toContainText("思考中");
  await page.evaluate(() => {
    window.activityCase = "activity-details-updated";
  });
  await expect(row.getByRole("cell").first().locator("p").first()).toHaveText(
    "现在修改登录按钮颜色。",
  );
  await expect(row).toContainText("已提出调用：Edit、Bash");
  await expect(row).not.toContainText("请检查最新登录失败");
  const result = feed(page).getByRole("row").filter({ hasText: "检查工具链" });
  await expect(result).toContainText("工具结果已返回");
  await expect(result).toContainText("Read 已返回；仍待返回：Grep");
  await expect(row).toHaveCount(1);
});

test("completed or stale activities retain input but do not claim current thinking or execution", async ({
  page,
}) => {
  await load(page, "activity-details");
  const row = feed(page).getByRole("row").filter({ hasText: "登录修复会话" });
  await expect(row).toContainText("思考中");
  await page.evaluate(() => {
    window.activityCase = "activity-details-stale";
  });
  await expect(row).toContainText("状态待更新");
  await expect(row).toContainText("最近步骤：");
  await expect(row).not.toContainText("思考中");
  await page.evaluate(() => {
    window.activityCase = "activity-details-completed";
  });
  await expect(row).toContainText("已结束 · 等待输入");
  await expect(row.getByRole("cell").first().locator("p").first()).toHaveText(
    "请检查最新登录失败，再补充回归测试。",
  );
});

test("the latest Session input replaces older requests and markup still renders as text", async ({
  page,
}) => {
  await load(page, "activity-details-scope");
  const current = feed(page)
    .getByRole("row")
    .filter({ hasText: "收到新的用户消息" });
  await expect(current.getByRole("cell").first()).toContainText(
    "后续轮次的输入不得套给旧请求",
  );
  await expect(current).toHaveCount(1);
  await expect(feed(page)).not.toContainText("旧请求自身的输入摘要");
  await expect(feed(page).getByRole("row")).toHaveCount(3);
  await expect(feed(page)).toContainText("<img src=x onerror=alert(1)>");
  await expect(feed(page).locator("img[src='x']")).toHaveCount(0);
});

test("long input and tool labels stay within the table at the minimum native window width", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 860 });
  await load(page, "activity-details-long");
  await expect(feed(page)).toContainText("检查包含中文和🙂的长输入");
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth > window.innerWidth + 1,
  );
  expect(overflow).toBe(false);
  const input = feed(page)
    .getByRole("row")
    .filter({ hasText: "登录修复会话" })
    .getByRole("cell")
    .first();
  expect(
    await input.evaluate((e) => e.getBoundingClientRect().height),
  ).toBeLessThan(240);
  const title = input.locator("p").first();
  const text = await title.textContent();
  expect(text).toMatch(/^检查包含中文和🙂的长输入/);
  await expect(title).toHaveAttribute("title", text!);
  expect(await title.evaluate((e) => e.scrollHeight > e.clientHeight)).toBe(
    true,
  );
});

test("disabled native sources cannot expose excerpts from an already populated overview", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&sourceState=disabled#/overview",
  );
  await page.evaluate(() => {
    window.activityCase = "activity-details";
  });
  await expect(feed(page)).toContainText("核对请求参数，不要修改配置。");
  await expect(feed(page)).not.toContainText("请检查最新登录失败");
  await expect(feed(page)).not.toContainText("先读取项目说明");
  await expect(feed(page)).not.toContainText("登录修复会话");
});
