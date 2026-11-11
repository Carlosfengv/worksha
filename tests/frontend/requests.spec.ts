import { test, expect } from "@playwright/test";
import type {} from "./requests-harness";

test("request detail shows candidate decisions when no upstream send occurred", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto(
    "/tests/frontend/requests.html?route-decisions=1#/requests/request-001",
  );
  const decisions = page.getByRole("region", { name: "候选路由决策" });
  await expect(decisions).toContainText(
    "本轮未向候选发送请求 · 网关返回 HTTP 403",
  );
  await expect(decisions.getByRole("listitem")).toHaveCount(2);
  await expect(decisions.getByRole("listitem").first()).toContainText(
    "主服务 / 子服务入口",
  );
  await expect(decisions.getByRole("listitem").first()).toContainText(
    "子服务 / 模型 A",
  );
  await expect(decisions).toContainText("条目 child-mo");
  await expect(decisions).toContainText(
    "账号池第 2 位 · 上游拒绝此账号访问模型，冷却中",
  );
  await expect(decisions).toContainText("跨模型回退未开启");
  await expect(page.getByRole("region", { name: "重试瀑布图" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /^尝试 \d/ })).toHaveCount(0);
});

test("request detail distinguishes an unconfirmed send from a skipped candidate", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/requests.html?route-decisions=unknown#/requests/request-001",
  );
  const decisions = page.getByRole("region", { name: "候选路由决策" });
  await expect(decisions).toContainText("本轮发送 1 次 · 网关返回 HTTP 504");
  await expect(decisions.getByRole("listitem").first()).toContainText(
    "已尝试发送，上游结果未确认",
  );
  await expect(decisions.getByRole("listitem").last()).toContainText(
    "已有请求结果待确认，未尝试",
  );
});

test("request overview, full count, frozen pages, detail and return retain scope", async ({
  page,
}) => {
  await page.goto("/tests/frontend/requests.html#/requests?range=all");
  await expect(
    page.getByRole("region", { name: "请求", exact: true }).getByRole("row"),
  ).toHaveCount(41);
  await expect(
    page.getByText("当前筛选 85 条", { exact: false }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "请求列表" }),
  ).toBeInViewport();
  await expect(page.getByRole("region", { name: "请求指标概览" })).toHaveCount(
    0,
  );
  await page.getByRole("button", { name: "下一页请求" }).click();
  await expect(page).toHaveURL(/snapshot=fixture-snapshot/);
  await expect(
    page.getByText("当前为历史分页", { exact: false }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "查看详情", exact: true })
    .first()
    .click();
  await expect(page.getByRole("region", { name: "请求详情" })).toContainText(
    "request-040",
  );
  await page.getByRole("button", { name: /^尝试 1 · 测试 Provider/ }).click();
  await expect(page.getByText("上游报告模型", { exact: true })).toBeVisible();
  await expect(
    page.getByText("fixture-immutable-route-version", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("轮次准入依据", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "返回请求列表" }).click();
  await expect(page).toHaveURL(/cursor=fixture-snapshot/);
  await expect(
    page.getByText("当前为历史分页", { exact: false }),
  ).toBeVisible();
});

test("gateway retry waterfall uses measured spans and links only the exact attempt", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto(
    "/tests/frontend/requests.html?trace=1#/requests/request-001?range=all",
  );
  const trace = page.getByRole("region", { name: "重试瀑布图", exact: true });
  await expect(trace.getByRole("heading")).toBeInViewport();
  await expect(trace.getByRole("img")).toHaveCount(2);
  await expect(
    trace.getByRole("img", { name: "从 0 秒 到 0.2 秒" }),
  ).toBeVisible();
  await expect(
    trace.getByRole("img", { name: "从 1.2 秒 到 2 秒" }),
  ).toBeVisible();
  const attempts = trace.getByRole("listitem");
  await expect(attempts.nth(1)).toContainText("耗时 未知");
  await expect(attempts.nth(1)).toContainText("与上次间隔 0.35 秒");
  await expect(attempts.nth(3)).toContainText("相对起点未知");
  await expect(attempts.nth(3)).toContainText("耗时 0.2 秒");
  const jump = trace.getByRole("button", { name: "定位会话调用" });
  await expect(jump).toHaveCount(1);
  await expect(
    attempts.nth(2).getByRole("button", { name: "定位会话调用" }),
  ).toBeVisible();
  await jump.click();
  const url = new URL(page.url());
  expect(url.hash.split("?")[0]).toBe("#/sessions/session-native");
  const params = new URLSearchParams(url.hash.split("?")[1]);
  expect(params.get("anchor")).toBe("native-0");
  expect(params.get("view")).toBe("timeline");
  expect(params.get("returnTo")).toBe("/requests/request-001?range=all");
});

test("keyboard detail navigation restores the triggering record in a short window", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/requests.html#/requests?range=all");
  const table = page.getByRole("region", { name: "请求", exact: true });
  await expect(table.getByRole("row")).toHaveCount(41);
  const trigger = table
    .getByRole("button", { name: "查看详情", exact: true })
    .last();
  await trigger.focus();
  await expect(trigger).toBeInViewport();
  await trigger.press("Enter");

  const title = page.getByRole("heading", { name: "请求详情", exact: true });
  await expect(title).toBeFocused();
  await expect(title).toBeInViewport();
  await page.keyboard.press("Tab");
  const back = page.getByRole("button", { name: "返回请求列表", exact: true });
  await expect(back).toBeFocused();
  await expect(back).toBeInViewport();
  await page.keyboard.press("Enter");

  await expect(page).toHaveURL(/snapshot=fixture-snapshot/);
  await expect(trigger).toBeFocused();
  await expect(trigger).toBeInViewport();
});

test("filters, model dimension, slow response and time drilldown preserve explicit scope", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/requests.html#/requests?range=all&session=known-session",
  );
  await expect(page.getByLabel("请求筛选")).toContainText("known-session");
  await page.getByRole("button", { name: "失败 1", exact: true }).click();
  await expect(page).toHaveURL(/result=failed/);
  await page
    .getByRole("button", { name: "查看全部请求（移除关联范围）" })
    .click();
  await expect(page).not.toHaveURL(/session=/);
  await expect(page).toHaveURL(/result=failed/);
  await page.getByLabel("请求结果", { exact: true }).click();
  await page.getByRole("option", { name: "全部结果", exact: true }).click();
  await page.getByLabel("搜索请求", { exact: true }).fill("slow");
  await page.getByRole("button", { name: "搜索", exact: true }).click();
  await page.getByLabel("搜索请求", { exact: true }).fill("request-001");
  await page.getByRole("button", { name: "搜索", exact: true }).click();
  await expect(page.getByText("当前筛选 1 条", { exact: false })).toBeVisible();
  await page.waitForTimeout(400);
  await expect(page.getByText("当前筛选 1 条", { exact: false })).toBeVisible();
  await page.getByRole("tab", { name: "趋势分析", exact: true }).click();
  await page.getByLabel("拆分维度", { exact: true }).click();
  await page.getByRole("option", { name: "按模型", exact: true }).click();
  await expect(page).toHaveURL(/dimension=model/);
  await page.getByRole("button", { name: "时段明细与下钻" }).click();
  await page.getByRole("button", { name: "查看此时段" }).first().click();
  await expect(page).toHaveURL(/range=custom/);
  await expect(page).toHaveURL(/start=/);
});

test("native export result is shown only for saved files and cancellation stays neutral", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/requests.html?export=cancel#/requests?range=all",
  );
  await page.getByRole("button", { name: "导出 CSV" }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "已取消导出" }),
  ).toHaveText("已取消导出");
  await page.goto(
    "/tests/frontend/requests.html?export=fail#/requests?range=all",
  );
  await page.getByRole("button", { name: "导出 CSV" }).click();
  await expect(page.getByRole("alert")).toHaveText("磁盘空间不足");
  await page.goto("/tests/frontend/requests.html#/requests?range=all");
  await page.getByRole("button", { name: "导出 CSV" }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "已导出" }),
  ).toHaveText("已导出 85 条请求至 requests.csv");
});

test("export pauses polling during destination selection and resumes after completion", async ({
  page,
}) => {
  await page.clock.install();
  await page.goto(
    "/tests/frontend/requests.html?export=wait#/requests?range=all",
  );
  await page.getByRole("tab", { name: "趋势分析", exact: true }).click();
  const before = await page.evaluate(() => window.requestQueries.length);
  await page.clock.fastForward(5000);
  await expect
    .poll(() => page.evaluate(() => window.requestQueries.length))
    .toBeGreaterThan(before);
  await page.clock.runFor(20);
  await page.getByRole("button", { name: "导出 CSV", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.requestExports.length))
    .toBe(1);
  const during = await page.evaluate(() => window.requestQueries.length);
  await page.clock.fastForward(60000);
  expect(await page.evaluate(() => window.requestQueries.length)).toBe(during);
  await page.evaluate(() => window.resolveRequestExport?.());
  await expect(
    page.getByRole("status").filter({ hasText: "已导出" }),
  ).toContainText("85 条请求");
  await page.clock.fastForward(5000);
  await expect
    .poll(() => page.evaluate(() => window.requestQueries.length))
    .toBeGreaterThan(during);
});

test("narrow screen and keyboard column controls avoid document overflow", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 820 });
  await page.goto("/tests/frontend/requests.html#/requests?range=all");
  await expect(
    page.getByRole("region", { name: "请求", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "选择显示列" }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});

test("native source keeps usable filters, labels response units and opens original evidence", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/requests.html#/requests?range=all&agent=codex-cli&provider=fixture",
  );
  await page.getByRole("tab", { name: "原生响应", exact: true }).click();
  await expect(page).toHaveURL(/source=native/);
  await expect(page).toHaveURL(/agent=codex-cli/);
  await expect(page).not.toHaveURL(/provider=/);
  await expect(
    page.getByText("当前筛选 45 条", { exact: false }),
  ).toBeVisible();
  await page.getByRole("tab", { name: "趋势分析", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "请求指标概览" }),
  ).toContainText("逐条响应，不代表已确认 HTTP 请求");
  await page.getByRole("tab", { name: "响应记录", exact: true }).click();
  await expect(
    page
      .getByRole("region", { name: "原生响应", exact: true })
      .getByRole("row"),
  ).toHaveCount(41);
  await page
    .getByRole("button", { name: "查看详情", exact: true })
    .first()
    .click();
  await expect(page.getByRole("region", { name: "原生证据" })).toContainText(
    "明确响应 ID 一对一关联",
  );
  await expect(page.getByRole("region", { name: "重试瀑布图" })).toHaveCount(0);
  await expect(page.getByRole("region", { name: "原生证据" })).toContainText(
    "用量字段冲突",
  );
  await page.getByRole("button", { name: "查看关联的网关请求" }).click();
  await expect(
    page.getByRole("region", { name: "跨源关联证据" }),
  ).toContainText("消费只采用网关记录");
  await page.getByRole("button", { name: "返回请求列表" }).click();
  await expect(page).toHaveURL(/source=native/);
});

test("linked view and export keep explicit source without presenting an additive total", async ({
  page,
}) => {
  await page.goto("/tests/frontend/requests.html#/requests?range=all");
  await page
    .getByRole("button", { name: "跨来源统计与采集状态", exact: true })
    .click();
  const sourcePanel = await page
    .getByRole("region", { name: "跨来源统计与采集状态", exact: true })
    .boundingBox();
  const sourceSection = await page
    .getByRole("region", { name: "来源与跨源统计", exact: true })
    .boundingBox();
  expect(sourcePanel!.width).toBeGreaterThan(sourceSection!.width * 0.9);
  await page.getByRole("tab", { name: "跨来源精确匹配", exact: true }).click();
  await expect(page.getByText("当前筛选 1 条", { exact: false })).toBeVisible();
  await expect(
    page.getByRole("region", { name: "来源与跨源统计" }),
  ).toContainText("请求能够打开所属会话时，仍可能缺少用于跨来源匹配的响应 ID");
  await page.getByRole("tab", { name: "原生响应", exact: true }).click();
  await page.getByRole("button", { name: "导出 CSV" }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "已导出" }),
  ).toHaveText("已导出 45 条模型响应记录至 requests.csv");
});

test("no exact match keeps both underlying request sources accessible", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/requests.html?noExactMatch=1#/requests?range=all&source=linked",
  );
  await expect(
    page.getByText("当前范围没有按响应 ID 精确匹配的记录。", { exact: false }),
  ).toBeVisible();
  await page.getByRole("button", { name: "查看网关请求" }).click();
  await expect(page.getByRole("tab", { name: "网关请求" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await page.getByRole("tab", { name: "跨来源精确匹配" }).click();
  await page.getByRole("button", { name: "查看原生响应" }).click();
  await expect(page.getByRole("tab", { name: "原生响应" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

test("heatmap is collapsed by default, supports keyboard day drilldown and preserves source", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/requests.html#/requests?range=all&source=native",
  );
  await expect(
    page.getByLabel("热力图指标", { exact: true }),
  ).not.toBeVisible();
  await page.getByRole("tab", { name: "趋势分析", exact: true }).click();
  await page.getByRole("button", { name: "用量热力图 · 最近 53 周" }).click();
  await page.getByLabel("热力图指标", { exact: true }).click();
  await page
    .getByRole("option", { name: "模型响应记录量", exact: true })
    .click();
  const calendar = page.getByRole("group", {
    name: /Heatmap\. Use arrow keys/,
  });
  await calendar.focus();
  await page.keyboard.press("End");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/range=custom/);
  await expect(page).toHaveURL(/source=native/);
  await expect(page).toHaveURL(/start=2026-10-19T00/);
  await expect(
    page.getByText("当前筛选 24 条", { exact: false }),
  ).toBeVisible();
});

test("source authorization state and heatmap fit narrow layouts", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 820 });
  await page.goto(
    "/tests/frontend/requests.html?noConsent=1#/requests?range=all&source=native",
  );
  await expect(
    page.getByText(
      "原生来源尚未授权。可在 Agents 的连接与观测中选择允许读取的来源。",
    ),
  ).toBeVisible();
  await page.getByRole("tab", { name: "趋势分析", exact: true }).click();
  await page.getByRole("button", { name: "用量热力图 · 最近 53 周" }).click();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await expect(
    page.getByRole("region", { name: "用量热力图", exact: true }),
  ).toContainText("缺口不补零");
});
