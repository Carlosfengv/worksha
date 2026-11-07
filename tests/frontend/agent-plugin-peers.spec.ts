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
