import { expect, test } from "@playwright/test";

for (const [mode, label] of [
  ["unknown", "未知，尚未读取工具目录"],
  ["empty", "无"],
  ["known", "lookup"],
]) {
  test(`MCP catalog ${mode} preserves tool observation and is not connection proof`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 720, height: 560 });
    await page.goto(
      `/tests/frontend/domain-readonly.html?unified=1&mcpCatalog=${mode}`,
    );
    await page
      .getByRole("button", { name: "查看项目与响应事实", exact: true })
      .click();
    await page.getByLabel("事实种类", { exact: true }).click();
    await page.getByRole("option", { name: "MCP 目录", exact: true }).click();
    await page
      .getByRole("row")
      .filter({ hasText: "MCP configuration" })
      .click();
    await expect(
      page.getByText(`目录工具：${label}；配置或目录不代表已连接或发生调用。`),
    ).toBeVisible();
    expect(
      (await page.evaluate(() => window.domainFixture.calls)).filter((c) =>
        c.startsWith("body:"),
      ),
    ).toEqual([]);
  });
}

test("fact graph navigation, summary units and explicit body at minimum width", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/domain-readonly.html?unified=1");
  await page
    .getByRole("button", { name: "查看项目与响应事实", exact: true })
    .click();
  const project = page
    .getByRole("row")
    .filter({ hasText: "Reported workspace" });
  await project.focus();
  await project.press("Enter");
  await expect(page.getByText(/来源报告路径：/)).toContainText(
    "/reported-only/project",
  );
  await page.getByRole("button", { name: "查看项目会话", exact: true }).click();
  await page.getByRole("row").filter({ hasText: "Alpha" }).click();
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter((c) =>
      c.startsWith("body:"),
    ),
  ).toEqual([]);
  await page.getByRole("button", { name: "查看会话响应", exact: true }).click();
  await page.getByRole("row").filter({ hasText: "Fixture-000" }).click();
  await expect(page.getByText(/网关关联未核实/)).toContainText(
    "source-claim-only",
  );
  await page.getByRole("button", { name: "定位所属会话", exact: true }).click();
  await page.getByRole("row").filter({ hasText: "Alpha" }).click();
  await page
    .getByRole("button", { name: "按当前授权读取此会话正文", exact: true })
    .click();
  await expect(
    page.getByText("Alpha private body", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByLabel("完整事实范围摘要", { exact: true }),
  ).toContainText("逐响应 12 Token");
  const detail = page.getByLabel("来源事实详情", { exact: true });
  await detail.focus();
  await detail.press("End");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.evaluate(() => window.domainFixture.revoke());
  await expect(
    page.getByText("Alpha private body", { exact: true }),
  ).toHaveCount(0);
});

test("fact hiding can be restored and pagination preserves full totals", async ({
  page,
}) => {
  await page.goto("/tests/frontend/domain-readonly.html?unified=1&factsMany=1");
  await page
    .getByRole("button", { name: "查看项目与响应事实", exact: true })
    .click();
  await page.getByRole("row").filter({ hasText: "Reported workspace" }).click();
  await page
    .getByRole("button", { name: "隐藏此来源报告项目", exact: true })
    .click();
  await expect(
    page.getByRole("button", {
      name: "恢复显示 Reported workspace",
      exact: true,
    }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "恢复显示 Reported workspace", exact: true })
    .click();
  await page.getByLabel("事实种类", { exact: true }).click();
  await page.getByRole("option", { name: "模型响应", exact: true }).click();
  await expect(page.getByRole("cell", { name: /Fixture-000/ })).toBeVisible();
  await expect(
    page.getByLabel("完整事实范围摘要", { exact: true }),
  ).toContainText("模型响应 83");
  await page.getByRole("button", { name: "下一页事实", exact: true }).click();
  await expect(page.getByRole("cell", { name: /Fixture-040/ })).toBeVisible();
  await page.getByRole("button", { name: "下一页事实", exact: true }).click();
  await expect(page.getByRole("cell", { name: /Fixture-080/ })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "下一页事实", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "上一页事实", exact: true }).click();
  await expect(page.getByRole("cell", { name: /Fixture-040/ })).toBeVisible();
  await expect(
    page.getByLabel("完整事实范围摘要", { exact: true }),
  ).toContainText("模型响应 83");
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter((c) =>
      c.startsWith("body:"),
    ),
  ).toEqual([]);
});

test("full facts storage consent can be withdrawn without retaining persistence", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/domain-readonly.html?unapproved=1&fullFacts=1",
  );
  await page.getByRole("button", { name: "统一会话展示", exact: true }).click();
  await page
    .getByRole("checkbox", {
      name: "保存元数据（重开后需要重新授权）",
      exact: true,
    })
    .check();
  await page
    .getByRole("button", { name: "确认加入统一展示", exact: true })
    .click();
  await expect(page.getByText("已加入统一展示 · 保存元数据")).toBeVisible();
  await page.getByRole("button", { name: "停止统一展示", exact: true }).click();
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter((c) =>
      c.startsWith("consent:"),
    ),
  ).toEqual(["consent:true:true", "consent:false:false"]);
});
