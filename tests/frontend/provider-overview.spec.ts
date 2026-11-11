import { expect, test } from "@playwright/test";
import type {} from "./provider-overview-harness";

test("upstream account quota follows the exact provider and account version", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-overview.html?detail&mode=upstream-quota",
  );
  await page.getByRole("tab", { name: "上游账号", exact: true }).click();
  const inspect = page.getByRole("button", {
    name: "验证与状态 额度主账号",
    exact: true,
  });
  const dialog = page.getByRole("dialog", { name: "验证与状态 · 额度主账号" });
  const quotaTab = dialog.getByRole("tab", { name: "用量与额度", exact: true });
  await inspect.click();
  await quotaTab.click();
  const quota = dialog.getByRole("region", {
    name: "账户用量与额度",
    exact: true,
  });
  await expect(quota).toContainText("原版本 5 小时窗口");
  await expect(quota).toContainText("7 天窗口");
  await expect(quota).not.toContainText("其他供应商额度");
  await expect(quota).not.toContainText("8.52");
  await quota.getByRole("button", { name: "刷新账户用量" }).click();
  const confirm = page.getByRole("dialog", {
    name: "读取 当前订阅账号",
    exact: true,
  });
  await expect(confirm).toBeVisible();
  expect(
    await page.evaluate(() => window.providerOverviewFixture.quotaReads),
  ).toEqual([]);

  await page.evaluate(() => {
    window.providerOverviewFixture.upstreamRevision = 2;
    window.dispatchEvent(new Event("upstream-accounts-changed"));
  });
  await expect(confirm).toHaveCount(0);
  await expect(dialog).toContainText("当前账号或连接已变化");
  expect(
    await page.evaluate(() => window.providerOverviewFixture.quotaReads),
  ).toEqual([]);
  await dialog.getByRole("button", { name: "关闭", exact: true }).click();
  await inspect.click();
  await quotaTab.click();
  await expect(quota).toContainText("额度目录中的账号版本已变化");
  await expect(quota).not.toContainText("原版本 5 小时窗口");
  await page.evaluate(() => {
    window.providerOverviewFixture.quotaDirectoryRevision = 2;
  });
  await quota
    .getByRole("button", { name: "重新读取目录", exact: true })
    .click();
  await expect(quota).toContainText("尚未读取账户快照");
  await quota.getByRole("button", { name: "读取真实账户" }).click();
  await confirm.getByRole("button", { name: "确认读取", exact: true }).click();
  await expect(confirm).toHaveCount(0);
  await expect(quota).toContainText("当前版本 5 小时窗口");
  expect(
    await page.evaluate(() => window.providerOverviewFixture.quotaReads),
  ).toEqual([["upstream-quota-a", "quota-2"]]);
  await dialog.getByRole("button", { name: "关闭", exact: true }).click();
  await page.getByRole("button", { name: "验证与状态 未接入账号" }).click();
  const unsupported = page.getByRole("dialog", {
    name: "验证与状态 · 未接入账号",
  });
  await unsupported
    .getByRole("tab", { name: "用量与额度", exact: true })
    .click();
  await expect(unsupported).toContainText("此账号暂无可关联的额度来源");
  await expect(
    unsupported.getByRole("button", { name: "读取真实账户" }),
  ).toHaveCount(0);
});

test("upstream account status distinguishes expired cooldown and in-flight recovery", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-overview.html?detail&mode=routing-recovery",
  );
  await page.getByRole("tab", { name: "上游账号", exact: true }).click();
  for (const [account, phase, reason, scope] of [
    ["等待账号", "待恢复探测", "上游限流", "整个账号"],
    ["探测账号", "账号恢复探测中", "上游临时故障", "deepseek-v4-pro"],
  ]) {
    const row = page.getByRole("row").filter({ hasText: account });
    await expect(row).toContainText(phase);
    await expect(row).not.toContainText("无活跃冷却");
    await row.getByRole("button", { name: `验证与状态 ${account}` }).click();
    const dialog = page.getByRole("dialog", {
      name: `验证与状态 · ${account}`,
    });
    const record = dialog.getByRole("region", { name: "冷却记录" });
    await expect(record).toContainText(scope);
    await expect(record).toContainText(`${reason} · ${phase}`);
    await dialog.getByRole("button", { name: "关闭", exact: true }).click();
  }
});
test("subscription quota distinguishes authorization recovery from an unconfirmed plan", async ({
  page,
}) => {
  for (const [mode, label] of [
    ["subscription-authorization", "订阅授权未通过"],
    ["subscription-unconfirmed", "订阅状态待确认"],
  ]) {
    await page.goto(
      `/tests/frontend/provider-overview.html?detail&mode=${mode}`,
    );
    await page.getByRole("button", { name: "查看账户与读取用量" }).click();
    const dialog = page.getByRole("dialog", {
      name: "账户用量与读取",
      exact: true,
    });
    await expect(dialog.getByText(label, { exact: true })).toBeVisible();
    await expect(
      dialog.getByRole("button", { name: "官方用量与账单" }),
    ).toBeVisible();
    const recovery = dialog.getByText(/在供应商的「上游账号」更新该账号/);
    if (mode === "subscription-authorization")
      await expect(recovery).toBeVisible();
    else {
      await expect(dialog.getByText(/请核对订阅状态后稍后刷新/)).toBeVisible();
      await expect(recovery).toHaveCount(0);
    }
  }
});

test("Provider list shows every account's core statistics before selection with one full aggregate and no external reads", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-overview.html");
  await expect(
    page.getByRole("tab", { name: "用量概览", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  const list = page.getByLabel("Provider 与计费账户概览", { exact: true });
  const api = list.getByRole("row").filter({ hasText: "MyDeepSeek" });
  const other = list.getByRole("row").filter({ hasText: "另一个计费连接" });
  const subscription = list.getByRole("row").filter({ hasText: "Codex 订阅" });
  await expect(api).toContainText("8.52 CNY");
  await expect(api).toContainText("11.93 CNY · 估算 / 18 次未知");
  await expect(api).toContainText("11,520,138 · 部分");
  await expect(other).toContainText("12.34 USD");
  await expect(other).toContainText("0 USD · 实际");
  await expect(other.getByRole("cell").nth(3)).toHaveText("10");
  await expect(other).not.toContainText("11.93");
  await expect(subscription).toContainText("5 小时窗口 · 剩余 20%");
  await expect(subscription).toContainText("7 天窗口 · 剩余 100%");
  await expect(subscription.getByText(/重置$/)).toHaveCount(2);
  const overview = page.getByRole("region", {
    name: "MyDeepSeek用量概览",
    exact: true,
  });
  await expect(overview).toHaveCount(0);
  expect(
    await page.evaluate(() => window.providerOverviewFixture.refreshes),
  ).toBe(0);
  const queries = await page.evaluate(
    () => window.providerOverviewFixture.usageQueries,
  );
  expect(queries).toEqual([{ source: "gateway", days: 7 }]);
  await api.click();
  await expect(overview).toContainText("11.934522 CNY · 估算 / 18 次未知");
  await expect(overview).toContainText("291");
  await expect(overview).toContainText("18 次调用费用未知");
  await expect(overview).toContainText("13 次：上游拒绝跨协议请求");
  await expect(overview).not.toContainText("12.34 USD");
  await expect(api).toContainText("11.93 CNY · 估算 / 18 次未知");
});

test("native subscription is selectable without an API binding and never shows API spending", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-overview.html");
  const row = page.getByRole("row").filter({ hasText: "Codex 订阅" });
  await row.focus();
  await page.keyboard.press("Enter");
  const overview = page.getByRole("region", {
    name: "Codex 订阅用量概览",
    exact: true,
  });
  await expect(overview).toContainText("Pro");
  await expect(overview).toContainText("80% / 20%");
  await expect(overview).toContainText("0% / 100%");
  await expect(overview.getByRole("figure")).toBeVisible();
  await expect(overview).not.toContainText("近 7 天费用");
  // Accessible chart timestamps may contain "8.52"; check the actual money
  // sentinels, including their currencies, rather than an arbitrary substring.
  await expect(overview).not.toContainText("8.52 CNY");
  await expect(overview).not.toContainText("11.934522 CNY");
  expect(
    await page.evaluate(() => window.providerOverviewFixture.usageQueries),
  ).toEqual([{ source: "gateway", days: 7 }]);
});

test("account switching keeps currencies and zero separate, and expiry suppresses subscription forecast", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-overview.html?mode=expires");
  await page.getByRole("row").filter({ hasText: "另一个计费连接" }).click();
  const other = page.getByRole("region", {
    name: "另一个计费连接用量概览",
    exact: true,
  });
  await expect(other).toContainText("0 USD · 实际");
  await expect(other).not.toContainText("11.934522");
  await page.getByRole("row").filter({ hasText: "Codex 订阅" }).click();
  const sub = page.getByRole("region", {
    name: "Codex 订阅用量概览",
    exact: true,
  });
  await expect(sub).toContainText("快照已过期", { timeout: 6000 });
  await expect(sub.getByRole("figure")).toHaveCount(0);
  await expect(
    page.getByRole("row").filter({ hasText: "Codex 订阅" }),
  ).toContainText("剩余 20%");
});

test("detail starts with overview and isolates linked accounts, while confirmed reads retain their existing dialog", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-overview.html?detail&mode=unread");
  await expect(
    page.getByRole("tab", { name: "概览", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(
    page.getByRole("region", { name: "Provider 用量概览", exact: true }),
  ).not.toContainText("Codex 订阅");
  await page.getByRole("button", { name: "查看账户与读取用量" }).click();
  const dialog = page.getByRole("dialog", {
    name: "账户用量与读取",
    exact: true,
  });
  await expect(dialog).toContainText("MyDeepSeek");
  await expect(dialog).not.toContainText("另一个计费连接");
  await dialog
    .getByRole("button", { name: "读取真实账户", exact: true })
    .click();
  await expect(
    page.getByRole("dialog", { name: "读取 MyDeepSeek", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => window.providerOverviewFixture.refreshes),
  ).toBe(0);
  await page.getByRole("button", { name: "确认读取", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.providerOverviewFixture.refreshes))
    .toBe(1);
  await page.goto("/tests/frontend/provider-overview.html?mode=unsupported");
  const unsupported = page
    .getByRole("row")
    .filter({ hasText: "另一个计费连接" });
  await expect(unsupported).toContainText("读取能力未接入");
  await expect(unsupported).not.toContainText("读取失败");
});

test("directory and usage errors preserve prior values and unknown forecasts; overview remains usable narrow and dark", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/tests/frontend/provider-overview.html?dark");
  const row = page.getByRole("row").filter({ hasText: "MyDeepSeek" });
  await expect(row).toContainText("8.52 CNY");
  await row.click();
  await page.evaluate(() => {
    window.providerOverviewFixture.directoryFailed = true;
    window.providerOverviewFixture.usageFailed = true;
  });
  await page.getByRole("button", { name: "刷新概览", exact: true }).click();
  await expect(page.getByText("账户目录未更新", { exact: true })).toBeVisible();
  await expect(page.getByText("调用统计未更新", { exact: true })).toBeVisible();
  await expect(row).toContainText("8.52 CNY");
  await expect(row).toContainText("11.93 CNY · 估算 / 18 次未知");
  await expect(row).toContainText("调用未更新 · 上次统计");
  await expect
    .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
    .toBeLessThanOrEqual(390);
  await page.getByRole("tab", { name: "连接与调用", exact: true }).click();
  await expect(page.getByText("已添加 · 2", { exact: true })).toBeVisible();
});

test("list keeps loading distinct from unknown, then publishes the complete aggregate", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-overview.html?mode=usage-loading");
  const row = page.getByRole("row").filter({ hasText: "MyDeepSeek" });
  await expect(row.getByRole("cell").nth(2)).toHaveText("加载中…");
  await expect(row.getByRole("cell").nth(3)).toHaveText("加载中…");
  await expect(row).toContainText("8.52 CNY");
  await page.evaluate(() => window.providerOverviewFixture.finishUsage?.());
  await expect(row).toContainText("11.93 CNY · 估算 / 18 次未知");
  await expect(row).toContainText("11,520,138 · 部分");
});

for (const mode of ["wrong-scope", "source-partial"]) {
  test(`list rejects unreliable aggregate scope: ${mode}`, async ({ page }) => {
    await page.goto(`/tests/frontend/provider-overview.html?mode=${mode}`);
    const row = page.getByRole("row").filter({ hasText: "MyDeepSeek" });
    await expect(row).toContainText("调用范围未确认");
    await expect(row.getByRole("cell").nth(2)).toHaveText("未知");
    await expect(row.getByRole("cell").nth(3)).toHaveText("未知");
    await expect(row).toContainText("8.52 CNY");
  });
}

test("subscription rows preserve unknown remaining quota, true zero and due resets independently", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-overview.html?mode=quota-unknown");
  const row = page.getByRole("row").filter({ hasText: "Codex 订阅" });
  await expect(row).toContainText("5 小时窗口 · 剩余 未知");
  await expect(row).toContainText("重置时间未知");
  await expect(row).toContainText("7 天窗口 · 剩余 0%");
  await expect(row).toContainText("已到重置时间 · 待更新");
  await expect(row).not.toContainText("剩余 20%");
});

test("confirmed empty aggregates show zero calls without inventing an amount currency", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-overview.html?mode=no-calls");
  const row = page.getByRole("row").filter({ hasText: "MyDeepSeek" });
  await expect(row.getByRole("cell").nth(2)).toHaveText("0 · 无调用");
  await expect(row.getByRole("cell").nth(3)).toHaveText("0");
  await expect(row).toContainText("8.52 CNY");
  await expect(row).not.toContainText("0 CNY");
});

test("closing a confirmed account read restores pointer access to other accounts", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-overview.html?mode=unread");
  await page.getByRole("row").filter({ hasText: "MyDeepSeek" }).click();
  await page.getByRole("button", { name: "查看账户与读取用量" }).click();
  const dialog = page.getByRole("dialog", {
    name: "账户用量与读取",
    exact: true,
  });
  await dialog.getByRole("button", { name: "读取真实账户" }).click();
  await page.getByRole("button", { name: "确认读取", exact: true }).click();
  await expect(dialog.getByText("已读取", { exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await page.getByRole("row").filter({ hasText: "Codex 订阅" }).click();
  await expect(
    page.getByRole("region", { name: "Codex 订阅用量概览", exact: true }),
  ).toBeVisible();
});
test("add provider stays in the page header across views in a small window", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/provider-overview.html");
  const add = page.getByRole("button", { name: "添加供应商", exact: true });
  await expect(add).toBeInViewport();
  await page.getByRole("tab", { name: "全部模型", exact: true }).click();
  await expect(add).toBeInViewport();
  await add.focus();
  await page.keyboard.press("Enter");
  await expect(
    page.getByRole("dialog", { name: "添加模型供应商" }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(add).toBeFocused();
});
