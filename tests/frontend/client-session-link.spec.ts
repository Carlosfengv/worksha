import { expect, test } from "@playwright/test";
import type {} from "./activity-fixture";
import { openAgentHistory } from "./overview-helpers";

test("OpenCode native tool remains active between completed model requests with its Session context", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  await page.evaluate(() => {
    window.activityCase = "opencode-tool";
  });
  const card = page.getByRole("article", {
    name: "OpenCode实时活动",
    exact: true,
  });
  await expect(card).toContainText("已提出工具调用");
  await expect(card).toContainText("100 Token");
  await card
    .getByRole("button", {
      name: "OpenCode，1 个活动进行中，查看明细",
      exact: true,
    })
    .click();
  const dialog = page.getByRole("dialog", { name: "OpenCode活动明细" });
  await expect(dialog).toContainText("正在执行工具：bash");
  await dialog
    .getByRole("link", { name: /OpenCode 已关联会话.*查看会话/ })
    .click();
  await expect(page).toHaveURL(/opencode%3Ases_verified$/);
  await page.goBack();
  await openAgentHistory(page);
  await card
    .getByRole("link", {
      name: "查看会话上下文：opencode:ses_verified",
      exact: true,
    })
    .click();
  await expect(page).toHaveURL(/opencode%3Ases_verified\?view=context$/);
  await expect(
    page.getByRole("tab", { name: "上下文", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
});

test("explicit Claude CLI and ambiguous SDK keep separate activities and context targets", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  await page.evaluate(() => {
    window.activityCase = "client-session-links";
  });
  const cli = page.getByRole("article", {
    name: "Claude Code CLI实时活动",
    exact: true,
  });
  const unknown = page.getByRole("region", {
    name: "Claude · 客户端来源未知实时活动",
    exact: true,
  });
  await expect(cli).toContainText("交互式 Claude 任务");
  await expect(cli).not.toContainText("SDK 宿主未确认");
  await expect(unknown).toContainText("SDK 宿主未确认");
  await unknown
    .getByRole("button", { name: /个活动进行中，查看明细$/ })
    .click();
  const popover = page.getByRole("dialog", {
    name: "Claude · 客户端来源未知活动明细",
  });
  await expect(
    popover.getByRole("link", { name: /SDK 宿主未确认/ }).locator("[title]"),
  ).toHaveAttribute("title", /SDK 模式未报告最终客户端/);
  await page.keyboard.press("Escape");
  for (const [region, id] of [
    [cli, "claude:verified-a"],
    [unknown, "claude:sdk-b"],
  ] as const) {
    await region
      .getByRole("link", { name: `查看会话上下文：${id}`, exact: true })
      .click();
    await expect(page).toHaveURL(
      new RegExp(`${encodeURIComponent(id)}\\?view=context$`),
    );
    await expect(
      page.getByRole("tab", { name: "上下文", exact: true }),
    ).toHaveAttribute("aria-selected", "true");
    await page.goBack();
    await openAgentHistory(page);
  }
});

test("OpenCode explicit Session link provides context and conversation without duplicating idle native state", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await openAgentHistory(page);
  await page.evaluate(() => {
    window.activityCase = "client-session-links";
  });
  const card = page.getByRole("article", {
    name: "OpenCode实时活动",
    exact: true,
  });
  await expect(card).toContainText("OpenCode 已关联会话");
  await expect(card).toContainText("100 Token");
  await expect(card).not.toContainText("会话未关联");
  await card
    .getByRole("button", {
      name: "OpenCode，1 个活动进行中，查看明细",
      exact: true,
    })
    .click();
  const popover = page.getByRole("dialog", { name: "OpenCode活动明细" });
  await popover
    .getByRole("link", { name: /OpenCode 已关联会话.*查看会话/ })
    .click();
  await expect(page).toHaveURL(/opencode%3Ases_verified$/);
  await expect(
    page.getByRole("tab", { name: "对话", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await page.goBack();
  await openAgentHistory(page);
  await card
    .getByRole("link", {
      name: "查看会话上下文：opencode:ses_verified",
      exact: true,
    })
    .click();
  await expect(page).toHaveURL(/opencode%3Ases_verified\?view=context$/);
  await expect(
    page.getByRole("tab", { name: "上下文", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("region", { name: "会话上下文" })).toContainText(
    "100 Token",
  );
});
