import { expect, test } from "@playwright/test";

test("official declarations and scoped live samples remain separate, including failures and unknowns", async ({
  page,
}) => {
  await page.clock.install({ time: new Date("2026-10-26T06:00:00Z") });
  await page.goto(
    "/tests/frontend/provider-model-evidence.html?observations=live",
  );
  await page
    .getByRole("button", { name: "读取全部模型目录", exact: true })
    .click();
  const table = page.getByRole("region", {
    name: "全部 Provider 模型",
    exact: true,
  });
  const row = (protocol: string, model: string) =>
    table
      .getByRole("row")
      .filter({ has: page.getByRole("link", { name: protocol, exact: true }) })
      .filter({ has: page.getByText(model, { exact: true }) });
  await expect(
    table.getByRole("columnheader", { name: "目录 / 官方声明", exact: true }),
  ).toBeVisible();
  const pro = row("chat-completions", "deepseek-v4-pro");
  await expect(pro.getByRole("cell").nth(5)).toContainText("JSON 模式：支持");
  await expect(pro.getByRole("cell").nth(6)).toContainText(
    "JSON 模式：通过（样本）",
  );
  await expect(pro.getByRole("cell").nth(6)).toContainText("JSON 稳定性：未知");
  await pro.getByRole("button", { name: "实测详情", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "模型实测记录" });
  await expect(dialog).toContainText("OpenCode 1.18.13 · 2026-10-26");
  await expect(dialog).toContainText("3 次中 2 次有效、1 次无效");
  await expect(dialog).toContainText("首次无效输出保留");
  await expect(dialog).toContainText("实际上下文容量：未知");
  await expect(dialog).toContainText("最大输出：未知");
  await expect(dialog).toContainText("JSON Schema：未知");
  await page.keyboard.press("Escape");
  await expect(row("responses", "deepseek-v4-pro")).toContainText(
    "暂无匹配的实测记录",
  );
  await expect(row("messages", "deepseek-v4-pro")).toContainText(
    "暂无匹配的实测记录",
  );
  const alias = row("messages", "deepseek-v4-pro[1m]");
  await expect(alias.getByRole("cell").nth(5)).toContainText("工具调用：未知");
  await expect(alias.getByRole("cell").nth(6)).toContainText(
    "工具调用：通过（样本）",
  );
  await expect(alias.getByRole("cell").nth(6)).toContainText("[1M] 标记：未知");
  await alias.getByRole("button", { name: "实测详情", exact: true }).click();
  await expect(dialog).toContainText("Claude Code 2.1.310");
  await expect(dialog).toContainText("OpenCode 1.18.13");
  await expect(dialog).toContainText("JSON 模式：未知");
  await page.setViewportSize({ width: 720, height: 800 });
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  await expect(dialog).toBeInViewport();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.keyboard.press("Escape");
  await expect(row("messages", "unknown[1M]")).toContainText(
    "暂无匹配的实测记录",
  );
});

test("unreadable evidence is unknown without hiding official metadata", async ({
  page,
}) => {
  await page.clock.install({ time: new Date("2026-10-26T06:00:00Z") });
  await page.goto(
    "/tests/frontend/provider-model-evidence.html?observations=unavailable",
  );
  await page
    .getByRole("button", { name: "读取全部模型目录", exact: true })
    .click();
  await expect(page.getByText("实测未知 ·记录读取失败").first()).toBeVisible();
  await expect(
    page.getByText("1,048,576 Token", { exact: true }).first(),
  ).toBeVisible();
});
