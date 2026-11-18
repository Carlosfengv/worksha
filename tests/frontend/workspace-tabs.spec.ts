import { expect, test } from "@playwright/test";

const start = "/tests/frontend/tables.html?mode=workspace#/requests?range=all";
const tabSelector = "[data-workspace-tab]";
test("reuses the preview and preserves Home scroll, bounds, and filters", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 860 });
  await page.goto(start);
  const layout = page.locator("[data-workspace-layout]");
  const bound = await layout.boundingBox();
  const preview = page.locator(tabSelector);
  for (const id of ["req-1", "req-2", "req-3"]) {
    await page.locator(`[data-request-id="${id}"]`).click();
    await expect(preview).toHaveCount(1);
    await expect(preview).toHaveAttribute(
      "data-workspace-tab",
      `request:${id}`,
    );
    await expect(
      page.getByRole("complementary", { name: "应用导航" }),
    ).toHaveCount(0);
    expect(await layout.boundingBox()).toEqual(bound);
    await page
      .getByRole("button", { name: /返回|← 调用与工具/ })
      .first()
      .click();
    await expect(page).toHaveURL(/range=all/);
  }
  await page.locator('[data-request-id="req-4"]').dblclick();
  await expect(
    page.locator('[data-workspace-tab="request:req-4"]'),
  ).toHaveAttribute("data-pinned", "true");
  await page.keyboard.press("Meta+1");
  const list = page.locator("[data-request-toolbar]").locator("..");
  await list.evaluate((element) => {
    element.scrollTop = 300;
  });
  const scroll = await list.evaluate((element) => element.scrollTop);
  expect(scroll).toBeGreaterThan(0);
  await page.keyboard.press("Meta+k");
  await page.getByPlaceholder("搜索已打开的标签…").fill("req-4");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/requests\/req-4/);
  await page.keyboard.press("Meta+w");
  await expect(
    page.locator('[data-workspace-tab="request:req-4"]'),
  ).toHaveCount(0);
  await page.keyboard.press("Meta+1");
  expect(await list.evaluate((element) => element.scrollTop)).toBe(scroll);
});

test("long pinned tabs preserve a native drag region and pause hidden detail polling", async ({
  page,
}) => {
  await page.goto(start);
  for (let i = 1; i <= 10; i++) {
    await page
      .locator(`[data-request-id="req-${i}"]`)
      .click({ modifiers: ["Meta"] });
    await expect(page.locator(tabSelector)).toHaveCount(i);
    if (i < 10)
      await page.getByRole("button", { name: "Home", exact: true }).click();
  }
  for (const width of [1280, 960]) {
    await page.setViewportSize({ width, height: 860 });
    expect(
      (await page.getByLabel("窗口拖拽区").boundingBox())!.width,
    ).toBeGreaterThanOrEqual(64);
  }
  await expect
    .poll(() => page.evaluate(() => window.workspaceReads["req-10"] ?? 0))
    .toBeGreaterThan(0);
  const reads = await page.evaluate(() => ({ ...window.workspaceReads }));
  await expect
    .poll(() => page.evaluate(() => window.workspaceReads["req-10"]), {
      timeout: 8_000,
    })
    .toBeGreaterThan(reads["req-10"]);
  expect(await page.evaluate(() => window.workspaceReads["req-1"])).toBe(
    reads["req-1"],
  );
});

test("linked sessions open in a second pinned tab and keep the request", async ({
  page,
}) => {
  await page.goto(start);
  const request = page.locator('[data-workspace-tab="request:req-1"]');
  await page.locator('[data-request-id="req-1"]').click();
  await expect(request).toHaveAttribute("data-pinned", "false");
  await page.getByRole("link", { name: "打开关联会话" }).click();
  const session = page.locator(
    '[data-workspace-tab="session:workspace-linked-session"]',
  );
  await expect(session).toHaveAttribute("data-pinned", "true");
  await expect(page).toHaveURL(/sessions\/workspace-linked-session/);
  await expect(page.locator(tabSelector)).toHaveCount(2);
  await request.click();
  await expect(page).toHaveURL(/requests\/req-1/);
  await session.click();
  await expect(page).toHaveURL(/sessions\/workspace-linked-session/);
});

test("publishes the selected provider model and protects referenced connections", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=workspace&providerLifecycle#/providers",
  );
  await page.getByRole("button", { name: "添加供应商", exact: true }).click();
  const wizard = page.getByRole("region", { name: "添加模型供应商" });
  await wizard.getByRole("combobox", { name: "供应商", exact: true }).click();
  await page.getByRole("option", { name: "Ollama", exact: true }).click();
  await wizard.getByRole("combobox", { name: "接口协议", exact: true }).click();
  await page
    .getByRole("option", { name: "Chat Completions", exact: true })
    .click();
  await wizard
    .getByRole("textbox", { name: "连接名称（可选）" })
    .fill("验收供应商");
  await wizard.getByRole("button", { name: "下一步" }).click();
  await wizard.getByRole("button", { name: "继续并获取模型" }).click();
  await wizard.getByRole("combobox", { name: "默认模型" }).click();
  await page.getByRole("option", { name: /Test Alternate/ }).click();
  await wizard.getByRole("button", { name: "保存并前往绑定 Agent" }).click();
  await expect(page).toHaveURL(/#\/agents\/opencode-cli\?tab=connection/);

  await page.getByRole("button", { name: "Home", exact: true }).click();
  await page.getByRole("link", { name: "验收供应商", exact: true }).click();
  await page.getByRole("tab", { name: "模型", exact: true }).click();
  const model = page.getByRole("row", { name: /test-alternate/ });
  await expect(model).toContainText("默认模型");
  await model.getByRole("switch", { name: "启用模型 test-alternate" }).click();
  await expect(
    page.getByText("停用模型影响下游服务", { exact: true }),
  ).toBeVisible();
  const remove = page.getByRole("button", { name: "删除供应商", exact: true });
  await remove.click();
  const guard = page.getByRole("dialog", { name: "删除供应商" });
  await expect(guard).toContainText("供应商仍被下游引用");
  await expect(guard).toContainText("验收供应商");
  await expect(guard.getByRole("button", { name: "确认删除" })).toBeDisabled();
  await guard.getByRole("button", { name: "取消" }).click();

  await page.getByRole("button", { name: "Home", exact: true }).click();
  await page.getByRole("link", { name: "独立供应商", exact: true }).click();
  const standalone = page.locator('[data-workspace-tab="provider:fixture-0"]');
  await expect(standalone).toBeVisible();
  await remove.click();
  await guard.getByRole("button", { name: "确认删除" }).click();
  await expect(standalone).toHaveCount(0);
  await expect(page).toHaveURL(/#\/providers$/);
  await expect(
    page.getByRole("link", { name: "独立供应商", exact: true }),
  ).toHaveCount(0);
});

test("model service edits are promoted and every close path protects the draft", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  await page.getByRole("link", { name: "编程服务", exact: true }).click();
  const tab = page.locator('[data-workspace-tab^="service:"]');
  await expect(tab).toHaveAttribute("data-pinned", "false");
  const name = page.getByRole("textbox", { name: "服务名称", exact: true });
  await name.fill("编程服务待发布");
  await page.getByRole("button", { name: "Flash", exact: true }).click();
  const modelName = page.getByRole("textbox", {
    name: "显示名称",
    exact: true,
  });
  await expect(modelName).toHaveValue("Flash");
  await expect(tab).toHaveAttribute("data-pinned", "true");
  const guard = page.getByRole("dialog", { name: "标签含有未保存修改" });
  await page
    .getByRole("button", { name: "关闭标签 编程服务", exact: true })
    .click();
  await expect(guard).toBeVisible();
  await guard.getByRole("button", { name: "继续编辑" }).click();
  await page.keyboard.press("Meta+w");
  await expect(guard).toBeVisible();
  await guard.getByRole("button", { name: "继续编辑" }).click();
  await expect(name).toHaveValue("编程服务待发布");
  await page.getByRole("button", { name: "Home", exact: true }).click();
  await page
    .getByRole("button", { name: "+ 创建模型服务", exact: true })
    .click();
  await page.getByRole("button", { name: "标签操作", exact: true }).click();
  await page.getByRole("menuitem", { name: /关闭其他动态标签/ }).click();
  await expect(guard).toBeVisible();
  await guard.getByRole("button", { name: "继续编辑" }).click();
  await tab.getByRole("button", { name: /有未发布变更 编程服务/ }).click();
  await expect(name).toHaveValue("编程服务待发布");
  await expect(modelName).toHaveValue("Flash");
  await page.getByRole("button", { name: "标签操作", exact: true }).click();
  await page.getByRole("menuitem", { name: /关闭全部动态标签/ }).click();
  await expect(guard).toBeVisible();
  await guard.getByRole("button", { name: "放弃未保存修改" }).click();
  await expect(tab).toHaveCount(0);
  await expect(
    page.getByRole("link", { name: "编程服务", exact: true }),
  ).toBeVisible();
});

test("request filters remain within the first 172 pixels of the viewport", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 860 });
  await page.goto(start);
  const toolbar = page.locator("[data-request-toolbar]");
  await expect(
    toolbar.getByRole("slider", { name: "请求时间分布" }),
  ).toBeVisible();
  await expect(
    page.getByRole("link", { name: "MCP 工具目录与调用" }),
  ).toBeVisible();
  const bounds = (await toolbar.boundingBox())!;
  expect(bounds.y + bounds.height).toBeLessThanOrEqual(172);
});
