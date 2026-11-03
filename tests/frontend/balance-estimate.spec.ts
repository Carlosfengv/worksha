import { expect, test } from "@playwright/test";
import type {} from "./account-forecast-harness";

async function openBalance(
  page: import("@playwright/test").Page,
  mode: string,
) {
  await page.goto(`/tests/frontend/account-forecast.html?mode=${mode}`);
  await page.getByRole("tab", { name: "连接与额度", exact: true }).click();
  return page.getByRole("region", { name: "余额消耗估算", exact: true });
}

test("same-currency full aggregate produces a labelled seven-day balance consumption estimate", async ({
  page,
}) => {
  const estimate = await openBalance(page, "balance-ready");
  await expect(estimate).toContainText("约 8.6 天");
  await expect(estimate).toContainText("7 CNY · 估算");
  await expect(estimate).toContainText("1 CNY / 天");
  await expect(estimate).toContainText("不能代表完整账单");
  await expect
    .poll(() =>
      page.evaluate(() =>
        window.accountForecastFixture.usageQueries.some(
          (query) =>
            query.source === "gateway" &&
            query.days === 7 &&
            query.providerId === "provider-a" &&
            query.agent === undefined &&
            query.model === undefined &&
            query.cursor === undefined,
        ),
      ),
    )
    .toBe(true);
  expect(
    await page.evaluate(() => window.accountForecastFixture.refreshes),
  ).toBe(0);
});

test("currency mismatch, missing costs, stale balance and sparse samples preserve unknown duration", async ({
  page,
}) => {
  for (const [mode, reason] of [
    ["balance-mismatch", "币种不一致：余额为 CNY，调用费用为 USD"],
    ["balance-unknown", "1 次调用费用未知"],
    ["balance-stale", "余额快照已过期"],
    ["balance-few", "费用样本不足"],
  ]) {
    const estimate = await openBalance(page, mode);
    await expect(estimate).toContainText(reason);
    await expect(
      estimate.getByRole("status", { name: "剩余时长估算" }),
    ).toHaveText("未知");
    await expect(estimate).not.toContainText("约 8.6 天");
  }
});

test("failed refresh preserves marked costs and retry restores the estimate without external account reads", async ({
  page,
}) => {
  const estimate = await openBalance(page, "balance-ready");
  await expect(estimate).toContainText("约 8.6 天");
  await page.evaluate(() => {
    window.accountForecastFixture.usageFailed = true;
  });
  await estimate.getByRole("button", { name: "重新读取调用费用" }).click();
  await expect(estimate).toContainText("隔离调用统计读取失败");
  await expect(estimate).toContainText("上次统计");
  await expect(estimate).toContainText("7 CNY · 估算");
  await expect(
    estimate.getByRole("status", { name: "剩余时长估算" }),
  ).toHaveText("未知");
  await page.evaluate(() => {
    window.accountForecastFixture.usageFailed = false;
  });
  await estimate.getByRole("button", { name: "重新读取调用费用" }).click();
  await expect(estimate).toContainText("约 8.6 天");
  expect(
    await page.evaluate(() => window.accountForecastFixture.refreshes),
  ).toBe(0);
});

test("loading and a switch to another provider never expose a cached estimate for the previous account", async ({
  page,
}) => {
  const estimate = await openBalance(page, "balance-loading");
  await expect(estimate).toContainText("正在读取近 7 天调用费用");
  await expect(
    estimate.getByRole("status", { name: "剩余时长估算" }),
  ).toHaveText("未知");
  await page.evaluate(() => window.accountForecastFixture.finishUsage?.());
  await expect(estimate).toContainText("约 8.6 天");
  await page.getByRole("button", { name: "切换测试连接" }).click();
  await page.getByRole("tab", { name: "连接与额度", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "账户用量与额度", exact: true }),
  ).toContainText("其他连接账户 B");
  await expect(
    estimate.getByRole("status", { name: "剩余时长估算" }),
  ).toHaveText("未知");
  await expect(estimate).not.toContainText("约 8.6 天");
});

test("balance estimate wraps in narrow and dark layouts", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(
    "/tests/frontend/account-forecast.html?mode=balance-mismatch&dark",
  );
  await page.getByRole("tab", { name: "连接与额度", exact: true }).click();
  const estimate = page.getByRole("region", {
    name: "余额消耗估算",
    exact: true,
  });
  await expect(estimate).toContainText("币种不一致");
  await expect
    .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
    .toBeLessThanOrEqual(390);
});

test("official CNY estimates retain all 18 missing costs and recorded reasons without a balance duration", async ({
  page,
}) => {
  const estimate = await openBalance(page, "balance-cny");
  await expect(estimate).toContainText("11.904978 CNY · 估算 / 18 次未知");
  await expect(estimate).toContainText("13 次：上游拒绝跨协议请求");
  await expect(estimate).toContainText(
    "2 次：上游返回当时无法转换的思考或拒绝内容",
  );
  await expect(estimate).toContainText("3 次：客户端中断");
  await expect(estimate).toContainText(
    "18 次调用费用未知，不能用已知部分推算剩余时长",
  );
  await expect(
    estimate.getByRole("status", { name: "剩余时长估算" }),
  ).toHaveText("未知");
  await expect(estimate).not.toContainText("币种不一致");
  await expect(estimate).not.toContainText("USD");
  await page.setViewportSize({ width: 390, height: 844 });
  await expect
    .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
    .toBeLessThanOrEqual(390);
  expect(
    await page.evaluate(() => window.accountForecastFixture.refreshes),
  ).toBe(0);
});

test("unread account polling and confirmed balance refresh keep one quota section and one estimate", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  const estimate = await openBalance(page, "balance-unread");
  const quota = page.getByRole("region", { name: "账户额度预测", exact: true });
  const account = page.getByRole("region", {
    name: "账户用量与额度",
    exact: true,
  });
  await expect(quota).toHaveCount(1);
  await expect(estimate).toContainText("尚未读取余额快照");
  for (let i = 0; i < 3; i++) {
    await account.getByRole("button", { name: "重新读取目录" }).click();
    await expect(account).not.toContainText("正在读取账户目录");
    await expect(quota).toHaveCount(1);
    await expect(estimate).toHaveCount(1);
  }
  await account.getByRole("button", { name: "读取真实账户" }).click();
  await page.getByRole("button", { name: "确认读取", exact: true }).click();
  await expect(estimate).toContainText("约 8.6 天");
  await expect(quota).toHaveCount(1);
  await expect(quota).toContainText("此账户未提供百分比额度窗口");
  await expect(account).not.toContainText("尚未读取账户快照");
  expect(errors).toEqual([]);
});
