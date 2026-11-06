import { expect, test } from "@playwright/test";

test("空会话显示提示，只有工具记录时保留记录与输入框", async ({ page }) => {
  await page.goto("/tests/frontend/session-thread.html");
  await page.getByRole("button", { name: "清空消息" }).click();
  await expect(
    page.getByText("告诉 OpenCode 你想完成的任务。", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "仅工具记录" }).click();
  await expect(
    page.getByText("已停止本轮", { exact: true }).filter({ visible: true }),
  ).toBeVisible();
  await expect(page.getByRole("textbox", { name: "草稿" })).toBeVisible();
});

test("history and streaming text remain visible when animation frames are paused", async ({
  page,
}) => {
  await page.addInitScript(() => {
    window.requestAnimationFrame = () => 1;
    const animate = Element.prototype.animate;
    Element.prototype.animate = function (...args) {
      const animation = animate.apply(this, args);
      animation.pause();
      return animation;
    };
  });
  await page.goto("/tests/frontend/session-thread.html");
  const messages = page.locator('[data-slot="chat-message"]');
  await expect(messages).toHaveCount(2);
  for (const message of await messages.all()) {
    await expect(message).toHaveCSS("opacity", "1");
    await expect(message).toBeInViewport();
  }
  await expect(page.getByText("历史回复", { exact: true })).toBeVisible();
  const editor = page.getByRole("textbox", { name: "草稿" });
  await editor.fill("保留未发送草稿");
  await page.getByRole("button", { name: "追加一轮" }).click();
  await expect(messages).toHaveCount(4);
  await expect(page.getByText("首段回复", { exact: true })).toBeVisible();
  for (const message of await messages.all()) {
    await expect(message).toHaveCSS("opacity", "1");
  }
  const question = await page
    .getByText("新问题", { exact: true })
    .boundingBox();
  const answer = await page
    .getByText("首段回复", { exact: true })
    .boundingBox();
  expect(question).not.toBeNull();
  expect(answer).not.toBeNull();
  expect(answer!.y).toBeGreaterThanOrEqual(question!.y + question!.height);
  await page.getByRole("button", { name: "完成本轮" }).click();
  await expect(
    page.getByText("首段回复，追加内容", { exact: true }),
  ).toBeVisible();
  await expect(messages.last()).toHaveCSS("opacity", "1");
  await expect(editor).toHaveValue("保留未发送草稿");
});
