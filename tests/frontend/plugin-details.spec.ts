import { expect, test } from "@playwright/test";
import type {} from "./plugin-details-harness";

for (const [index, target] of ["Agent", "Provider", "账户"].entries()) {
  test(`缩回窄窗后 ${target} 降级插槽的原焦点保持完整可见`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1920, height: 958 });
    await page.goto("/tests/frontend/plugin-details.html?mode=resize-focus");
    for (const name of ["Agent", "Provider", "账户"]) {
      await expect(
        page.getByRole("article", { name: `${name} · 扩展摘要`, exact: true }),
      ).toContainText("逻辑请求 17");
    }
    const section = page
      .getByRole("region", { name: "详情扩展信息", exact: true })
      .nth(index);
    const link = section.getByRole("link", { name: "管理插件", exact: true });
    await expect(link).toHaveCount(1);
    await link.evaluate((element) => element.scrollIntoView({ block: "end" }));
    await link.focus();
    await expect(link).toBeInViewport({ ratio: 1 });
    const queries = await page.evaluate(() => [...window.slotQueries]);
    await page.setViewportSize({ width: 720, height: 560 });
    await expect(link).toBeFocused();
    await expect(link).toBeInViewport({ ratio: 1 });
    expect(await page.evaluate(() => window.slotQueries)).toEqual(queries);
    expect(await page.evaluate(() => window.slotMutations)).toEqual([]);
  });
}

test("三个详情共用卡片与页签，只读查询不执行采集或模型", async ({ page }) => {
  await page.goto("/tests/frontend/plugin-details.html");
  await expect(
    page.getByRole("article", { name: "Agent · 扩展摘要" }),
  ).toContainText("逻辑请求 17");
  await expect(
    page.getByRole("article", { name: "Provider · 扩展摘要" }),
  ).toContainText("逻辑请求 17");
  await expect(
    page.getByRole("article", { name: "账户 · 扩展摘要" }),
  ).toContainText("逻辑请求 17");
  await expect(
    page.getByRole("tab", { name: "Agent · 增强信息" }),
  ).toBeVisible();
  expect(await page.evaluate(() => window.slotMutations)).toEqual([]);
  await expect(page.getByRole("button", { name: "手动刷新报表" })).toHaveCount(
    0,
  );
});
test("可选依赖缺失和恢复仅降级相关视图，轮询不执行动作", async ({ page }) => {
  await page.clock.install();
  await page.goto("/tests/frontend/plugin-details.html?mode=degraded");
  await expect(
    page.getByRole("article", { name: "Agent · 扩展摘要" }),
  ).toContainText("逻辑请求 17");
  await expect(
    page.getByText("可选依赖暂不可用", { exact: false }),
  ).toHaveCount(3);
  expect(
    (await page.evaluate(() => window.slotQueries)).includes("enhanced"),
  ).toBe(false);
  await page.evaluate(() => window.setSlotDependency(true));
  await page.clock.fastForward(10000);
  await expect(
    page.getByText("可选依赖暂不可用", { exact: false }),
  ).toHaveCount(0);
  expect(
    (await page.evaluate(() => window.slotQueries)).includes("enhanced"),
  ).toBe(true);
  expect(await page.evaluate(() => window.slotMutations)).toEqual([]);
});
test("旧深链、移除与返回保留原对象和原页签参数", async ({ page }) => {
  await page.clock.install();
  await page.goto(
    "/tests/frontend/plugin-details.html#/agents/claude-code-cli?tab=quota&pluginSlot=agent:org.example.detail-report:agent-enhanced",
  );
  await expect(
    page.getByRole("tab", { name: "Agent · 增强信息" }),
  ).toHaveAttribute("aria-selected", "true");
  await page.evaluate(() => window.removeSlotPlugin());
  await page.clock.fastForward(10000);
  await expect(
    page.getByText("插件详情插槽不可用", { exact: false }),
  ).toBeVisible();
  await page.getByRole("button", { name: "返回当前详情" }).click();
  expect(page.url()).toContain("tab=quota");
  expect(page.url()).not.toContain("pluginSlot=");
  await expect(
    page.getByRole("heading", { name: "原有详情保持" }),
  ).toBeVisible();
});
test("目录、声明和单视图错误不会破坏已有详情；旧服务保持兼容", async ({
  page,
}) => {
  for (const mode of [
    "directory-error",
    "invalid",
    "prototype",
    "view-error",
    "legacy",
  ]) {
    await page.goto("/tests/frontend/plugin-details.html?mode=" + mode);
    await expect(
      page.getByRole("heading", { name: "原有详情保持" }),
    ).toBeVisible();
    if (
      mode === "directory-error" ||
      mode === "invalid" ||
      mode === "prototype"
    )
      await expect(
        page.getByText("扩展信息暂不可用", { exact: false }),
      ).toHaveCount(3);
    if (mode === "view-error")
      await expect(
        page.getByText("此视图读取失败", { exact: false }),
      ).toHaveCount(6);
    if (mode === "legacy")
      await expect(
        page.getByRole("tab", { name: "Agent · 增强信息" }),
      ).toHaveCount(0);
    expect(await page.evaluate(() => window.slotMutations)).toEqual([]);
  }
});
test("切换对象后不显示旧请求迟到结果，窄屏与键盘可用", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/tests/frontend/plugin-details.html?mode=slow-object");
  await expect
    .poll(() => page.evaluate(() => window.slotQueries.length))
    .toBeGreaterThan(0);
  await page.getByRole("button", { name: "切换对象" }).click();
  await expect(
    page.getByRole("article", { name: "Agent · 扩展摘要" }),
  ).toContainText("逻辑请求 17");
  await page.waitForTimeout(1100);
  await expect(
    page.getByRole("article", { name: "Agent · 扩展摘要" }),
  ).not.toContainText("逻辑请求 999");
  await page.getByRole("tab", { name: "Agent · 增强信息" }).focus();
  await page.keyboard.press("Enter");
  await expect(
    page.getByRole("tab", { name: "Agent · 增强信息" }),
  ).toBeFocused();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
});
test("声明只匹配指定对象，切换后不沿用旧插槽", async ({ page }) => {
  await page.goto("/tests/frontend/plugin-details.html?mode=scoped");
  await expect(
    page.getByRole("article", { name: "Agent · 扩展摘要" }),
  ).toContainText("逻辑请求 17");
  await page.getByRole("button", { name: "切换对象" }).click();
  await expect(
    page.getByRole("article", { name: "Agent · 扩展摘要" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "原有详情保持" }),
  ).toBeVisible();
});
test("新版本或新授权响应不能显示在旧目录视图下", async ({ page }) => {
  await page.goto(
    "/tests/frontend/plugin-details.html?mode=new-version-response",
  );
  await expect(
    page.getByText("插件版本或授权已变化，请重新读取目录。"),
  ).toHaveCount(6);
  await expect(page.getByText("逻辑请求 17", { exact: false })).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "原有详情保持" }),
  ).toBeVisible();
});

for (const width of [720, 390]) {
  test(`长标题与降级依赖在 ${width}px 内完整换行，键盘页签和焦点可见`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 560 });
    if (width === 390)
      await page.addInitScript(() =>
        Object.defineProperty(Intl, "Segmenter", { value: undefined }),
      );
    await page.goto("/tests/frontend/plugin-details.html?mode=long-degraded");
    for (const target of ["agent", "provider", "account"]) {
      const card = page.getByRole("article", {
        name: target + " " + "C".repeat(80),
      });
      const heading = card.getByRole("heading").first();
      await expect(card).toContainText("逻辑请求 17");
      expect(
        await heading.evaluate((el) => el.scrollWidth <= el.clientWidth),
      ).toBe(true);
      const enhancedTitle = target + " " + "T".repeat(80);
      const basicTitle = target + " " + "B".repeat(80);
      const enhanced = page.getByRole("tab", { name: enhancedTitle });
      const basic = page.getByRole("tab", { name: basicTitle });
      await enhanced.focus();
      await page.keyboard.press("ArrowRight");
      await expect(basic).toBeFocused();
      await expect(basic).toHaveAttribute("aria-selected", "true");
      await expect(
        page.getByRole("heading", { name: basicTitle }),
      ).toBeVisible();
      const bounds = await basic.boundingBox();
      expect(bounds).not.toBeNull();
      expect(bounds!.x).toBeGreaterThanOrEqual(0);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
      await page.keyboard.press("ArrowLeft");
      await expect(enhanced).toHaveAttribute("aria-selected", "true");
      const panel = page.getByRole("tabpanel", { name: enhancedTitle });
      const dependency = panel.getByText("net." + "a".repeat(96), {
        exact: false,
      });
      await expect(dependency).toBeVisible();
      expect(
        await dependency.evaluate((el) => el.scrollWidth <= el.clientWidth),
      ).toBe(true);
      await page.keyboard.press("Tab");
      await page.keyboard.press("Tab");
      await expect(panel.getByRole("link", { name: "管理插件" })).toBeFocused();
    }
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
    expect(await page.evaluate(() => window.slotMutations)).toEqual([]);
    expect(
      await page.evaluate(() => window.slotQueries.includes("enhanced")),
    ).toBe(false);
  });
}
