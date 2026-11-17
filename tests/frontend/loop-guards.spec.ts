import { expect, test } from "@playwright/test";

test("overview shows loop status, links to the session and confirms eligible overrides", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=activity&loopGuards=1#/");
  const section = page.getByRole("region", { name: "本机熔断与预警" });
  await expect(
    section.getByText("连续工具报错", { exact: true }),
  ).toBeVisible();
  await expect(section.getByText("工具去重容量已满")).toBeVisible();
  await expect(
    section.getByText("请发送新用户指令或开启新会话。"),
  ).toBeVisible();
  await expect(section.getByText("预警", { exact: true })).toBeVisible();
  await expect(section.getByRole("button", { name: "放行本轮" })).toHaveCount(
    1,
  );
  const main = section
    .getByRole("listitem")
    .filter({ hasText: "Main binding" });
  await expect(
    main.getByRole("link", { name: "查看会话详情" }),
  ).toHaveAttribute("href", "#/sessions/claude%3Aloop?view=requests");
  await main.getByRole("button", { name: "放行本轮" }).click();
  const dialog = page.getByRole("dialog", { name: "放行本轮" });
  await expect(dialog).toContainText("250,000 Tokens");
  await dialog.getByRole("button", { name: "取消" }).click();
  await expect(main).toBeVisible();
  await main.getByRole("button", { name: "放行本轮" }).click();
  await dialog.getByRole("button", { name: "确认放行" }).click();
  await expect(section.getByRole("status")).toHaveText(
    "已放行本轮。请在原客户端重试。",
  );
  await expect(main).toHaveCount(0);
  await expect(section.getByRole("button", { name: "放行本轮" })).toHaveCount(
    0,
  );
  await section
    .getByRole("listitem")
    .filter({ hasText: "Full binding" })
    .getByRole("link", { name: "查看会话详情" })
    .click();
  await expect(page).toHaveURL(/#\/sessions\/chat%3Afull\?view=requests$/);
});
