import { test, expect } from "@playwright/test";
import type {} from "./requests-harness";

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
  await expect(
    page.getByRole("region", { name: "请求指标概览" }),
  ).toContainText("逐条响应，不代表已确认 HTTP 请求");
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
  await page.getByRole("tab", { name: "已关联请求", exact: true }).click();
  await expect(page.getByText("当前筛选 1 条", { exact: false })).toBeVisible();
  await expect(
    page.getByRole("region", { name: "来源与跨源统计" }),
  ).toContainText("未关联记录可能重叠，不相加为全部请求");
  await page.getByRole("tab", { name: "原生响应", exact: true }).click();
  await page.getByRole("button", { name: "导出 CSV" }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "已导出" }),
  ).toHaveText("已导出 45 条模型响应记录至 requests.csv");
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
