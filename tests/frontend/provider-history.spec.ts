import { expect, test } from "@playwright/test";
import type {} from "./tables-harness";

async function openHistory(page: import("@playwright/test").Page, url: string) {
  await page.goto(url);
  await page
    .getByRole("tab", {
      name: url.endsWith("/providers") ? "连接与调用" : "调用与用量",
      exact: true,
    })
    .click();
}

test("costs keep actual zero, estimates and unknown separate, with per-call evidence and model scopes", async ({
  page,
}) => {
  await openHistory(
    page,
    "/tests/frontend/tables.html?mode=provider-history#/providers",
  );
  const list = page.getByRole("region", { name: "供应商连接", exact: true });
  await expect(list).toContainText("累计费用");
  await expect(list).toContainText("0 USD · 实际 / 6.23 USD · 估算 / 1 次未知");
  await page.getByRole("link", { name: "连接 000 详情", exact: true }).click();
  await page.getByRole("tab", { name: "调用与用量", exact: true }).click();
  const history = page.getByRole("region", {
    name: "供应商调用历史",
    exact: true,
  });
  await expect(history.getByLabel("供应商调用汇总")).toContainText(
    "6.23 USD · 估算",
  );
  await history
    .getByRole("button", { name: /^实际费用 / })
    .first()
    .click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("实际费用");
  await expect(dialog).toContainText("0 USD");
  await expect(dialog).toContainText("服务商已报告本次账户扣费");
  await page.keyboard.press("Escape");
  await history
    .getByRole("button", { name: "费用未知，查看原因", exact: true })
    .first()
    .click();
  await expect(dialog).toContainText("费用未知");
  await expect(dialog).toContainText("缓存命中用量未报告");
  await page.keyboard.press("Escape");
  await history
    .getByRole("button", { name: /^估算费用 / })
    .first()
    .click();
  await expect(dialog).toContainText("估算费用");
  await expect(
    dialog.getByRole("table", { name: "本次调用费用明细" }),
  ).toContainText("缓存命中");
  await expect(dialog).toContainText("fixture-price");
  await expect(dialog.getByRole("link", { name: "计费来源" })).toHaveAttribute(
    "href",
    "https://api-docs.deepseek.com/quick_start/pricing/",
  );
  await page.keyboard.press("Escape");
  await history
    .getByRole("button", { name: "下一页调用", exact: true })
    .click();
  await expect(history.getByLabel("供应商调用汇总")).toContainText(
    "6.23 USD · 估算",
  );
  await history
    .getByRole("combobox", { name: "筛选供应商历史模型", exact: true })
    .fill("model-324");
  await page.getByRole("option", { name: "model-324", exact: true }).click();
  await expect(history.getByLabel("供应商调用汇总")).toContainText(
    "0.01 USD · 估算",
  );
  await expect(history.getByLabel("供应商调用汇总")).not.toContainText(
    "6.23 USD",
  );
});

test("provider list keeps narrow overflow inside its table and an unconnected source never reports measured zero", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await openHistory(
    page,
    "/tests/frontend/tables.html?mode=provider-history&sourceDisabled#/providers",
  );
  const region = page.getByRole("region", { name: "供应商连接", exact: true });
  await expect(region).toContainText("6,250");
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth),
  ).toBeLessThanOrEqual(390);
  await page
    .getByRole("combobox", { name: "供应商调用来源", exact: true })
    .click();
  await page.getByRole("option", { name: "原生模型响应", exact: true }).click();
  await expect(
    page.getByText("原生会话来源未接入", { exact: true }),
  ).toBeVisible();
  await expect(
    region.getByRole("row").nth(1).getByText("0", { exact: true }),
  ).toHaveCount(0);
  await expect(page.getByLabel("供应商调用汇总")).not.toContainText("630");
});

test("provider list shows full-history totals, detail keeps totals across call pages and searches every historical model", async ({
  page,
}) => {
  await openHistory(
    page,
    "/tests/frontend/tables.html?mode=provider-history#/providers",
  );
  await expect(
    page.getByRole("region", { name: "供应商连接", exact: true }),
  ).toContainText("625");
  await expect(
    page.getByRole("region", { name: "供应商连接", exact: true }),
  ).toContainText("6,250");
  await expect(
    page.getByRole("link", { name: "未归属调用 · 2 次" }),
  ).toBeVisible();
  await page.getByRole("link", { name: "连接 000 详情", exact: true }).click();
  await page.getByRole("tab", { name: "调用与用量", exact: true }).click();
  const history = page.getByRole("region", {
    name: "供应商调用历史",
    exact: true,
  });
  await expect(history.getByLabel("供应商调用汇总")).toContainText("625");
  await expect(
    page
      .getByRole("region", { name: "供应商调用记录", exact: true })
      .getByRole("row"),
  ).toHaveCount(41);
  await history
    .getByRole("button", { name: "下一页调用", exact: true })
    .click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.scopeQueries
            .filter((q) => q.feature === "provider-usage")
            .at(-1)?.query.cursor,
      ),
    )
    .toBe("40");
  await expect(history.getByLabel("供应商调用汇总")).toContainText("625");
  await history
    .getByRole("combobox", { name: "筛选供应商历史模型", exact: true })
    .fill("model-324");
  await page.getByRole("option", { name: "model-324", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.scopeQueries
            .filter((q) => q.feature === "provider-usage")
            .at(-1)?.query,
      ),
    )
    .toMatchObject({ providerId: "fixture-0", model: "model-324" });
  await expect(
    page
      .getByRole("region", { name: "供应商调用记录", exact: true })
      .getByRole("row"),
  ).toHaveCount(2);
  await expect(
    history.getByText("第 1 页 · 每页 40 条", { exact: true }),
  ).toBeVisible();
  await expect(
    history.getByRole("link", { name: "查看请求", exact: true }),
  ).toHaveAttribute("href", /^#\/requests\/call-0\?range=all&agent=/);
});

test("source, date and independent Agent filters scope totals and unknown calls remain separately inspectable", async ({
  page,
}) => {
  await openHistory(
    page,
    "/tests/frontend/tables.html?mode=provider-history#/providers",
  );
  await page
    .getByRole("combobox", { name: "供应商调用来源", exact: true })
    .click();
  await page.getByRole("option", { name: "原生模型响应", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "供应商连接", exact: true }),
  ).toContainText("200");
  await page
    .getByRole("combobox", { name: "供应商调用日期", exact: true })
    .click();
  await page.getByRole("option", { name: "最近 7 天", exact: true }).click();
  await page
    .getByRole("combobox", { name: "供应商调用 Agent", exact: true })
    .click();
  await page.getByRole("option", { name: "Codex 桌面端", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.scopeQueries
            .filter((q) => q.feature === "provider-usage")
            .at(-1)?.query,
      ),
    )
    .toMatchObject({ source: "native", days: 7, agent: "codex-desktop" });
  await page.getByRole("link", { name: "未归属调用 · 2 次" }).click();
  await expect(
    page.getByRole("heading", { name: "未归属调用", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("region", { name: "供应商调用记录", exact: true }),
  ).toContainText("缺少连接归属证据");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.scopeQueries
            .filter((q) => q.feature === "provider-usage")
            .at(-1)?.query,
      ),
    )
    .toMatchObject({
      providerId: "_unknown",
      source: "native",
      days: 7,
      agent: "codex-desktop",
    });
});

test("failed refresh retains a marked snapshot, and a different provider never receives cached totals", async ({
  page,
}) => {
  await openHistory(
    page,
    "/tests/frontend/tables.html?mode=provider-history#/providers/fixture-0",
  );
  await expect(page.getByLabel("供应商调用汇总")).toContainText("625");
  await page.evaluate(() => {
    window.failProviderUsage = true;
  });
  await page.getByRole("button", { name: "刷新调用统计", exact: true }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "调用统计未更新" }),
  ).toContainText("保留上次快照");
  await expect(page.getByLabel("供应商调用汇总")).toContainText("625");
  await page.evaluate(() => {
    location.hash = "/providers/fixture-1";
  });
  await expect(
    page.getByRole("heading", { name: "连接 001", exact: true }),
  ).toBeVisible();
  await page.getByRole("tab", { name: "调用与用量", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "供应商调用历史", exact: true }),
  ).not.toContainText("625");
  await expect(
    page.getByRole("status").filter({ hasText: "调用统计未更新" }),
  ).toBeVisible();
  await page.evaluate(() => {
    window.failProviderUsage = false;
  });
  await page.getByRole("button", { name: "重试调用统计", exact: true }).click();
  await expect(page.getByLabel("供应商调用汇总")).toContainText("30");
});
