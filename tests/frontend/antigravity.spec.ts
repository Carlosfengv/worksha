import { expect, test } from "@playwright/test";
import type {} from "./antigravity-harness";

test("Antigravity source requires scoped confirmation; cancellation writes nothing and revocation is immediate", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/tests/frontend/antigravity.html");
  await expect(
    page.getByText("模型连接由 Antigravity 管理", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("未读取会话", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "读取本机会话", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Antigravity");
  await expect(dialog).toContainText("step_format=0");
  await expect(dialog).not.toContainText("此来源由 CLI");
  await expect(dialog).toContainText("不读取 IDE / CLI");
  await expect(dialog).toContainText(
    "/fixture/.gemini/antigravity/conversations",
  );
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  expect(await page.evaluate(() => window.antigravitySourceWrites)).toEqual([]);
  await page.getByRole("button", { name: "读取本机会话", exact: true }).click();
  await dialog.getByRole("button", { name: "开始读取", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(
    page.getByText("已读取 1 个历史会话", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "停止读取", exact: true }).click();
  await expect(page.getByText("未读取会话", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => window.antigravitySourceWrites)).toEqual([
    true,
    false,
  ]);
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth),
  ).toBeLessThanOrEqual(390);
  expect(errors).toEqual([]);
});
