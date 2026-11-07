import { expect, test, type Page } from "@playwright/test";
import type {} from "./plugin-diagnostics-harness";

async function open(page: Page, mode: string) {
  await page.goto(`/tests/frontend/plugin-diagnostics.html?mode=${mode}`);
  await page.getByRole("row").filter({ hasText: "Aurora 只读会话" }).click();
  return page.getByRole("region", { name: "插件能力与运行诊断" });
}
test("能力、授权、初始化与数据成功分开，诊断计数保持大整数精度", async ({
  page,
}) => {
  const panel = await open(page, "ready");
  await expect(
    page.getByRole("tablist", { name: "插件管理", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByLabel("插件中心列表", { exact: true }).getByRole("table"),
  ).toContainText("用户上传");
  await expect(panel).toContainText("初始化通过");
  await expect(panel).toContainText("最多 8 MiB、10240 条记录");
  await expect(panel).toContainText("历史来源：已授权");
  await expect(panel).toContainText("上次完整快照可用");
  await expect(panel).toContainText("9,007,199,254,740,993 次");
  await expect(panel).toContainText("超时 1；资源限制 1；进程异常 1");
  await expect(panel).toContainText("重启后重新计数");
  expect(await page.evaluate(() => window.diagnosticMutations)).toEqual([]);
});
test("未知、确认无数据、过期与撤权不能互相冒充", async ({ page }) => {
  const unknown = await open(page, "unknown");
  await expect(unknown).toContainText("尚无本次启动的完整读取结果");
  await expect(unknown).not.toContainText("确认无记录");
  await expect(unknown).not.toContainText("成功 0");
  const empty = await open(page, "empty");
  await expect(empty).toContainText("上次完整读取确认无记录");
  const stale = await open(page, "stale");
  await expect(stale).toContainText("保留上次完整结果");
  await expect(stale).toContainText("513");
  const revoked = await open(page, "revoked");
  await expect(revoked).toContainText("等待资源授权");
  await expect(revoked).not.toContainText("完整快照记录数");
});
test("旧服务和无效诊断不会隐藏插件及原管理入口", async ({ page }) => {
  for (const mode of ["old", "broken"]) {
    const panel = await open(page, mode);
    await expect(panel).toContainText("运行诊断暂不可用");
    await expect(
      page.getByRole("button", { name: "停用", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "选择并授权 JSONL" }),
    ).toBeVisible();
    expect(await page.evaluate(() => window.diagnosticMutations)).toEqual([]);
  }
});
test("诊断缺口可见，窄屏保持阅读与键盘行激活", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/tests/frontend/plugin-diagnostics.html?mode=gap");
  const row = page.getByRole("row").filter({ hasText: "Aurora 只读会话" });
  await row.focus();
  await page.keyboard.press("Enter");
  await expect(
    page.getByRole("region", { name: "插件能力与运行诊断" }),
  ).toContainText("部分操作诊断未记录（2 次）");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});
