import { expect, test } from "@playwright/test";
import type {} from "./account-forecast-harness";

async function openConnection(
  page: import("@playwright/test").Page,
  query = "",
) {
  await page.goto(`/tests/frontend/account-forecast.html${query}`);
  await page.getByRole("tab", { name: "连接与额度", exact: true }).click();
}

test("Provider detail isolates linked accounts, keeps unknown and zero, and selects quota windows by keyboard", async ({
  page,
}) => {
  await openConnection(page);
  const quota = page.getByRole("region", {
    name: "账户用量与额度",
    exact: true,
  });
  const forecast = quota.getByRole("region", {
    name: "账户额度预测",
    exact: true,
  });
  await expect(quota).toContainText("已关联账户 A");
  await expect(quota).not.toContainText("其他连接账户 B");
  await expect(quota).not.toContainText("未关联本机订阅");
  await expect(forecast).toContainText("1 个窗口可能提前用完");
  await expect(
    forecast.getByRole("row").filter({ hasText: "7 天窗口" }),
  ).toContainText("0% / 100%");
  const details = forecast.getByRole("region", { name: "选中额度窗口预测" });
  const unknown = forecast.getByRole("row").filter({ hasText: "未知用量窗口" });
  await unknown.focus();
  await page.keyboard.press("Enter");
  await expect(details).toContainText("来源未提供已用百分比");
  await expect(details.getByRole("figure")).toHaveCount(0);
  await forecast.getByRole("row").filter({ hasText: "缺少周期窗口" }).click();
  await expect(details).toContainText("数据不足，暂无法预测");
  await expect
    .poll(() => page.evaluate(() => window.accountForecastFixture.refreshes))
    .toBe(0);
  await page.getByRole("button", { name: "切换测试连接" }).click();
  await page.getByRole("tab", { name: "连接与额度", exact: true }).click();
  await expect(quota).toContainText("其他连接账户 B");
  await expect(quota).not.toContainText("已关联账户 A");
});

test("default and pooled accounts display quota admission separately from the snapshot and enabled state", async ({
  page,
}) => {
  await openConnection(page, "?mode=routing-quota");
  const quota = page.getByRole("region", {
    name: "账户用量与额度",
    exact: true,
  });
  await expect(quota).toContainText("已读取");
  await expect(quota).toContainText("模型服务 · 已知额度已用尽");
  await page.getByRole("tab", { name: "上游账号", exact: true }).click();
  const row = page.getByRole("row").filter({ hasText: "验收账号" });
  await expect(row).toContainText("已启用");
  await expect(row).toContainText("已知额度已用尽");
  await expect(row).not.toContainText("无活跃冷却");
  await row.getByRole("button", { name: "验证与状态 验收账号" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("region", { name: "冷却记录" })).toContainText(
    "已知额度已用尽",
  );
  await dialog.getByRole("tab", { name: "用量与额度", exact: true }).click();
  await expect(dialog).toContainText("模型服务 · 已知额度已用尽");
  await dialog.getByRole("tab", { name: "验证与冷却", exact: true }).click();
  await page.evaluate(() => {
    window.accountForecastFixture.routingQuota = "recovery";
    window.dispatchEvent(new Event("account-quota-changed"));
  });
  await expect(dialog.getByRole("region", { name: "冷却记录" })).toContainText(
    "待额度恢复探测",
  );
});

test("snapshot expiry and directory read failures keep observed values and suppress forecasts", async ({
  page,
}) => {
  await openConnection(page, "?mode=expires");
  const forecast = page.getByRole("region", {
    name: "账户额度预测",
    exact: true,
  });
  await expect(forecast).toContainText("快照已过期，刷新账户用量后再预测", {
    timeout: 8000,
  });
  await expect(
    forecast.getByRole("row").filter({ hasText: "5 小时窗口" }),
  ).toContainText("80% / 20%");
  await expect(forecast.getByRole("figure")).toHaveCount(0);
  await openConnection(page);
  await page.evaluate(() => {
    window.accountForecastFixture.directoryFailed = true;
  });
  await page.getByRole("button", { name: "重新读取目录", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "账户用量与额度", exact: true }),
  ).toContainText("账户目录未更新");
  await expect(forecast).toContainText("读取未成功，保留上次快照，暂停预测");
  await expect(forecast.getByRole("figure")).toHaveCount(0);
});

test("API balances, unread accounts, missing associations and directory failure never become fake quota percentages", async ({
  page,
}) => {
  await openConnection(page, "?mode=money");
  const quota = page.getByRole("region", {
    name: "账户用量与额度",
    exact: true,
  });
  await expect(quota).toContainText("USD 12.34");
  await expect(quota).toContainText("此账户未提供百分比额度窗口");
  await expect(quota.getByRole("figure")).toHaveCount(0);
  await openConnection(page, "?mode=unread");
  await expect(quota).toContainText("尚未读取账户快照，额度预测保持未知");
  await openConnection(page, "?mode=empty");
  await expect(quota).toContainText("此连接暂无账户额度数据");
  await expect(quota).not.toContainText("未关联本机订阅");
  await openConnection(page, "?mode=directory-error");
  await expect(quota).toContainText("账户目录未更新");
  await expect(quota).not.toContainText("此连接暂无账户额度数据");
});

test("forecast table owns narrow overflow, and dark mode preserves accessible controls", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await openConnection(page, "?dark");
  const forecast = page.getByRole("region", {
    name: "账户额度预测",
    exact: true,
  });
  await expect(
    forecast.getByRole("figure", { name: "5 小时窗口额度预测图" }),
  ).toBeVisible();
  await expect
    .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
    .toBeLessThanOrEqual(720);
  await page.setViewportSize({ width: 390, height: 844 });
  await expect
    .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
    .toBeLessThanOrEqual(390);
  await forecast.getByRole("row").filter({ hasText: "7 天窗口" }).click();
  await expect(
    forecast.getByRole("region", { name: "选中额度窗口预测" }),
  ).toContainText("重置时预计已用 0%");
});
