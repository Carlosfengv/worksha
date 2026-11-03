import { expect, test } from "@playwright/test";

const destination =
  `/requests/${encodeURIComponent("claude:项目/a:model:response#1")}?` +
  new URLSearchParams({
    source: "native",
    range: "all",
    agent: "unknown",
    descending: "false",
    snapshot: "original-snapshot",
    cursor: "original/cursor+value",
    pluginSlot: "original-slot",
  });
const fixtureUrl = (returnTo: string) =>
  `/tests/frontend/reading-capacity.html?${new URLSearchParams({
    words: "8",
    returnTo,
  })}`;

test("native conversation returns to the exact originating request and frozen filters without another read", async ({
  page,
}) => {
  await page.goto(fixtureUrl(destination));
  await expect(
    page.getByRole("heading", { name: "claude:fixture-0", exact: true }),
  ).toBeVisible();
  const reads = await page.evaluate(() => window.readingCapacity.reads.length);
  const back = page.getByRole("button", { name: "返回来源请求", exact: true });
  await expect(back).toBeVisible();
  await back.focus();
  await back.press("Enter");
  await expect
    .poll(() => page.evaluate(() => location.hash))
    .toBe(`#${destination}`);
  expect(await page.evaluate(() => window.readingCapacity.reads.length)).toBe(
    reads,
  );
});

test("originating request remains reachable when the linked conversation cannot be read", async ({
  page,
}) => {
  await page.goto(fixtureUrl(destination));
  await expect(
    page.getByRole("heading", { name: "claude:fixture-0", exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    window.readingCapacity.error = "busy";
  });
  await page.getByRole("button", { name: "session-1", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "会话内容暂不可用", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "返回来源请求", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => location.hash))
    .toBe(`#${destination}`);
});

test("existing MCP origin keeps its own return action", async ({ page }) => {
  const target = `/mcp/${"a".repeat(64)}?agent=claude-code-cli&days=7`;
  await page.goto(fixtureUrl(target));
  await page
    .getByRole("button", { name: "返回 MCP 服务与调用", exact: true })
    .click();
  await expect
    .poll(() => page.evaluate(() => location.hash))
    .toBe(`#${target}`);
  await expect(
    page.getByRole("button", { name: "返回来源请求", exact: true }),
  ).toHaveCount(0);
});

for (const target of [
  "https://example.invalid/requests/id",
  "//example.invalid/requests/id",
  "javascript:alert(1)",
  "/requests/",
  "/requests/a/b",
  "/requests/%E0%A4%A",
  "/requests/%00",
  "/requests/id#other-route",
]) {
  test(`does not offer an unsupported or malformed request origin: ${target}`, async ({
    page,
  }) => {
    await page.goto(fixtureUrl(target));
    await expect(
      page.getByRole("heading", { name: "claude:fixture-0", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "返回来源请求", exact: true }),
    ).toHaveCount(0);
  });
}
