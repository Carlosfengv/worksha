import { expect, test } from "@playwright/test";

test("overview separates actual zero, sub-micro estimates and unknown fees while retaining filters", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  const analysis = page.getByRole("region", {
    name: "调用趋势与占比",
    exact: true,
  });
  await page.getByRole("tab", { name: "费用", exact: true }).click();
  await expect(analysis.getByLabel("费用构成")).toContainText(
    "实际 — · 估算 — · 2 次费用未知",
  );
  await expect(analysis).toContainText("缺少可核实的价格或用量。");
  await page.getByLabel("调用统计来源").click();
  await page.getByRole("option", { name: "网关请求", exact: true }).click();
  await expect(analysis.getByLabel("费用构成")).toContainText(
    "实际 $0 · 估算 $0.000000004 · 1 次费用未知",
  );
  await expect(analysis).toContainText("费用未知 / 部分");
  await expect(
    analysis.getByRole("heading", { name: "Agent占比 · 费用" }),
  ).toBeVisible();
  await page.getByLabel("时间范围").click();
  await page.getByRole("option", { name: "最近 6 小时", exact: true }).click();
  await page.getByLabel("首页筛选 Agent").click();
  await page.getByRole("option", { name: "OpenCode", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.overviewReads.at(-1)))
    .toMatchObject({
      agent: "opencode",
      range: 6,
      metric: "cost",
      source: "gateway",
    });
  await expect(analysis.getByLabel("费用构成")).toContainText("$0.000000004");
  await page.getByRole("tab", { name: "Token 用量", exact: true }).click();
  await expect(analysis.getByLabel("费用构成")).toHaveCount(0);
});
