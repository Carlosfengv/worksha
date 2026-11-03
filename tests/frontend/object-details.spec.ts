import { expect, test } from "@playwright/test";
import type {} from "./object-details-harness";

test("摘要包独立入口只说明使用范围，不走旧报表查询与刷新", async ({ page }) => {
  await page.goto("/tests/frontend/object-details.html?mode=standalone");
  await expect(
    page.getByRole("region", { name: "对象摘要扩展使用范围" }),
  ).toContainText("逐项确认摘要字段与用途");
  await expect(page.getByRole("button", { name: "手动刷新报表" })).toHaveCount(
    0,
  );
  await expect(page.getByRole("button", { name: /授权报表汇总/ })).toHaveCount(
    0,
  );
  await expect(page.getByText("来源文件：", { exact: false })).toHaveCount(0);
  expect(await page.evaluate(() => window.objectHarness.commands)).toEqual([]);
});

test("统一来源模型响应不被标为原生响应，保留原冻结来源与单位", async ({
  page,
}) => {
  await page.goto("/tests/frontend/object-details.html?mode=external-response");
  const card = page.getByRole("article", { name: "request card" });
  await expect(card).toContainText("范围：原冻结查询中的单条模型响应。");
  await expect(card).not.toContainText("原生模型响应");
  const objects = await page.evaluate(() =>
    window.objectHarness.calls.flatMap((call) =>
      call.kind === "issue" && call.request.object.target === "request"
        ? [call.request.object]
        : [],
    ),
  );
  expect(objects).toHaveLength(2);
  expect(objects[0]).toMatchObject({
    unit: "model-response",
    origin: {
      kind: "unified",
      sourceId: "org.example.history",
      snapshotToken: "owned-frozen-request-snapshot",
      scope: { kind: "external", sourceIds: ["org.example.history"] },
    },
  });
});

test("六类摘要保留字段单位和大整数，目录轮询不会重建引用", async ({ page }) => {
  await page.clock.install();
  await page.goto("/tests/frontend/object-details.html");
  await expect(
    page.getByRole("article", { name: "session card" }),
  ).toContainText("9,007,199,254,740,993");
  await expect(
    page.getByRole("article", { name: "provider card" }),
  ).toContainText("已保存启用模型数0");
  await expect(
    page.getByRole("article", { name: "request card" }),
  ).toContainText("1.25 ms");
  await expect(
    page.getByRole("article", { name: "request card" }),
  ).toContainText("未记录");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.objectHarness.calls.filter((c) => c.kind === "issue").length,
      ),
    )
    .toBe(12);
  await page.clock.fastForward(20_000);
  expect(
    await page.evaluate(
      () => window.objectHarness.calls.filter((c) => c.kind === "issue").length,
    ),
  ).toBe(12);
  expect(
    await page.evaluate(
      () =>
        window.objectHarness.calls.filter(
          (c) => c.kind === "grant" || c.kind === "revoke",
        ).length,
    ),
  ).toBe(0);
});

test("摘要授权列明范围字段用途，默认取消不写授权", async ({ page }) => {
  await page.goto("/tests/frontend/object-details.html?mode=unauthorized");
  const card = page.getByRole("article", { name: "session card" });
  await card.getByRole("button", { name: "授权此摘要", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("授权适用于该类对象");
  await expect(dialog).toContainText("输入 Token、总 Token");
  await expect(
    dialog.getByRole("button", { name: "取消", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(
    card.getByRole("button", { name: "授权此摘要", exact: true }),
  ).toBeFocused();
  expect(
    await page.evaluate(() =>
      window.objectHarness.calls.some((c) => c.kind === "grant"),
    ),
  ).toBe(false);
  await card.getByRole("button", { name: "授权此摘要", exact: true }).click();
  await dialog
    .getByRole("button", { name: "确认授权摘要", exact: true })
    .click();
  await expect(card).toContainText("9,007,199,254,740,993");
  const revoke = card.getByRole("button", {
    name: "撤销摘要授权",
    exact: true,
  });
  await expect(revoke).toBeFocused();
  expect(
    await page.evaluate(
      () => window.objectHarness.calls.filter((c) => c.kind === "grant").length,
    ),
  ).toBe(1);
  await revoke.click();
  await dialog
    .getByRole("button", { name: "确认撤销授权", exact: true })
    .click();
  await expect(
    card.getByRole("button", { name: "授权此摘要", exact: true }),
  ).toBeFocused();
  await expect(card).toContainText("尚未授权此对象摘要");
});

test("窄窗口中摘要展开后，返回的授权按钮仍完整可见", async ({ page }) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/object-details.html?mode=unauthorized");
  const card = page.getByRole("article", { name: "session card" });
  const trigger = card.getByRole("button", { name: "授权此摘要", exact: true });
  await trigger.evaluate((element) => element.scrollIntoView({ block: "end" }));
  await trigger.click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "确认授权摘要", exact: true })
    .click();
  await expect(card).toContainText("9,007,199,254,740,993");
  const returned = card.getByRole("button", {
    name: "撤销摘要授权",
    exact: true,
  });
  await expect(returned).toBeFocused();
  await expect(returned).toBeInViewport({ ratio: 1 });
});

test("授权上下文变化关闭旧确认，不能沿用确认授权新上下文", async ({ page }) => {
  await page.goto("/tests/frontend/object-details.html?mode=unauthorized");
  const card = page.getByRole("article", { name: "session card" });
  const trigger = card.getByRole("button", { name: "授权此摘要", exact: true });
  await trigger.click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.evaluate(() => window.objectHarness.changeContext());
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(trigger).toBeFocused();
  expect(
    await page.evaluate(() =>
      window.objectHarness.calls.some((c) => c.kind === "grant"),
    ),
  ).toBe(false);
  await page.keyboard.press("Enter");
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "取消", exact: true }),
  ).toBeFocused();
});

test("授权由未获取焦点的指针操作打开时，取消仍回到原插槽", async ({ page }) => {
  await page.goto("/tests/frontend/object-details.html?mode=unauthorized");
  const card = page.getByRole("article", { name: "session card" });
  const trigger = card.getByRole("button", { name: "授权此摘要", exact: true });
  // Native WebKit does not focus every pointer-activated button.
  await page.getByRole("button", { name: "切换会话对象" }).focus();
  await trigger.dispatchEvent("click");
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(trigger).toBeFocused();
  expect(
    await page.evaluate(() =>
      window.objectHarness.calls.some((c) => c.kind === "grant"),
    ),
  ).toBe(false);
});

for (const mode of ["late-issue", "late-read"])
  test(`${mode} 切换对象后旧响应不覆盖新摘要且精确回收`, async ({ page }) => {
    await page.goto(`/tests/frontend/object-details.html?mode=${mode}`);
    await expect
      .poll(() => page.evaluate(() => window.objectHarness.held()))
      .toBe(1);
    await page.getByRole("button", { name: "切换会话对象" }).click();
    const card = page.getByRole("article", { name: "session card" });
    await expect(card.getByText("42", { exact: true })).toHaveCount(2);
    await page.evaluate(() => window.objectHarness.release());
    await expect(card.getByText("42", { exact: true })).toHaveCount(2);
    await expect(card).not.toContainText("9,007,199,254,740,993");
    await page.evaluate(() => window.objectHarness.unmount());
    await expect
      .poll(() => page.evaluate(() => window.objectHarness.refs()))
      .toBe(0);
  });

test("可选依赖只降级增强页签，撤销来源立即清除旧摘要", async ({ page }) => {
  await page.goto("/tests/frontend/object-details.html?mode=degraded");
  const card = page.getByRole("article", { name: "session card" });
  await expect(card).toContainText("9,007,199,254,740,993");
  await expect(
    page.getByRole("status").filter({ hasText: "可选依赖暂不可用" }),
  ).toBeVisible();
  await page.evaluate(() => window.objectHarness.dependency(true));
  await expect(
    page.getByRole("status").filter({ hasText: "可选依赖暂不可用" }),
  ).toHaveCount(0);
  await page.evaluate(() => window.objectHarness.revokeSource());
  await expect(card).not.toContainText("9,007,199,254,740,993");
  await expect(card.getByRole("alert")).toContainText("来源授权已撤销");
  await expect(
    page.getByRole("article", { name: "project card" }),
  ).toContainText("已记录会话数2");
});

test("错误对象响应只影响单个插槽", async ({ page }) => {
  await page.goto("/tests/frontend/object-details.html?mode=wrong-target");
  await expect(
    page.getByRole("article", { name: "session card" }).getByRole("alert"),
  ).toContainText("当前对象不一致");
  await expect(
    page.getByRole("article", { name: "project card" }),
  ).toContainText("已记录会话数2");
  await expect(
    page.getByRole("heading", { name: "原有详情内容" }),
  ).toBeVisible();
});

test("引用到期清除摘要，重新读取创建新引用", async ({ page }) => {
  await page.clock.install();
  await page.goto("/tests/frontend/object-details.html");
  const card = page.getByRole("article", { name: "session card" });
  await expect(card).toContainText("9,007,199,254,740,993");
  await page.clock.fastForward(61_000);
  await expect(card).not.toContainText("9,007,199,254,740,993");
  await expect(card.getByRole("alert")).toContainText("过期");
  await card.getByRole("button", { name: "重新读取摘要" }).click();
  await expect(card).toContainText("9,007,199,254,740,993");
});
