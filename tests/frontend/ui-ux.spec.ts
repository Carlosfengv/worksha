import { test, expect } from "@playwright/test";
import {
  compactNumber,
  chartColor,
  chartDash,
} from "../../src/shared/lib/format";
import { agentProducts } from "../../src/shared/types/agent";
import { summaryMoney } from "../../src/shared/lib/money";

test("summary formatting keeps unknown, actual zero and tiny nonzero money distinct", () => {
  expect(compactNumber(null)).toBe("未知");
  expect(compactNumber(0)).toBe("0");
  expect(compactNumber(1_400_000_000)).toBe("14亿");
  expect(summaryMoney(0, "USD")).toBe("0 USD");
  expect(summaryMoney(0.000000004, "USD")).toBe("< 0.0001 USD");
  expect(summaryMoney(1.722194, "USD")).toBe("1.72 USD");
  for (const agent of agentProducts)
    expect(chartColor(agent.id)).toMatch(/^var\(--chart-[1-5]\)$/);
  expect(chartColor("fixture-model")).toMatch(/^var\(--chart-[1-5]\)$/);
  expect(
    new Set(Array.from({ length: 6 }, (_, index) => chartDash(index))).size,
  ).toBe(6);
});

test("overview prioritizes live records and keyboard navigation while keeping history available", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 860 });
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  const table = page.getByRole("region", { name: "当前活动列表", exact: true });
  await expect(table.getByRole("combobox", { name: "每页条数" })).toHaveText(
    "5",
  );
  await expect(table.getByRole("row")).toHaveCount(6);
  await expect(table).toContainText("修复登录");
  const history = page.getByRole("button", {
    name: "Agent 连接与历史指标",
    exact: true,
  });
  await expect(history).toHaveAttribute("aria-expanded", "false");
  await expect(
    page.getByRole("article", { name: "Claude Code CLI实时活动", exact: true }),
  ).not.toBeVisible();
  const task = table.getByRole("row").filter({ hasText: "修复登录" });
  await task.focus();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/sessions\/claude%3Aa/);
});

test("native requests adapt default columns and switch to analysis without losing filters", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 860 });
  await page.goto(
    "/tests/frontend/requests.html#/requests?range=all&source=native&agent=codex-cli",
  );
  const table = page.getByRole("region", { name: "原生响应", exact: true });
  await expect(table.getByRole("row")).toHaveCount(41);
  await expect(table.getByRole("columnheader", { name: "TTFT" })).toHaveCount(
    0,
  );
  await expect(
    table.getByRole("columnheader", { name: "缓存读取" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "跨来源统计与采集状态", exact: true }),
  ).toHaveAttribute("aria-expanded", "false");
  await page.getByRole("tab", { name: "趋势分析", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "请求趋势分析", exact: true }),
  ).toBeVisible();
  await expect(page).toHaveURL(/source=native/);
  await expect(page).toHaveURL(/agent=codex-cli/);
});

test("long MCP Agent lists stay inside their cells and comparisons open scoped detail", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 860 });
  await page.goto(
    "/tests/frontend/tables.html?mode=projects-page&longAgents=1#/mcp",
  );
  const table = page.getByRole("region", { name: "MCP 聚合列表", exact: true });
  const cell = table.getByRole("row").nth(1).getByRole("cell").nth(1);
  await expect(cell).toContainText("Claude Code CLI");
  const overflow = await cell.evaluate((element) =>
    [...element.querySelectorAll("p")].some(
      (p) => p.scrollWidth > p.clientWidth + 1,
    ),
  );
  expect(overflow).toBe(false);
  await page.getByRole("tab", { name: "调用分析", exact: true }).click();
  await page
    .getByRole("region", { name: "MCP 调用排行", exact: true })
    .getByRole("button", { name: /^Fixture MCP/ })
    .click();
  await expect(
    page.getByRole("region", { name: "MCP 聚合详情", exact: true }),
  ).toBeVisible();
});

test("MCP directory opens details at the top and returns to the triggering service", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto(
    "/tests/frontend/tables.html?mode=projects-page&longAgents=1#/mcp?days=7",
  );
  const directory = page.getByRole("tab", { name: "服务目录", exact: true });
  await expect(directory).toHaveAttribute("aria-selected", "true");
  const trigger = page.getByRole("button", {
    name: "查看 Fixture MCP",
    exact: true,
  });
  await expect(trigger).toBeInViewport();
  await trigger.focus();
  await page.keyboard.press("Enter");
  await expect(
    page.getByRole("heading", { name: "Fixture MCP", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("Tab");
  const back = page.getByRole("button", { name: "返回 MCP 列表", exact: true });
  await expect(back).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/#\/mcp\?days=7$/);
  await expect(trigger).toBeFocused();
});

test("Agent and model lists share the MCP title and action placement at minimum width", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  const bounds = [];
  for (const { route, title, action } of [
    { route: "mcp", title: "MCP", action: "重新读取 MCP 配置" },
    { route: "providers", title: "模型供应商", action: "添加供应商" },
    { route: "model-services", title: "模型服务", action: "创建模型服务" },
    { route: "agents", title: "Agents", action: "重新读取配置" },
  ]) {
    await page.goto(`/tests/frontend/tables.html?mode=shell#/${route}`);
    const heading = page.getByRole("heading", {
      name: title,
      exact: true,
      level: 2,
    });
    const button = page.getByRole("button", { name: action, exact: true });
    await expect(heading).toBeInViewport();
    await expect(button).toBeInViewport();
    const titleBox = await heading.boundingBox();
    const actionBox = await button.boundingBox();
    expect(titleBox).not.toBeNull();
    expect(actionBox).not.toBeNull();
    expect(actionBox!.x).toBeGreaterThan(titleBox!.x);
    expect(Math.abs(actionBox!.y - titleBox!.y)).toBeLessThan(12);
    bounds.push({ x: titleBox!.x, y: titleBox!.y });
  }
  for (const position of bounds.slice(1)) expect(position).toEqual(bounds[0]);
});

test("primary workspaces keep horizontal scrolling within tables at the minimum desktop width", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  for (const url of [
    "/tests/frontend/tables.html?mode=activity#/overview",
    "/tests/frontend/requests.html#/requests?range=all",
    "/tests/frontend/tables.html?mode=projects-page&longAgents=1#/mcp",
  ]) {
    await page.goto(url);
    await expect(page.getByRole("table").first()).toBeVisible();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
  }
});
