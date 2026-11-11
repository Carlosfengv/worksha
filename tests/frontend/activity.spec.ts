import { test, expect } from "@playwright/test";
import type {} from "./activity-fixture";
import { openAgentHistory } from "./overview-helpers";

test("activity links navigate to the associated Session with mouse and keyboard", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  const trigger = page.getByRole("button", {
    name: /^Claude Code CLI，.*个活动进行中，查看明细$/,
  });
  await trigger.hover();
  const popover = page.getByRole("dialog", { name: "Claude Code CLI活动明细" });
  const first = popover.getByRole("link", { name: /修复登录/ });
  await expect(first).toHaveAttribute("href", "#/sessions/claude%3Aa");
  await first.click();
  await expect(page).toHaveURL(/#\/sessions\/claude%3Aa$/);
  await expect(
    page.getByRole("heading", { name: "修复登录", exact: true }),
  ).toBeVisible();
  await expect(popover).toBeHidden();
  await page.goBack();
  await openAgentHistory(page);
  await trigger.press("Enter");
  await expect(first).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(
    popover.getByRole("link", {
      name: "查看会话上下文：claude:a",
      exact: true,
    }),
  ).toBeFocused();
  await page.keyboard.press("Tab");
  const second = popover.getByRole("link", { name: /整理组件/ });
  await expect(second).toBeFocused();
  await page.waitForTimeout(1800);
  await expect(second).toBeFocused();
  await expect(second).toHaveAttribute("href", "#/sessions/claude%3Ab");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/#\/sessions\/claude%3Ab$/);
  await expect(
    page.getByRole("heading", { name: "整理组件", exact: true }),
  ).toBeVisible();
  await expect(popover).toBeHidden();
  expect(errors).toEqual([]);
});

test("unassociated gateway activity opens its exact request without guessing a Session", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  await page
    .getByRole("button", {
      name: /^Codex CLI，.*个活动进行中，查看明细$/,
    })
    .click();
  const popover = page.getByRole("dialog", { name: "Codex CLI活动明细" });
  const request = popover.getByRole("link", { name: /模型请求 · codex-cl/ });
  await expect(request).toContainText("查看请求");
  await expect(request).toHaveAttribute("href", "#/requests/codex-cli");
  await expect(popover.getByRole("link", { name: /检查服务/ })).toHaveAttribute(
    "href",
    "#/sessions/codex%3Aa",
  );
  await expect(popover.getByRole("link", { name: /更新测试/ })).toHaveAttribute(
    "href",
    "#/sessions/codex%3Ab",
  );
  await request.click();
  await expect(page).toHaveURL(/#\/requests\/codex-cli$/);
  await expect(
    page.getByRole("heading", { name: "请求详情", exact: true }),
  ).toBeVisible();
  await expect(
    page
      .getByRole("region", { name: "请求详情", exact: true })
      .getByRole("listitem")
      .filter({ hasText: "请求 ID" })
      .getByText("codex-cli", { exact: true }),
  ).toBeVisible();
  await expect(popover).toBeHidden();
});

test("native activity opens the selected Session and outside click dismisses the popover", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  const trigger = page.getByRole("button", {
    name: /^Codex 桌面端，.*个活动进行中，查看明细$/,
  });
  await trigger.press("Space");
  const popover = page.getByRole("dialog", { name: "Codex 桌面端活动明细" });
  await expect(popover).toBeVisible();
  await page.getByRole("heading", { name: "总览", exact: true }).click();
  await expect(popover).toBeHidden();
  await trigger.press("Enter");
  const link = popover.getByRole("link", { name: /核对文档/ });
  await expect(link).toHaveAttribute("href", "#/sessions/codex%3Adesktop-b");
  await link.click();
  await expect(
    page.getByRole("heading", { name: "核对文档", exact: true }),
  ).toBeVisible();
  await expect(page).toHaveURL(/#\/sessions\/codex%3Adesktop-b$/);
  await expect(popover).toBeHidden();
});

test("all concurrent activities remain inspectable and keyboard scrolling belongs to the popover", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  await page.evaluate(() => {
    window.activityCase = "many";
  });
  const card = page.getByRole("article", {
    name: "Claude Code CLI实时活动",
    exact: true,
  });
  await expect(card).toContainText("50 个活动进行中");
  await expect(card.locator("dl")).toContainText("24.5 Token/s");
  await card
    .getByRole("button", {
      name: /^Claude Code CLI，.*个活动进行中，查看明细$/,
    })
    .press("Enter");
  const popover = page.getByRole("dialog", {
    name: "Claude Code CLI活动明细",
  });
  await expect(popover.getByRole("listitem")).toHaveCount(50);
  const list = popover.getByLabel("Claude Code CLI活动列表");
  const before = await page.evaluate(
    () => document.scrollingElement?.scrollTop,
  );
  await page.keyboard.press("PageDown");
  await expect.poll(() => list.evaluate((e) => e.scrollTop)).toBeGreaterThan(0);
  expect(await page.evaluate(() => document.scrollingElement?.scrollTop)).toBe(
    before,
  );
  await page.keyboard.press("Escape");
  await expect(popover).toBeHidden();
});

test("parallel activity card averages independent measurements and shows deduplicated activities in a popover", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  const card = page.getByRole("article", {
    name: "Claude Code CLI实时活动",
    exact: true,
  });
  await expect(card).toContainText("2 个活动进行中");
  await expect(card.locator("dl")).toContainText(
    "当前平均文本速度30.0 Token/s",
  );
  await expect(card.locator("dl")).toContainText("当前请求平均 TTFT2.00 秒");
  const trigger = card.getByRole("button", {
    name: /^Claude Code CLI，.*个活动进行中，查看明细$/,
    exact: true,
  });
  await trigger.hover();
  const popover = page.getByRole("dialog");
  await expect(popover).toContainText("修复登录");
  await expect(popover).toContainText("整理组件");
  await expect(popover).toHaveCSS("opacity", "1");
  const activityBounds = await popover.getByRole("link").evaluateAll((links) =>
    links.map((e) => ({
      contentBottom: e.firstElementChild!.getBoundingClientRect().bottom,
      bottom: e.getBoundingClientRect().bottom,
      top: e.getBoundingClientRect().top,
    })),
  );
  for (const row of activityBounds)
    expect(row.contentBottom).toBeLessThanOrEqual(row.bottom);
  expect(activityBounds[1].top).toBeGreaterThanOrEqual(
    activityBounds[0].bottom,
  );
  await expect(popover.getByText("修复登录", { exact: true })).toHaveCount(1);
  await expect(popover).toContainText("速度 2 个样本，TTFT 2 个样本");
  await page.keyboard.press("Escape");
  await expect(popover).toBeHidden();
  await trigger.click();
  await expect(popover).toBeVisible();
  // Regular polling must not dismiss a pinned list or change its membership.
  await page.waitForTimeout(1800);
  await expect(popover).toContainText("整理组件");
  await page.keyboard.press("Escape");
  await expect(popover).toBeHidden();
});

test("Agent attribution, native measurement gaps, real zero and ambiguous links remain explicit", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  const desktop = page.getByRole("article", {
    name: "Claude Code 桌面端实时活动",
    exact: true,
  });
  await expect(desktop).toContainText("正在输出正文");
  await expect(desktop.locator("dl")).toContainText("90.0");
  const native = page.getByRole("article", {
    name: "Codex 桌面端实时活动",
    exact: true,
  });
  await expect(native).toContainText("2 个活动进行中");
  await expect(native.locator("dl")).toContainText("未采集");
  await expect(native.locator("dl")).not.toContainText("999");
  const open = page.getByRole("region", {
    name: "OpenCode · 客户端来源未知实时活动",
    exact: true,
  });
  await expect(open).toContainText("3 个活动进行中");
  await open
    .getByRole("button", {
      name: /^OpenCode · 客户端来源未知，.*个活动进行中，查看明细$/,
    })
    .press("Enter");
  const openDetails = page.getByRole("dialog", {
    name: "OpenCode · 客户端来源未知活动明细",
  });
  await expect(openDetails).toContainText("速度 1 个样本，TTFT 1 个样本");
  const measured = openDetails.getByRole("link", { name: /检查工具/ });
  await expect(measured).toContainText("0.0 Token/s");
  await expect(measured).toContainText("TTFT 0.000 秒");
  for (const client of ["OpenCode CLI", "OpenCode Desktop"]) {
    await expect(
      page.getByRole("article", { name: `${client}实时活动`, exact: true }),
    ).not.toContainText("检查工具");
  }
  await page.keyboard.press("Escape");
  const cli = page.getByRole("article", {
    name: "Codex CLI实时活动",
    exact: true,
  });
  await expect(cli).toContainText("至少 2 个活动进行中");
  await cli
    .getByRole("button", {
      name: /^Codex CLI，.*个活动进行中，查看明细$/,
      exact: true,
    })
    .press("Enter");
  await expect(
    page.getByRole("dialog", { name: "Codex CLI活动明细" }),
  ).toContainText("原生与网关关联未确认，数量为下限");
  await expect(
    page.getByRole("dialog", { name: "Codex CLI活动明细" }),
  ).toContainText("检查服务");
  await expect(
    page.getByRole("dialog", { name: "Codex CLI活动明细" }),
  ).toContainText("更新测试");
});

test("completed and stale snapshots do not claim ongoing activity and recovery refreshes the averages", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  const card = page.getByRole("article", {
    name: "Claude Code CLI实时活动",
    exact: true,
  });
  await expect(card).toContainText("2 个活动进行中");
  await page.evaluate(() => {
    window.activityCase = "error";
  });
  await expect(
    page.getByText("活动采集暂不可用", { exact: false }),
  ).toBeVisible();
  await expect(card.getByRole("button", { name: /个活动进行中/ })).toHaveCount(
    0,
  );
  await expect(card).toContainText("等待更新");
  await page.evaluate(() => {
    window.activityCase = "parallel";
  });
  await expect(card).toContainText("2 个活动进行中");
  await page.evaluate(() => {
    window.activityCase = "completed";
  });
  await expect(card).toContainText("最近文本均速 · 估算");
  await expect(card.locator("dl")).toContainText("35.0");
  await page.evaluate(() => {
    window.activityCase = "stale-native";
  });
  await expect(card).toContainText("请求已完成");
  await expect(card.locator("dl")).toContainText("35.0");
  await expect(card.getByRole("button", { name: /个活动进行中/ })).toHaveCount(
    0,
  );
});

test("activity popover stays within a narrow viewport and remains keyboard readable", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  await page
    .getByRole("button", {
      name: /^Claude Code CLI，.*个活动进行中，查看明细$/,
      exact: true,
    })
    .press("Enter");
  const popover = page.getByRole("dialog");
  await expect(popover).toContainText("修复登录");
  const box = await popover.boundingBox();
  expect(box).not.toBeNull();
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(390);
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth),
  ).toBeLessThanOrEqual(390);
  await page.evaluate(() => {
    window.activityCase = "long";
  });
  const longActivity = popover.getByRole("link", { name: /检查长标题/ });
  await expect(longActivity).toBeVisible();
  expect(
    await popover.evaluate((e) => e.scrollWidth - e.clientWidth),
  ).toBeLessThanOrEqual(1);
  expect(
    await popover
      .getByLabel("Claude Code CLI活动列表")
      .evaluate((e) => e.scrollWidth - e.clientWidth),
  ).toBeLessThanOrEqual(1);
  await page.keyboard.press("Escape");
  await expect(popover).toBeHidden();
});
