import { expect, test } from "@playwright/test";
import type {} from "./render-boundary-harness";

test("render failure preserves navigation and retries without writes or private diagnostics", async ({
  page,
}) => {
  const diagnostics: string[] = [];
  page.on("console", (event) => diagnostics.push(event.text()));
  page.on("pageerror", (error) => diagnostics.push(error.message));
  await page.goto("/tests/frontend/render-boundary.html#/objects/a");
  await expect(page.getByRole("region", { name: "页面恢复" })).toBeVisible();
  await expect(
    page.getByRole("navigation", { name: "夹具导航" }),
  ).toBeVisible();
  await expect(page.locator("body")).not.toContainText("fixture-session-body");
  await page.evaluate(() => {
    window.recoveryFixture.crash = false;
  });
  await page.getByRole("button", { name: "重试页面", exact: true }).click();
  await expect(page.getByText("页面已恢复", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => window.recoveryFixture.writes)).toBe(0);
  expect(diagnostics.some((d) => d.includes("caught-render-error"))).toBe(true);
  expect(diagnostics.join("\n")).not.toContain("do-not-log");
});

test("object navigation clears a failed boundary while healthy updates preserve drafts", async ({
  page,
}) => {
  await page.goto("/tests/frontend/render-boundary.html#/objects/a");
  await expect(page.getByRole("region", { name: "页面恢复" })).toBeVisible();
  await page.evaluate(() => {
    window.recoveryFixture.crash = false;
  });
  await page.getByRole("link", { name: "对象 B" }).click();
  await page.getByLabel("草稿", { exact: true }).fill("keep draft");
  await page.getByRole("button", { name: /刷新夹具/ }).click();
  await page.getByRole("link", { name: "对象 A" }).click();
  await expect(page.getByLabel("草稿", { exact: true })).toHaveValue(
    "keep draft",
  );
});

test("lazy rejection gets a fresh loader only on retry, then retains the successful page", async ({
  page,
}) => {
  await page.goto("/tests/frontend/render-boundary.html?lazy#/objects/a");
  await expect(page.getByRole("region", { name: "页面恢复" })).toBeVisible();
  expect(await page.evaluate(() => window.recoveryFixture.loads)).toBe(1);
  await page.evaluate(() => {
    window.recoveryFixture.crash = false;
    window.recoveryFixture.loadFails = false;
  });
  await page.getByRole("button", { name: "重试页面", exact: true }).click();
  await expect(page.getByText("页面已恢复", { exact: true })).toBeVisible();
  await page.getByLabel("草稿", { exact: true }).fill("loaded draft");
  await page.getByRole("button", { name: /刷新夹具/ }).click();
  await expect(page.getByLabel("草稿", { exact: true })).toHaveValue(
    "loaded draft",
  );
  expect(await page.evaluate(() => window.recoveryFixture.loads)).toBe(2);
  expect(await page.evaluate(() => window.recoveryFixture.writes)).toBe(0);
});

test("root fallback recovers the application shell without replaying mutations", async ({
  page,
}) => {
  await page.goto("/tests/frontend/render-boundary.html?root");
  await expect(page.getByRole("region", { name: "应用恢复" })).toBeVisible();
  await page.evaluate(() => {
    window.recoveryFixture.rootCrash = false;
    window.recoveryFixture.crash = false;
  });
  await page.getByRole("button", { name: "重新打开界面", exact: true }).click();
  await expect(
    page.getByRole("navigation", { name: "夹具导航" }),
  ).toBeVisible();
  await expect(page.getByText("页面已恢复", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => window.recoveryFixture.writes)).toBe(0);
});

test("real workspace catches a page render crash and keeps the original navigation", async ({
  page,
}) => {
  await page.route("**/src/features/settings/page.tsx*", (route) =>
    route.fulfill({
      contentType: "application/javascript",
      body: 'export function SettingsPage() { if (window.recoveryFixture.crash) throw Error("fixture-private-session-do-not-log"); return "设置页面已恢复"; }',
    }),
  );
  await page.goto("/tests/frontend/render-boundary.html?app#/settings");
  await expect(page.getByRole("region", { name: "页面恢复" })).toBeVisible();
  await expect(
    page.getByRole("navigation", { name: "主导航", exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    window.recoveryFixture.crash = false;
  });
  await page.getByRole("button", { name: "重试页面", exact: true }).click();
  await expect(page.getByText("设置页面已恢复", { exact: true })).toBeVisible();
  await page.getByRole("link", { name: "模型供应商", exact: true }).click();
  await expect(page).toHaveURL(/#\/providers$/);
  await expect(page.getByRole("region", { name: "页面恢复" })).toHaveCount(0);
});

test("failed page module download offers a full application reload that fetches it again", async ({
  page,
}) => {
  await page.route("**/src/features/settings/page.tsx*", (route) =>
    route.abort(),
  );
  await page.goto("/tests/frontend/render-boundary.html?app#/settings");
  await expect(page.getByRole("region", { name: "页面恢复" })).toBeVisible();
  await expect(
    page.getByRole("navigation", { name: "主导航", exact: true }),
  ).toBeVisible();
  await page.unroute("**/src/features/settings/page.tsx*");
  await page.getByRole("button", { name: "重新加载应用", exact: true }).click();
  await expect(page.getByText("设置暂不可用", { exact: true })).toBeVisible();
  await expect(page.getByRole("region", { name: "页面恢复" })).toHaveCount(0);
  expect(await page.evaluate(() => window.recoveryFixture.writes)).toBe(0);
});
