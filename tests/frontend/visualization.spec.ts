import { expect, test } from "@playwright/test";
import {
  heatmapDateKey,
  requestAxisLabel,
  requestHeatmapColumns,
  requestHeatmapValue,
} from "../../src/features/observation/request-visualization";
import type { HeatmapDay } from "../../src/features/observation/request-types";

const day = (
  date: string,
  tokens: number | null,
  state = "recorded",
): HeatmapDay => ({
  date,
  start: `${date}T00:00:00+08:00`,
  end: `${date}T23:59:59+08:00`,
  state,
  records: 1,
  tokens,
  cost: { actual: [], estimated: [], unknownCalls: 1 },
  usageMissing: tokens === null ? 1 : 0,
});

test("calendar adaptation preserves civil dates, known zero, missing measures and unrecorded gaps", () => {
  const days = [
    day("2026-10-20", null),
    day("2026-10-18", 0),
    day("2026-10-21", 100, "future"),
  ];
  const before = JSON.stringify(days);
  const columns = requestHeatmapColumns(days, "tokens", "USD");
  const values = new Map(
    columns.flatMap((column) =>
      column.bins.map((bin) => [heatmapDateKey(bin.date), bin.count] as const),
    ),
  );
  expect(columns[0].bins[0].date.getDay()).toBe(0);
  expect(values.get("2026-10-18")).toBe(0);
  for (const key of ["2026-10-19", "2026-10-20", "2026-10-21"])
    expect(Number.isNaN(values.get(key))).toBe(true);
  expect(JSON.stringify(days)).toBe(before);
});

test("heatmap money stays in its requested currency and zero is distinct from missing", () => {
  const item = day("2026-10-18", 0);
  item.cost.actual = [
    { currency: "USD", amount: 0, calls: 1 },
    { currency: "CNY", amount: 9, calls: 1 },
  ];
  item.cost.estimated = [{ currency: "USD", amount: 0.000000004, calls: 1 }];
  expect(requestHeatmapValue(item, "cost", "USD")).toBe(0.000000004);
  expect(requestHeatmapValue(item, "cost", "CNY")).toBe(9);
  expect(requestHeatmapValue(item, "cost", "EUR")).toBeNull();
  expect(requestHeatmapValue(item, "tokens", "USD")).toBe(0);
  expect(requestAxisLabel(0.000000004, "cost")).toBe("0.000000004");
  expect(requestAxisLabel(0, "cost")).toBe("0");
  expect(requestAxisLabel(50, "ttft")).toBe("0.05s");
  expect(requestAxisLabel(100, "ttft")).toBe("0.1s");
});

test("overview rankings retain all groups and drill into the displayed source and time range", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 860 });
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&visuals=1#/overview",
  );
  const analysis = page.getByRole("region", {
    name: "调用趋势与占比",
    exact: true,
  });
  await expect(analysis.getByText("156", { exact: true })).toBeVisible();
  const agents = page.getByRole("region", {
    name: "Agent占比 · 调用次数",
    exact: true,
  });
  await expect(
    agents.getByRole("button", { name: /^unmeasured/ }),
  ).toContainText("—");
  await expect(
    page
      .getByRole("region", { name: "模型占比 · 调用次数", exact: true })
      .getByRole("button", { name: /^fixture-model-9/ }),
  ).toContainText("0");
  await agents.scrollIntoViewIfNeeded();
  await agents.getByRole("button", { name: /^Claude Code CLI/ }).click();
  await expect(page).toHaveURL(/#\/requests\?/);
  await expect(page).toHaveURL(/source=native/);
  await expect(page).toHaveURL(/agent=claude-code-cli/);
  await expect(page).toHaveURL(/range=custom/);
  await expect(page).toHaveURL(/start=/);
  await expect(page).toHaveURL(/end=/);
  await expect(
    page
      .getByRole("banner")
      .getByRole("heading", { name: "请求", exact: true }),
  ).toBeVisible();
});

test("performance analysis separates P50 and P95 and group drilldown retains the selected metric", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 860 });
  await page.goto(
    "/tests/frontend/requests.html?visuals=1#/requests?range=all",
  );
  await page.getByRole("tab", { name: "趋势分析", exact: true }).click();
  await page.getByLabel("趋势指标", { exact: true }).click();
  await page
    .getByRole("option", { name: "首次输出 TTFT", exact: true })
    .click();
  const analysis = page.getByRole("region", {
    name: "请求趋势分析",
    exact: true,
  });
  await expect(
    analysis.getByText("首次输出 TTFT P50", { exact: true }),
  ).toBeVisible();
  await expect(
    analysis.getByText("首次输出 TTFT P95", { exact: true }),
  ).toBeVisible();
  await expect(analysis.getByText("P95 · 虚线", { exact: true })).toBeVisible();
  await analysis.scrollIntoViewIfNeeded();
  await page
    .getByRole("region", { name: "Agent分组", exact: true })
    .getByRole("button", { name: /^Codex CLI/ })
    .click();
  await expect(page).toHaveURL(/agent=codex-cli/);
  await expect(page).toHaveURL(/metric=ttft/);
});

test("MCP result and latency comparisons retain unknown groups and scoped service navigation", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 860 });
  await page.goto(
    "/tests/frontend/tables.html?mode=projects-page&visuals=1#/mcp?agent=codex-cli&days=7",
  );
  const comparison = page.getByRole("region", {
    name: "MCP 调用比较",
    exact: true,
  });
  await expect(comparison.getByText("491", { exact: true })).toBeVisible();
  await expect(comparison).toContainText(
    "已返回表示工具返回了记录，不等同于任务成功",
  );
  const ranking = page.getByRole("region", {
    name: "MCP 调用排行",
    exact: true,
  });
  await expect(ranking.getByRole("button", { name: /^MCP 8/ })).toContainText(
    "尚无可确认的调用记录",
  );
  await expect(ranking.getByRole("button", { name: /^MCP 8/ })).toContainText(
    "未知",
  );
  const latency = page.getByRole("region", {
    name: "MCP 耗时比较 · P95",
    exact: true,
  });
  await expect(latency.getByRole("button", { name: /^MCP 8/ })).toContainText(
    "未知",
  );
  await comparison.scrollIntoViewIfNeeded();
  await ranking.getByRole("button", { name: /^MCP 1/ }).click();
  await expect(page).toHaveURL(/#\/mcp\/visual-mcp-0/);
  await expect(page).toHaveURL(/agent=codex-cli/);
  await expect(page).toHaveURL(/days=7/);
  await expect(
    page.getByRole("region", { name: "MCP 聚合详情", exact: true }),
  ).toContainText("MCP 1");
});

test("visual analysis fits compact light and dark contexts without page-wide overflow", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  for (const [name, url] of [
    [
      "overview",
      "/tests/frontend/tables.html?mode=activity&visuals=1#/overview",
    ],
    ["requests", "/tests/frontend/requests.html?visuals=1#/requests?range=all"],
    ["mcp", "/tests/frontend/tables.html?mode=projects-page&visuals=1#/mcp"],
  ]) {
    await page.goto(url);
    if (name === "requests")
      await page.getByRole("tab", { name: "趋势分析", exact: true }).click();
    await expect(
      page.getByRole("region", {
        name:
          name === "overview"
            ? "调用趋势与占比"
            : name === "requests"
              ? "请求趋势分析"
              : "MCP 调用比较",
        exact: true,
      }),
    ).toBeAttached();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    await page.evaluate(() => document.documentElement.classList.add("dark"));
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
  }
});
