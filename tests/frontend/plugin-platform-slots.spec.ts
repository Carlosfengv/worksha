import { expect, test } from "@playwright/test";
import type {} from "./fixtures/tables/plugin-platform";

const fixture = "/tests/frontend/tables.html?mode=workspace&platform";
const board = "#/extensions/org.example.platform-fixture/board";
const archive = "#/extensions/org.example.platform-fixture/archive";

test("保存草稿保留其他标签移动，同字段冲突保留草稿", async ({ page }) => {
  await page.goto(fixture + board);
  await page
    .locator('[data-slot="card"]')
    .filter({ has: page.getByText("跨页卡片", { exact: true }) })
    .getByRole("button", { name: "编辑" })
    .click();
  const dialog = page.getByRole("dialog", { name: "编辑记录" });
  await dialog.getByLabel("标题").fill("草稿改名");
  await page.evaluate(() => window.platformPatchCard({ state: "完成" }));
  await dialog.getByRole("button", { name: "保存" }).click();
  await expect(dialog).toHaveCount(0);
  const completed = page.getByRole("region", { name: "完成", exact: true });
  await expect(completed.getByText("草稿改名")).toBeVisible();
  await completed.getByRole("button", { name: "编辑" }).click();
  await dialog.getByLabel("标题").fill("第二个草稿");
  await page.evaluate(() => window.platformPatchCard({ title: "别处改名" }));
  await dialog.getByRole("button", { name: "保存" }).click();
  await expect(dialog.getByRole("alert")).toHaveText(/字段已在别处更新/);
  await expect(dialog.getByLabel("标题")).toHaveValue("第二个草稿");
});

test("插件入口归入现有分组并可搜索，工具栏保持紧凑", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 860 });
  await page.goto(fixture + "#/requests?range=all");
  await expect(
    page.getByRole("navigation", { name: "扩展", exact: true }),
  ).toHaveCount(0);
  const observation = page.getByRole("navigation", {
    name: "观测",
    exact: true,
  });
  await expect(
    observation.getByRole("link", { name: "测试任务看板" }),
  ).toBeVisible();
  await expect(
    page
      .getByRole("navigation", { name: "供给与网关" })
      .getByRole("link", { name: "测试归档" }),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: "查看归档" })).toBeVisible();
  const toolbar = (await page.locator("[data-request-toolbar]").boundingBox())!;
  expect(toolbar.y + toolbar.height).toBeLessThanOrEqual(172);
  await observation
    .getByRole("link", { name: "会话列表", exact: true })
    .click();
  await expect(page.getByRole("link", { name: "查看归档" })).toBeVisible();
  const sessions = (await page
    .locator("[data-session-toolbar]")
    .boundingBox())!;
  expect(sessions.y + sessions.height).toBeLessThanOrEqual(172);
  await page.keyboard.press("Meta+k");
  await page.getByPlaceholder("搜索标签与插件入口…").fill("测试任务看板");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(new RegExp(board + "$"));
  const directoryCard = page.getByText("目录来源卡片", { exact: true });
  await expect(directoryCard).toBeVisible();
  await expect(
    page.getByText("fixture-cursor-model", { exact: true }),
  ).toBeVisible();
  await expect(
    page
      .getByRole("link", { name: "打开关联会话" })
      .and(page.locator('a[data-workspace-title="目录会话"]')),
  ).toHaveAttribute(
    "href",
    "#/sessions/org.example.reader%3Aimported?scope=unified&source=org.example.reader%3Asource",
  );
});

test("会话、项目和全宽看板同步，保护草稿并停止后台轮询", async ({ page }) => {
  test.setTimeout(45000);
  await page.goto(fixture + board);
  await expect(page.getByText("跨页卡片", { exact: true })).toBeVisible();
  await page
    .getByRole("link", { name: "打开关联会话" })
    .and(page.locator('a[data-workspace-title="修复登录"]'))
    .click();
  const inspector = page.getByRole("complementary", { name: "会话检视栏" });
  await expect(inspector.getByText("生成中")).toBeVisible();
  await inspector.getByLabel("移动 跨页卡片").click();
  await page.getByRole("option", { name: "进行中" }).click();
  await expect(inspector.getByLabel("移动 跨页卡片")).toHaveText(/进行中/);
  const sessionHash = new URL(page.url()).hash;
  await page.evaluate(() => {
    location.hash = "#/projects/fixture-project";
  });
  await expect(page.getByRole("tab", { name: "项目看板" })).toBeVisible();
  await expect(
    page
      .getByRole("region", { name: "进行中", exact: true })
      .getByText("跨页卡片"),
  ).toBeVisible();
  await page.getByRole("combobox", { name: "移动 跨页卡片" }).click();
  await page.getByRole("option", { name: "完成" }).click();
  await expect(
    page
      .getByRole("region", { name: "完成", exact: true })
      .getByText("跨页卡片"),
  ).toBeVisible();
  await page.getByRole("button", { name: /固定标签/ }).click();
  await page.evaluate((route) => {
    location.hash = route;
  }, archive);
  await expect(page.getByRole("heading", { name: "测试归档" })).toBeVisible();
  await expect(page.locator("[data-workspace-tab]")).toHaveCount(4);
  await page.keyboard.press("Meta+1");
  const background = ["board", "session-slot", "project-slot", "archive"];
  const counts = () =>
    page.evaluate(
      (keys) => keys.map((key) => window.platformReads[key]),
      background,
    );
  const before = await counts();
  expect(before.every((value) => typeof value === "number" && value > 0)).toBe(
    true,
  );
  await page.waitForTimeout(12000);
  expect(await counts()).toEqual(before);
  await page.getByRole("link", { name: "测试任务看板" }).click();
  await expect(
    page
      .getByRole("region", { name: "完成", exact: true })
      .getByText("跨页卡片"),
  ).toBeVisible();
  await page.getByRole("button", { name: "新建记录", exact: true }).click();
  await page.getByLabel("标题").fill("未保存草稿");
  await page.keyboard.press("Meta+w");
  const guard = page.getByRole("dialog", { name: "标签含有未保存修改" });
  await expect(guard).toBeVisible();
  await guard.getByRole("button", { name: "放弃未保存修改" }).click();
  await page.evaluate((hash) => {
    location.hash = hash;
  }, sessionHash);
  await expect(inspector.getByLabel("移动 跨页卡片")).toHaveText(/完成/);
  await page.keyboard.press("Meta+1");
  await page.getByRole("link", { name: "测试任务看板" }).click();
  await expect(page).toHaveURL(new RegExp(board + "$"));
  await expect(
    page.getByRole("heading", { name: "测试任务看板" }),
  ).toBeVisible();
  await page.keyboard.press("Meta+1");
  await page.evaluate(() => {
    location.hash = "#/plugins";
  });
  const toggle = page.getByRole("switch", { name: "启用 测试插件" });
  await expect(page.locator('[data-workspace-tab^="extension:"]')).toHaveCount(
    2,
  );
  await expect(toggle).toBeChecked();
  await toggle.click();
  await page
    .getByRole("dialog", { name: "确认停用" })
    .getByRole("button", { name: "确认操作" })
    .click();
  await expect(toggle).not.toBeChecked();
  await page.keyboard.press("Meta+1");
  await expect(page.getByRole("link", { name: "测试任务看板" })).toHaveCount(0);
  await expect(page.locator('[data-workspace-tab^="extension:"]')).toHaveCount(
    0,
  );
  await page.evaluate((hash) => {
    location.hash = hash;
  }, sessionHash);
  await expect(inspector).toBeVisible();
  await expect(inspector.getByText("跨页卡片", { exact: true })).toHaveCount(0);
  await page
    .getByRole("navigation", { name: "工作区标签" })
    .getByRole("button", { name: "插件中心", exact: true })
    .click();
  await expect(toggle).not.toBeChecked();
  await toggle.click();
  await page
    .getByRole("dialog", { name: "确认启用" })
    .getByRole("button", { name: "确认操作" })
    .click();
  await expect(toggle).toBeChecked();
  await page.keyboard.press("Meta+1");
  await page.getByRole("link", { name: "测试任务看板" }).click();
  await expect(
    page
      .getByRole("region", { name: "完成", exact: true })
      .getByText("跨页卡片"),
  ).toBeVisible();
});
