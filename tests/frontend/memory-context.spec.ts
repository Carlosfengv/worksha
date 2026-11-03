import { test, expect } from "@playwright/test";
import type {} from "./memory-context-fixture";

for (const agent of [
  "claude-code-cli",
  "claude-code-desktop",
  "codex-cli",
  "codex-desktop",
  "opencode",
]) {
  test(`${agent} memory lists, searches and reads without claiming loaded state`, async ({
    page,
  }) => {
    await page.goto(
      `/tests/frontend/tables.html?mode=activity#/agents/${agent}?tab=memory`,
    );
    const region = page.getByRole("region", { name: "记忆文件列表" });
    await expect(region.getByRole("row")).toHaveCount(11);
    await expect(
      page.getByText("已达读取上限", { exact: false }),
    ).toBeVisible();
    await expect(
      region.getByText("已发现 · 加载情况未知").first(),
    ).toBeVisible();
    expect(await page.evaluate(() => window.memoryQueries)).toEqual([
      { agent },
    ]);
    await page
      .getByRole("textbox", { name: "搜索记忆文件" })
      .fill("memory-24.md");
    await expect(region.getByRole("row")).toHaveCount(2);
    await region
      .getByRole("button", { name: "查看 memory-24.md", exact: true })
      .click();
    const preview = page.getByRole("region", { name: "记忆正文" });
    await expect(preview).toContainText("memory body · [已隐藏凭据]");
    await expect(preview).toContainText("正文仅显示前部");
    await page.evaluate(() => {
      window.memoryFail = true;
    });
    await page
      .getByRole("button", { name: "重新读取记忆", exact: true })
      .click();
    await expect(page.getByRole("alert")).toContainText("权限已撤销");
    await expect(preview).toHaveCount(0);
    await page.evaluate(() => {
      window.memoryFail = false;
    });
    await page
      .getByRole("button", { name: "重新读取记忆", exact: true })
      .click();
    await expect(preview).toBeVisible();
    await preview.getByRole("button", { name: "关闭正文" }).click();
    await expect(preview).toHaveCount(0);
  });
}

test("Session context distinguishes observed zero, calculated percentage, compression and actual reads; refresh retains tab", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity#/sessions/codex%3Aa",
  );
  await page.getByRole("tab", { name: "上下文", exact: true }).click();
  const context = page.getByRole("region", { name: "会话上下文" });
  await expect(context).toContainText("0 Token");
  await expect(context).toContainText("0.0%");
  await expect(context.getByText("真实采集", { exact: true })).toHaveCount(2);
  await expect(context.getByText("估算", { exact: true })).toHaveCount(1);
  await context
    .getByRole("button", { name: "上下文压缩边界", exact: true })
    .click();
  await expect(context).toContainText("retained summary");
  await context
    .getByRole("button", { name: /读取记忆 \/ 规则：AGENTS.md/ })
    .click();
  await expect(context).toContainText("actual loaded rule text");
  await expect(context).toContainText("当前是否仍在上下文中未知");
  await expect(context).toContainText("内容已截断");
  await page.getByRole("button", { name: "刷新对话", exact: true }).click();
  await expect(
    page.getByRole("tab", { name: "上下文", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(context).toContainText("0.0%");
  await page.evaluate(() => {
    window.contextMissing = true;
  });
  await page.getByRole("button", { name: "刷新对话", exact: true }).click();
  await expect(context).toContainText("尚无原生上下文用量样本");
  await expect(context).not.toContainText("0.0%");
  await expect(context).not.toContainText("0 Token");
});

test("memory and context stay scrollable in a narrow window", async ({
  page,
}) => {
  await page.setViewportSize({ width: 740, height: 700 });
  await page.goto(
    "/tests/frontend/tables.html?mode=activity#/agents/codex-cli?tab=memory",
  );
  await page
    .getByRole("button", { name: "查看 memory-0.md", exact: true })
    .click();
  await expect(page.getByRole("region", { name: "记忆正文" })).toBeVisible();
  await expect(page.getByRole("region", { name: "记忆正文" })).toBeInViewport();
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth),
  ).toBeLessThanOrEqual(740);
});
