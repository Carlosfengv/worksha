import { expect, test } from "@playwright/test";

test("上传来源与系统 Agent 共用列表，详情只展示已声明能力", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=agents&uploadedAgent#/agents",
  );
  const table = page
    .getByLabel("Agent 列表", { exact: true })
    .getByRole("table");
  for (const name of [
    "Codex CLI",
    "Claude Code CLI",
    "OpenCode",
    "DeepSeek Harness 桌面端",
  ])
    await expect(table.getByRole("row").filter({ hasText: name })).toHaveCount(
      1,
    );
  const row = table
    .getByRole("row")
    .filter({ hasText: "DeepSeek Harness 桌面端" });
  await expect(row).toContainText("尚未授权读取");
  await expect(row).toContainText("仅观测");
  await row.getByRole("link", { name: "详情 DeepSeek Harness 桌面端" }).click();
  const tabs = page.getByRole("tablist", { name: "Agent 详情" });
  await expect(tabs.getByRole("tab")).toHaveCount(2);
  await expect(tabs.getByRole("tab", { name: "Sessions" })).toBeVisible();
  await tabs.getByRole("tab", { name: "连接与观测" }).click();
  await expect(
    page.getByRole("link", { name: "管理 Agent 接入" }),
  ).toHaveAttribute("href", "#/plugins?scene=agents");
});

test("Agent 接入场景同时展示系统自带与用户上传的接入", async ({ page }) => {
  await page.goto("/tests/frontend/plugin-center.html?mode=agent-peers");
  await page.getByRole("button", { name: "Agent 接入", exact: true }).click();
  const table = page
    .getByLabel("插件中心列表", { exact: true })
    .getByRole("table");
  for (const name of [
    "Claude 接入",
    "Codex 接入",
    "OpenCode 接入",
    "DSH 桌面端",
  ]) {
    const row = table.getByRole("row").filter({ hasText: name });
    await expect(row).toContainText(
      name === "DSH 桌面端" ? "用户上传" : "系统自带",
    );
    await expect(row.getByRole("switch")).toHaveCount(1);
  }
});

test("DSH 从列表选择 Provider 后选模型，授权后预览并可回到列表恢复", async ({
  page,
}) => {
  const name = "DeepSeek Harness 桌面端";
  await page.goto(
    "/tests/frontend/tables.html?mode=agents&uploadedAgent&connectionAdapter#/agents",
  );
  const current = page.getByLabel(`${name} 当前连接`);
  const picker = page.getByRole("combobox", { name: `${name} 模型连接` });
  const dialog = page.getByRole("dialog");
  await expect(current).toHaveText("切换时授权");
  await picker.click();
  await expect(
    page.getByRole("group", { name: "Provider", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("group", { name: "模型服务", exact: true }),
  ).toBeVisible();
  await page.getByRole("option", { name: "连接 000" }).click();
  await expect(
    dialog.getByRole("button", { name: "授权并预览" }),
  ).toBeDisabled();
  const model = dialog.getByRole("combobox", { name: "选择模型" });
  await model.fill("model-042");
  await page.getByRole("option", { name: "model-042", exact: true }).click();
  await dialog.getByRole("button", { name: "授权并预览" }).click();
  await expect(dialog).toContainText("新会话");
  await expect(dialog).toContainText("model-042");
  await expect(current).toHaveText("dsh · native");
  await dialog.getByRole("button", { name: "确认切换默认模型" }).click();
  await expect(current).toHaveText("Provider · 连接 000 · model-042");

  await page.getByRole("link", { name: `详情 ${name}` }).click();
  await expect(page.getByRole("tab", { name: "Sessions" })).toBeVisible();
  await page.getByRole("link", { name: "返回 Agents" }).click();
  await expect(current).toHaveText("Provider · 连接 000 · model-042");
  await page.getByRole("button", { name: "恢复变更前的连接" }).click();
  await dialog.getByRole("button", { name: "确认恢复" }).click();
  await expect(current).toHaveText("dsh · native");
  await expect(
    page.getByRole("button", { name: "恢复变更前的连接" }),
  ).toHaveCount(0);
});

test("声明模型连接能力的新 Agent 复用列表入口和模型服务恢复", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=agents&uploadedAgent&connectionAdapter&genericAgent#/agents",
  );
  const current = page.getByRole("status", { name: "Example Agent 当前连接" });
  const dialog = page.getByRole("dialog");
  await page.getByRole("combobox", { name: "Example Agent 模型连接" }).click();
  await page.getByRole("option", { name: "Fixture model service" }).click();
  await expect(dialog.getByRole("combobox", { name: "选择模型" })).toHaveCount(
    0,
  );
  await dialog.getByRole("button", { name: "取消" }).click();
  await expect(current).toHaveText("切换时授权");

  await page.getByRole("combobox", { name: "Example Agent 模型连接" }).click();
  await page.getByRole("option", { name: "Fixture model service" }).click();
  await dialog.getByRole("button", { name: "授权并预览" }).click();
  await dialog.getByRole("button", { name: "确认切换默认模型" }).click();
  await expect(current).toHaveText(
    "模型服务 · Fixture model service · Fixture default model",
  );
  await page.getByRole("button", { name: "恢复变更前的连接" }).click();
  await dialog.getByRole("button", { name: "确认恢复" }).click();
  await expect(current).toHaveText("dsh · native");
});
