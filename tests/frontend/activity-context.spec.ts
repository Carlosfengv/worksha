import { expect, test } from "@playwright/test";
import type {} from "./activity-fixture";
import { openAgentHistory } from "./overview-helpers";

const url = "/tests/frontend/tables.html?mode=activity#/overview";

test("live context waits for its first index and keeps the requested tab", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&conversationFailures=2#/sessions/claude%3Ab?view=context",
  );
  await expect(
    page
      .getByRole("region", { name: "会话内容", exact: true })
      .getByRole("status"),
  ).toHaveText("会话暂未就绪，正在重试…");
  await expect(
    page.getByRole("tab", { name: "上下文", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("region", { name: "会话上下文" })).toContainText(
    "20.0%",
  );
  expect(await page.evaluate(() => window.conversationReads)).toEqual([
    "claude:b",
    "claude:b",
    "claude:b",
  ]);
  expect(await page.evaluate(() => window.conversationRefreshes)).toBe(1);
  await expect(page.getByText("会话读取未完成", { exact: true })).toHaveCount(
    0,
  );
});

test("switching Session cancels an index retry and permission errors never retry", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&conversationFailures=99#/sessions/claude%3Ab?view=context",
  );
  await expect(
    page
      .getByRole("region", { name: "会话内容", exact: true })
      .getByRole("status"),
  ).toHaveText("会话暂未就绪，正在重试…");
  await page.getByRole("button", { name: /修复登录/ }).click();
  await expect(page).toHaveURL(/claude%3Aa/);
  await expect(
    page.getByRole("button", { name: "刷新对话", exact: true }),
  ).toBeEnabled();
  const reads = await page.evaluate(
    () => window.conversationReads.filter((id) => id === "claude:b").length,
  );
  await page.waitForTimeout(1300);
  expect(
    await page.evaluate(
      () => window.conversationReads.filter((id) => id === "claude:b").length,
    ),
  ).toBe(reads);
  await expect(page).toHaveURL(/claude%3Aa/);
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&conversationFailures=99&conversationFailureCode=unauthorized#/sessions/claude%3Ab?view=context",
  );
  await expect(page.getByText("会话读取未完成", { exact: true })).toBeVisible();
  await page.waitForTimeout(1300);
  expect(await page.evaluate(() => window.conversationReads)).toEqual([
    "claude:b",
  ]);
  expect(await page.evaluate(() => window.conversationRefreshes)).toBe(0);
});

test("a missing context stops retrying after a bounded wait", async ({
  page,
}) => {
  await page.clock.install();
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&conversationFailures=99#/sessions/claude%3Ab?view=context",
  );
  await expect(
    page
      .getByRole("region", { name: "会话内容", exact: true })
      .getByRole("status"),
  ).toHaveText("会话暂未就绪，正在重试…");
  for (const [index, delay] of [1000, 2000, 4000, 4000, 4000].entries()) {
    await page.clock.runFor(delay);
    await expect
      .poll(() => page.evaluate(() => window.conversationReads.length))
      .toBe(index + 2);
  }
  await expect(page.getByText("会话读取未完成", { exact: true })).toBeVisible();
  await page.clock.runFor(8000);
  expect(await page.evaluate(() => window.conversationReads.length)).toBe(6);
  await page.evaluate(() => {
    history.replaceState(
      null,
      "",
      location.href.replace(
        "conversationFailures=99",
        "conversationFailures=0",
      ),
    );
  });
  await page.getByRole("button", { name: "刷新对话", exact: true }).click();
  await expect(
    page.getByRole("tab", { name: "上下文", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
});

test("five Agent cards show separate Session context, estimated ratios, observed input and unknown association", async ({
  page,
}) => {
  await page.goto(url);
  await openAgentHistory(page);
  const cli = page.getByLabel("Claude Code CLI会话上下文", { exact: true });
  await expect(cli.getByRole("listitem")).toHaveCount(2);
  await expect(cli).toContainText("修复登录上下文 10.0% · 估算");
  await expect(cli).toContainText("整理组件上下文 20.0% · 估算");
  await expect(cli.getByRole("link").first()).toHaveAttribute(
    "title",
    /最近输入 100 Token · 真实采集/,
  );
  await expect(
    page.getByLabel("Claude Code 桌面端会话上下文", { exact: true }),
  ).toContainText("10.0% · 估算");
  const codex = page.getByLabel("Codex CLI会话上下文", { exact: true });
  await expect(codex).toContainText("0.0% · 估算");
  await expect(codex).toContainText("上下文 未知 · 会话未关联");
  await expect(codex.getByRole("link")).toHaveCount(2);
  await expect(
    page
      .getByLabel("Codex 桌面端会话上下文", { exact: true })
      .getByRole("link"),
  ).toHaveCount(2);
  const opencode = page.getByLabel("OpenCode会话上下文", { exact: true });
  await expect(opencode.getByRole("listitem")).toHaveCount(3);
  await expect(opencode).toContainText("100 Token · 真实采集");
  await expect(opencode).not.toContainText("%");
  await cli
    .getByRole("link", { name: "查看会话上下文：claude:b", exact: true })
    .click();
  await expect(page).toHaveURL(/#\/sessions\/claude%3Ab\?view=context$/);
  await expect(
    page.getByRole("tab", { name: "上下文", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("region", { name: "会话上下文" })).toContainText(
    "20.0%",
  );
  await page.getByRole("button", { name: "刷新对话", exact: true }).click();
  await expect(
    page.getByRole("tab", { name: "上下文", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await page.getByRole("tab", { name: "对话", exact: true }).click();
  await page.getByRole("button", { name: "刷新对话", exact: true }).click();
  await expect(
    page.getByRole("tab", { name: "对话", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
});

test("activity popover context link supports keyboard navigation and direct deep links keep Agent return scope", async ({
  page,
}) => {
  await page.goto(url);
  await openAgentHistory(page);
  const trigger = page.getByRole("button", {
    name: /^Codex 桌面端，.*个活动进行中，查看明细$/,
  });
  await trigger.press("Enter");
  const popover = page.getByRole("dialog", { name: "Codex 桌面端活动明细" });
  await expect(popover.getByRole("link").first()).toBeFocused();
  await page.keyboard.press("Tab");
  const context = popover.getByRole("link", {
    name: "查看会话上下文：codex:desktop-a",
    exact: true,
  });
  await expect(context).toBeFocused();
  await page.waitForTimeout(1800);
  await expect(context).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/codex%3Adesktop-a\?view=context$/);
  await expect(page.getByRole("region", { name: "会话上下文" })).toBeVisible();
  await expect(popover).toBeHidden();
  await page.goto(
    "/tests/frontend/tables.html?mode=activity#/sessions/codex%3Aa?view=context&from=codex-cli&tab=calls",
  );
  await expect(
    page.getByRole("tab", { name: "上下文", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await page
    .getByRole("button", { name: "返回 Agent 调用", exact: true })
    .click();
  await expect(page).toHaveURL(/#\/agents\/codex-cli\?tab=calls$/);
});

test("pending, missing, partial and shared context update without retaining a stale percentage", async ({
  page,
}) => {
  await page.goto(url);
  await openAgentHistory(page);
  const cli = page.getByLabel("Claude Code CLI会话上下文", { exact: true });
  await expect(cli).toContainText("10.0%");
  await page.evaluate(() => {
    window.activityContextState = "pending";
  });
  await expect(cli).not.toContainText("%");
  await expect(cli.getByRole("link").first()).toHaveAttribute(
    "title",
    /等待采集/,
  );
  await page.evaluate(() => {
    window.activityContextState = "partial";
  });
  await expect(cli).toContainText("10.0% · 估算 · 部分采集");
  await page.evaluate(() => {
    window.activityContextState = "missing";
  });
  await expect(cli).not.toContainText("%");
  await expect(cli.getByRole("link").first()).toContainText("上下文 未知");
  await page.evaluate(() => {
    window.activityContextState = "shared";
  });
  const activity = page.getByRole("article", {
    name: "Claude Code CLI实时活动",
    exact: true,
  });
  // The existing message grouping counts the shared Session once while keeping
  // all three gateway requests. Contexts remain one entry per Session.
  await expect(activity).toContainText("2 个活动进行中");
  await expect(activity).toContainText("3 个请求");
  await expect(cli.getByRole("listitem")).toHaveCount(2);
  await expect(
    cli.getByRole("link", { name: "查看会话上下文：claude:a", exact: true }),
  ).toHaveCount(1);
  await page.evaluate(() => {
    window.activityCase = "stale";
  });
  await expect(cli).toHaveCount(0);
});

test("many Session contexts scroll inside the card and fit a narrow window", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(url);
  await openAgentHistory(page);
  await page.evaluate(() => {
    window.activityCase = "many";
  });
  const list = page.getByLabel("Claude Code CLI会话上下文", { exact: true });
  await expect(list.getByRole("link")).toHaveCount(50);
  const size = await list.evaluate((e) => ({
    height: e.clientHeight,
    content: e.scrollHeight,
  }));
  expect(size.content).toBeGreaterThan(size.height);
  await list.getByRole("link").last().focus();
  await expect.poll(() => list.evaluate((e) => e.scrollTop)).toBeGreaterThan(0);
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth),
  ).toBeLessThanOrEqual(390);
});
