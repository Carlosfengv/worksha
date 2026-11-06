import { expect, test } from "@playwright/test";
import type {} from "./task-projects-harness";

test("MCP 重新读取实际触发配置刷新，失败后可重试并出现配置", async ({
  page,
}) => {
  await page.goto("/tests/frontend/task-projects.html?mcp=1&fail-refresh=1");
  await expect(page.getByText("当前范围未发现 MCP 配置或调用")).toBeVisible();
  await page.getByRole("button", { name: "重新读取 MCP 配置" }).click();
  await expect(page.getByRole("alert")).toContainText("配置重新读取失败");
  await page.getByRole("button", { name: "重新读取 MCP 配置" }).click();
  await expect(page.getByText("pencil", { exact: true }).first()).toBeVisible();
  expect(await page.evaluate(() => window.taskProjectCalls)).toEqual([
    { kind: "read-mcp" },
    { kind: "read-mcp" },
    { kind: "read-mcp" },
  ]);
});

test("新建任务可分页选择已有临时项目，并将其交给原生目录确认", async ({
  page,
}) => {
  await page.goto("/tests/frontend/task-projects.html");
  await page.getByRole("button", { name: "加载更多项目" }).click();
  await page.getByRole("combobox", { name: "项目", exact: true }).click();
  await page.getByRole("option", { name: /第二页项目/ }).click();
  await page
    .getByRole("textbox", { name: "想让 Agent 做什么？" })
    .fill("项目测试");
  await page.getByRole("button", { name: "确认项目并发送" }).click();
  expect(
    await page.evaluate(() =>
      window.taskProjectCalls.filter(
        (c) => (c as { kind: string }).kind === "create",
      ),
    ),
  ).toEqual([{ kind: "create", projectId: `project:${"b".repeat(64)}` }]);
  await page.getByRole("button", { name: "改选其他文件夹" }).click();
  await expect(
    page.getByRole("button", { name: "选择文件夹并发送" }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "选择文件夹并发送" }).click();
  const ids = await page.evaluate(() => window.taskProjectIds);
  expect(ids).toHaveLength(2);
  expect(ids[0]).not.toBe(ids[1]);
});

test("项目入口直接打开新任务，已选项目不受首屏分页影响", async ({ page }) => {
  await page.goto("/tests/frontend/task-projects.html?selected=1");
  await expect(page.getByRole("heading", { name: "新建任务" })).toBeVisible();
  await expect(
    page.getByRole("combobox", { name: "项目", exact: true }),
  ).toHaveValue(/第二页项目/);
});

test("切换到 Claude Code 保留已有项目，取消原生确认不会创建任务", async ({
  page,
}) => {
  await page.goto("/tests/frontend/task-projects.html?selected=1");
  await page.getByRole("combobox", { name: "执行 Agent" }).click();
  await page
    .getByRole("option", { name: "Claude Code CLI", exact: true })
    .click();
  await expect(
    page.getByRole("combobox", { name: "项目", exact: true }),
  ).toHaveValue(/第二页项目/);
  await page
    .getByRole("textbox", { name: "想让 Agent 做什么？" })
    .fill("目录确认取消测试");
  await page.getByRole("button", { name: "确认项目并发送" }).click();
  expect(
    await page.evaluate(() =>
      window.taskProjectCalls.filter(
        (c) => (c as { kind: string }).kind === "create-cli",
      ),
    ),
  ).toEqual([
    {
      kind: "create-cli",
      projectId: `project:${"b".repeat(64)}`,
      agent: "claude",
    },
  ]);
  await expect(
    page.getByRole("button", { name: "确认项目并发送" }),
  ).toBeEnabled();
});

test("项目搜索忽略过期结果，读取失败明确提示且可恢复", async ({ page }) => {
  await page.goto("/tests/frontend/task-projects.html");
  const input = page.getByRole("combobox", { name: "项目", exact: true });
  await input.fill("slow");
  await input.fill("fast");
  await expect(page.getByRole("option", { name: /第二页项目/ })).toBeVisible();
  await page.waitForTimeout(400);
  await expect(page.getByRole("option", { name: /第二页项目/ })).toBeVisible();
  await input.fill("broken");
  await expect(page.getByRole("alert")).toContainText("项目来源读取失败");
  await input.fill("fast");
  await expect(page.getByRole("option", { name: /第二页项目/ })).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
});
