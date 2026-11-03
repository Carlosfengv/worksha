import { expect, type Page } from "@playwright/test";

export async function openAgentHistory(page: Page) {
  const trigger = page.getByRole("button", {
    name: "Agent 连接与历史指标",
    exact: true,
  });
  await expect(trigger).toBeVisible();
  if ((await trigger.getAttribute("aria-expanded")) !== "true")
    await trigger.click();
}
