import { expect, test } from "@playwright/test";

test("official supplements distinguish JSON Schema, JSON mode and Claude Code context markers", async ({
  page,
}) => {
  await page.clock.install({ time: new Date("2026-10-26T04:00:00Z") });
  await page.goto("/tests/frontend/provider-model-evidence.html");
  await page
    .getByRole("button", { name: "读取全部模型目录", exact: true })
    .click();
  const table = page.getByRole("region", {
    name: "全部 Provider 模型",
    exact: true,
  });
  const row = (protocol: string, id: string) =>
    table
      .getByRole("row")
      .filter({ has: page.getByRole("link", { name: protocol, exact: true }) })
      .filter({ has: page.getByText(id, { exact: true }) });
  await expect(row("chat-completions", "deepseek-flash")).toContainText(
    "工具调用：支持",
  );
  await expect(row("chat-completions", "deepseek-flash")).toContainText(
    "结构化输出：不支持",
  );
  await expect(row("chat-completions", "deepseek-flash")).toContainText(
    "JSON 模式：支持",
  );
  await expect(row("responses", "deepseek-v4-pro")).toContainText(
    "结构化输出：支持",
  );
  await expect(row("messages", "deepseek-v4-pro")).toContainText(
    "JSON 模式：未知",
  );
  for (const id of ["deepseek-flash[1M]", "deepseek-v4-pro[1m]"]) {
    const alias = row("messages", id);
    await expect(alias.getByRole("cell").nth(3)).toContainText(
      "1,000,000 Token",
    );
    await expect(alias).toContainText("Claude Code 窗口；上游 1,048,576 Token");
    await expect(alias).toContainText("工具调用：未知");
    await expect(alias.getByRole("cell").nth(4)).toHaveText("未知");
    await expect(
      alias.getByRole("link", { name: "Claude Code 上下文标记" }),
    ).toHaveAttribute(
      "href",
      "https://code.claude.com/docs/en/model-config#context-window-and-auto-compaction",
    );
    for (const protocol of ["chat-completions", "responses"])
      await expect(row(protocol, id).getByRole("cell").nth(3)).toHaveText(
        "未知",
      );
  }
  await expect(
    row("messages", "unknown[1M]").getByRole("cell").nth(3),
  ).toHaveText("未知");
  await expect(
    row("messages", "deepseek-flash").getByRole("link", { name: "输出格式" }),
  ).toHaveAttribute(
    "href",
    "https://api-docs.deepseek.com/guides/anthropic_api/",
  );
  await page.setViewportSize({ width: 720, height: 800 });
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  const sourceLink = row("messages", "deepseek-flash[1M]").getByRole("link", {
    name: "Claude Code 上下文标记",
  });
  await sourceLink.focus();
  await expect(sourceLink).toBeInViewport();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  // A catalog left open beyond the review period loses only the supplemental values.
  await page.clock.setSystemTime(new Date("2026-11-09T04:00:00Z"));
  await page.getByRole("button", { name: "刷新用量", exact: true }).click();
  await expect(row("chat-completions", "deepseek-flash")).toContainText(
    "工具调用：未知",
  );
  await expect(row("chat-completions", "deepseek-flash")).toContainText(
    "官方资料已过期",
  );
  await expect(
    row("chat-completions", "deepseek-flash").getByRole("cell").nth(3),
  ).toHaveText("1,048,576 Token");
  await expect(
    row("messages", "deepseek-flash[1M]").getByRole("cell").nth(3),
  ).toHaveText("未知");
});
