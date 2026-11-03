import { expect, test } from "@playwright/test";
import type {} from "./activity-fixture";
import { openAgentHistory } from "./overview-helpers";

test("unknown Codex progress remains visible with separate context and correct Session targets", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  await page.evaluate(() => {
    window.activityCase = "unknown-live";
  });
  const region = page.getByRole("region", {
    name: "Codex · 客户端来源未知实时活动",
    exact: true,
  });
  await expect(region).toBeVisible();
  await expect(region).toContainText("持续检查真实进度");
  await expect(region).toContainText("继续处理项目");
  await expect(region).toContainText("10.0% · 估算");
  await expect(region).toContainText("20.0% · 估算");
  await expect(region).toContainText("会话未关联");
  await expect(
    page.getByRole("article", { name: "Codex 桌面端实时活动", exact: true }),
  ).not.toContainText("持续检查真实进度");
  await region
    .getByRole("link", { name: "查看会话上下文：codex:unknown-a", exact: true })
    .click();
  await expect(page).toHaveURL(/codex%3Aunknown-a\?view=context$/);
  await expect(
    page.getByRole("tab", { name: "上下文", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("region", { name: "会话上下文" })).toContainText(
    "10.0%",
  );
  await page.goBack();
  await openAgentHistory(page);
  await region
    .getByRole("button", { name: /个活动进行中，查看明细$/ })
    .press("Enter");
  const popover = page.getByRole("dialog", {
    name: "Codex · 客户端来源未知活动明细",
  });
  await expect(
    popover.getByRole("link", { name: /继续处理项目/ }),
  ).toHaveAttribute("href", "#/sessions/codex%3Aunknown-b");
  await expect(
    popover.getByRole("link", { name: /继续处理项目/ }).locator("[title]"),
  ).toHaveAttribute("title", /缺少逐次客户端证据/);
});

test("reliable client evidence moves only its activity; completion and stale snapshots clear unknown live rows", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  await page.evaluate(() => {
    window.activityCase = "unknown-live";
  });
  const region = page.getByRole("region", {
    name: "Codex · 客户端来源未知实时活动",
    exact: true,
  });
  await expect(region).toContainText("持续检查真实进度");
  await page.evaluate(() => {
    window.activityCase = "unknown-confirmed";
  });
  await expect(region).not.toContainText("持续检查真实进度");
  await expect(region).toContainText("继续处理项目");
  await expect(
    page.getByRole("article", { name: "Codex 桌面端实时活动", exact: true }),
  ).toContainText("持续检查真实进度");
  await page
    .getByRole("article", { name: "Codex 桌面端实时活动", exact: true })
    .getByRole("button", { name: /个活动进行中，查看明细$/ })
    .click();
  await expect(
    page
      .getByRole("dialog", { name: "Codex 桌面端活动明细" })
      .getByRole("link", { name: /持续检查真实进度/ })
      .locator("[title]"),
  ).toHaveAttribute("title", /桌面本机执行日志/);
  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    window.activityCase = "error";
  });
  await expect(region).toHaveCount(0);
  await page.evaluate(() => {
    window.activityCase = "unknown-live";
  });
  await expect(region).toContainText("继续处理项目");
  await page.evaluate(() => {
    window.activityCase = "completed";
  });
  await expect(region).toHaveCount(0);
});

test("unknown filter and narrow layout retain live Session context without inventing an Agent", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  await page.evaluate(() => {
    window.activityCase = "unknown-live";
  });
  await page.getByRole("combobox", { name: "首页筛选 Agent" }).click();
  await page
    .getByRole("option", { name: "客户端来源未知", exact: true })
    .click();
  await expect(
    page.getByRole("article", { name: "Codex 桌面端实时活动", exact: true }),
  ).toHaveCount(0);
  const region = page.getByRole("region", {
    name: "Codex · 客户端来源未知实时活动",
    exact: true,
  });
  await expect(region).toContainText("继续处理项目");
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth),
  ).toBeLessThanOrEqual(390);
});

for (const mode of ["exec", "tui"] as const) {
  test(`simultaneous ${mode} CLI and desktop activities keep separate cards, evidence and Session context targets`, async ({
    page,
  }) => {
    await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
    await openAgentHistory(page);
    await page.evaluate((mode) => {
      window.activityCase = mode === "tui" ? "tui-clients" : "dual-clients";
    }, mode);
    const unknown = page.getByRole("region", {
      name: "Codex · 客户端来源未知实时活动",
      exact: true,
    });
    await expect(unknown).toContainText("未确认客户端");
    for (const [agent, title, id, ratio, evidence] of [
      [
        "Codex CLI",
        "CLI 只读验证",
        "live-cli-a",
        "10.0%",
        mode === "tui" ? "CLI 本机执行日志" : "原生 CLI 本轮标记",
      ],
      [
        "Codex 桌面端",
        "桌面并行处理",
        "live-desktop-b",
        "20.0%",
        "桌面本机执行日志",
      ],
    ]) {
      const card = page.getByRole("article", {
        name: `${agent}实时活动`,
        exact: true,
      });
      await expect(card).toContainText(title);
      await expect(card).not.toContainText("未确认客户端");
      await expect(unknown).not.toContainText(title);
      await card
        .getByRole("button", { name: /个活动进行中，查看明细$/ })
        .click();
      const popover = page.getByRole("dialog", { name: `${agent}活动明细` });
      const activity = popover.getByRole("link", { name: new RegExp(title) });
      await expect(activity.locator("[title]")).toHaveAttribute(
        "title",
        new RegExp(evidence),
      );
      await activity.click();
      await expect(page).toHaveURL(new RegExp(`codex%3A${id}$`));
      await expect(
        page.getByRole("tab", { name: "对话", exact: true }),
      ).toHaveAttribute("aria-selected", "true");
      await page.goBack();
      await openAgentHistory(page);
      await card
        .getByRole("link", { name: `查看会话上下文：codex:${id}`, exact: true })
        .click();
      await expect(page).toHaveURL(new RegExp(`codex%3A${id}\\?view=context$`));
      await expect(
        page.getByRole("tab", { name: "上下文", exact: true }),
      ).toHaveAttribute("aria-selected", "true");
      await expect(
        page.getByRole("region", { name: "会话上下文" }),
      ).toContainText(ratio);
      await page.goBack();
      await openAgentHistory(page);
    }
  });
}
