import { expect, test } from "@playwright/test";
import type {} from "./plugin-directory-harness";

test("new read-only directory product reaches filters, detail and scoped consent without a model adapter", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/tests/frontend/plugin-directory.html");
  await expect(
    page.getByRole("heading", { name: "Fixture Reader", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("此来源提供历史与观测读取，尚未提供模型连接切换。", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "切换 Provider", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("combobox", { name: "请求 Agent", exact: true }).click();
  await page
    .getByRole("option", { name: "Fixture Reader", exact: true })
    .click();
  await expect(page.getByTestId("agent-query")).toHaveText("fixture-reader");
  await page.getByRole("button", { name: "更新目录", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Renamed Reader", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("combobox", { name: "请求 Agent", exact: true }),
  ).toContainText("Renamed Reader");
  await expect(page.getByTestId("agent-query")).toHaveText("fixture-reader");
  await page.getByRole("button", { name: "读取本机会话", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Renamed Reader");
  await expect(dialog).toContainText("/fixture/readonly");
  await expect(dialog).not.toContainText("此来源由 CLI");
  await page.keyboard.press("Escape");
  expect(await page.evaluate(() => window.directoryWrites)).toEqual([]);
  await page.getByRole("button", { name: "读取本机会话", exact: true }).click();
  await dialog.getByRole("button", { name: "开始读取", exact: true }).click();
  await page.getByRole("button", { name: "停止读取", exact: true }).click();
  expect(await page.evaluate(() => window.directoryWrites)).toEqual([
    "fixture-reader:true",
    "fixture-reader:false",
  ]);
  expect(await page.evaluate(() => window.directoryReads)).toEqual([
    "fixture-reader",
    "fixture-reader",
    "fixture-reader",
  ]);
  expect(errors).toEqual([]);
});
