import { expect, test } from "@playwright/test";
import type {} from "./activity-fixture";
import { openAgentHistory } from "./overview-helpers";

const url =
  "/tests/frontend/tables.html?mode=activity&connection=official#/overview";

test("gateway idle cards retain friendly historical models and do not expose configuration aliases", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&activity=completed#/overview",
  );
  await openAgentHistory(page);
  for (const name of ["Claude Code 桌面端", "OpenCode"]) {
    const card = page.getByRole("article", {
      name: `${name}实时活动`,
      exact: true,
    });
    await expect(card).toContainText("最近网关 · MyDeepSeek · deepseek-flash");
    await expect(card).not.toContainText("local-agent-");
    await expect(card).not.toContainText("local_agent_");
    await expect(card.locator("dl")).toContainText("35.0 Token/s");
  }
});

test("idle official cards show the configured default, keeping historical gateway timings explicitly separate", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&connection=official&activity=completed#/overview",
  );
  await openAgentHistory(page);
  for (const name of [
    "Claude Code CLI",
    "Claude Code 桌面端",
    "Codex CLI",
    "Codex 桌面端",
    "OpenCode",
  ]) {
    const card = page.getByRole("article", {
      name: `${name}实时活动`,
      exact: true,
    });
    await expect(card).toContainText("暂无进行中会话");
    await expect(card).toContainText("官方登录 · 默认");
    await expect(card).toContainText(
      `默认模型 · ${name.startsWith("Codex") ? "gpt-6.1-sol" : "claude-sonnet-5"}`,
    );
    await expect(card).not.toContainText("MyDeepSeek");
    const time = card.locator("time");
    if (name !== "Codex 桌面端") {
      await expect(card).toContainText("最近请求 TTFT");
      await expect(time).toHaveAttribute("title", /最近网关请求 · MyDeepSeek/);
      await expect(time).toHaveText(/\d+ (秒|分钟|小时)前请求/);
    }
  }
});

test("fresh gateway traffic overrides the activity model without rewriting the official default label", async ({
  page,
}) => {
  await page.goto(url);
  await openAgentHistory(page);
  const card = page.getByRole("article", {
    name: "Claude Code CLI实时活动",
    exact: true,
  });
  await expect(card).toContainText("官方登录 · 默认");
  await expect(card).toContainText("网关正在代理 · 2 个请求");
  await expect(card).toContainText("MyDeepSeek · deepseek-flash");
  await expect(card).not.toContainText("默认模型 · claude-sonnet-5");
  await expect(card.locator("dl")).toContainText("30.0 Token/s");
});

test("activity cards preserve their order and contain long content as membership changes", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto(url);
  await openAgentHistory(page);
  const cards = page.getByRole("article", { name: /实时活动$/ });
  await expect(cards).toHaveCount(6);
  const order = () => cards.getByRole("heading").allTextContents();
  const contained = () =>
    cards.evaluateAll((elements) =>
      elements
        .filter((e) => e.scrollWidth > e.clientWidth + 1)
        .map((e) => ({
          label: e.getAttribute("aria-label"),
          width: e.clientWidth,
          scroll: e.scrollWidth,
        })),
    );
  const before = await order();
  for (const mode of [
    "completed",
    "many",
    "long",
    "unknown-live",
    "parallel",
  ] as const) {
    const calls = await page.evaluate((mode) => {
      window.activityCase = mode;
      return window.overviewReads.length;
    }, mode);
    await expect
      .poll(() => page.evaluate(() => window.overviewReads.length))
      .toBeGreaterThan(calls);
    await expect.poll(order).toEqual(before);
    await expect.poll(contained).toEqual([]);
  }
});

test("polling preserves the context order, focus and inner scroll even when event recency reverses", async ({
  page,
}) => {
  await page.goto(url);
  await openAgentHistory(page);
  await page.evaluate(() => {
    window.activityCase = "many";
  });
  const list = page.getByLabel("Claude Code CLI会话上下文", { exact: true });
  await expect(list.getByRole("link")).toHaveCount(50);
  const hrefs = await list
    .getByRole("link")
    .evaluateAll((elements) => elements.map((e) => e.getAttribute("href")));
  const link = list.getByRole("link").last();
  await link.focus();
  const before = await list.evaluate((e) => e.scrollTop);
  expect(before).toBeGreaterThan(0);
  await page.evaluate(() => {
    window.activityReverse = true;
  });
  await page.waitForTimeout(1800);
  await expect(link).toBeFocused();
  expect(await list.evaluate((e) => e.scrollTop)).toBe(before);
  expect(
    await list
      .getByRole("link")
      .evaluateAll((elements) => elements.map((e) => e.getAttribute("href"))),
  ).toEqual(hrefs);
});

test("changing analysis controls keeps activity and charts mounted until the matching statistics arrive", async ({
  page,
}) => {
  await page.goto(url);
  await openAgentHistory(page);
  const card = page.getByRole("article", {
    name: "Claude Code CLI实时活动",
    exact: true,
  });
  await expect(card).toContainText("2 个活动进行中");
  await page.evaluate(() => {
    window.overviewDelay = 3000;
  });
  await page.getByRole("tab", { name: "Token 用量", exact: true }).click();
  const analysis = page.getByRole("region", {
    name: "调用趋势与占比",
    exact: true,
  });
  await expect(analysis).toHaveAttribute("aria-busy", "true");
  await expect(analysis).toContainText("正在更新统计，保留上次结果");
  await expect(card).toContainText("2 个活动进行中");
  await expect(page.getByText("正在读取调用分析", { exact: true })).toHaveCount(
    0,
  );
  await expect(analysis).toHaveAttribute("aria-busy", "false", {
    timeout: 6000,
  });
  await expect(
    analysis.getByRole("heading", { name: "模型占比 · Token 用量" }),
  ).toBeVisible();
});

test("hidden overview pauses polling and immediately refreshes on return without overlapping slow reads", async ({
  page,
}) => {
  await page.clock.install();
  await page.goto(url);
  await openAgentHistory(page);
  await expect(
    page.getByRole("article", { name: "Claude Code CLI实时活动", exact: true }),
  ).toContainText("2 个活动进行中");
  const initial = await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "hidden",
    });
    document.dispatchEvent(new Event("visibilitychange"));
    return window.overviewReads.length;
  });
  await page.clock.fastForward(15000);
  expect(await page.evaluate(() => window.overviewReads.length)).toBe(initial);
  await page.evaluate(() => {
    window.overviewDelay = 5000;
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "visible",
    });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await expect
    .poll(() => page.evaluate(() => window.overviewReads.length))
    .toBe(initial + 1);
  await page.clock.fastForward(4500);
  expect(await page.evaluate(() => window.overviewReads.length)).toBe(
    initial + 1,
  );
});

test("a failed analysis change keeps the prior scope labeled and recovery publishes the requested metric immediately", async ({
  page,
}) => {
  await page.goto(url);
  await openAgentHistory(page);
  const analysis = page.getByRole("region", {
    name: "调用趋势与占比",
    exact: true,
  });
  await expect(
    analysis.getByRole("heading", { name: "模型占比 · 调用次数" }),
  ).toBeVisible();
  await page.evaluate(() => {
    window.activityCase = "error";
  });
  await page.getByRole("tab", { name: "Token 用量", exact: true }).click();
  await expect(analysis).toContainText("统计更新失败，保留上次结果");
  await expect(
    analysis.getByRole("heading", { name: "模型占比 · 调用次数" }),
  ).toBeVisible();
  await page.evaluate(() => {
    window.activityCase = "parallel";
  });
  await expect(analysis).toHaveAttribute("aria-busy", "false");
  await expect(
    analysis.getByRole("heading", { name: "模型占比 · Token 用量" }),
  ).toBeVisible();
});

test("a slow previous Agent response cannot replace the selected Agent or its analysis", async ({
  page,
}) => {
  await page.goto(url);
  await openAgentHistory(page);
  await expect(
    page.getByRole("article", { name: "Claude Code CLI实时活动", exact: true }),
  ).toContainText("2 个活动进行中");
  await page.evaluate(() => {
    window.overviewHeldAgent = "claude-code-cli";
  });
  const select = page.getByRole("combobox", { name: "首页筛选 Agent" });
  await select.click();
  await page
    .getByRole("option", { name: "Claude Code CLI", exact: true })
    .click();
  await expect
    .poll(() => page.evaluate(() => window.overviewReads.at(-1)?.agent))
    .toBe("claude-code-cli");
  await expect
    .poll(() => page.evaluate(() => typeof window.overviewRelease))
    .toBe("function");
  await expect(page.getByRole("listbox", { includeHidden: true })).toBeHidden();
  await select.click();
  await page.getByRole("option", { name: "OpenCode", exact: true }).click();
  const card = page.getByRole("article", {
    name: "OpenCode实时活动",
    exact: true,
  });
  await expect(card).toContainText("3 个活动进行中");
  await page.evaluate(async () => {
    window.overviewHeldAgent = undefined;
    window.overviewRelease?.();
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
    );
  });
  await expect
    .poll(() => page.evaluate(() => window.overviewHeldDone))
    .toBe(true);
  await expect(page.getByRole("article")).toHaveCount(1);
  await expect(card).toContainText("3 个活动进行中");
  await expect(
    page.getByRole("region", { name: "调用趋势与占比", exact: true }),
  ).toHaveAttribute("aria-busy", "false");
});
