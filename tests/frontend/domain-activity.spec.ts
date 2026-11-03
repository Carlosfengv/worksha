import { expect, test } from "@playwright/test";

for (const failure of ["partial", "unavailable"] as const) {
  test(`activity source ${failure} failure is explicit and never invents an empty success`, async ({
    page,
  }) => {
    await page.goto(
      `/tests/frontend/domain-readonly.html?unified=1&activityFailure=${failure}`,
    );
    await page
      .getByRole("button", { name: "查看活动快照", exact: true })
      .click();
    const summary = page.getByLabel("活动范围与单位说明", { exact: true });
    await expect(summary).toContainText(
      failure === "partial"
        ? "来源读取失败，保留已发布观测"
        : "来源读取失败，当前范围未知",
    );
    if (failure === "partial") {
      await expect(
        page.getByRole("row").filter({ hasText: "历史事件-000" }),
      ).toBeVisible();
      await expect(summary).toContainText("已观测 1 条");
    } else {
      await expect(
        page.getByRole("row").filter({ hasText: "历史事件-000" }),
      ).toHaveCount(0);
      await expect(summary).toContainText("已观测 未知 条");
    }
    expect(
      (await page.evaluate(() => window.domainFixture.calls)).filter(
        (c) => c.startsWith("body:") || c === "refresh",
      ),
    ).toEqual([]);
  });
}

test("historical activity keeps unknown time, explicit body access and keyboard scrolling at 720px", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto(
    "/tests/frontend/domain-readonly.html?unified=1&activityMany=1",
  );
  await page.getByRole("button", { name: "查看活动快照", exact: true }).click();
  const summary = page.getByLabel("活动范围与单位说明", { exact: true });
  await expect(summary).toContainText("已观测 320 条");
  await expect(summary).toContainText("时间未知 320");
  await expect(summary).toContainText("仅显示最近 256");
  const row = page.getByRole("row").filter({ hasText: "历史事件-000" });
  await row.focus();
  await row.press("Enter");
  const detail = page.getByLabel("活动观测详情", { exact: true });
  await expect(detail).toContainText("不提供实时采样");
  await expect(detail).toContainText("历史正文均速：未知");
  await expect(detail).toContainText("来源报告耗时：9,007,199,254,740,993 ms");
  await expect(
    page.getByRole("checkbox", { name: "包含所选 Agent 的网关观测" }),
  ).toBeDisabled();
  const pane = page.getByLabel("活动记录滚动区域", { exact: true });
  await pane.focus();
  await pane.press("End");
  await expect
    .poll(() => pane.evaluate((el) => el.scrollTop))
    .toBeGreaterThan(0);
  await detail.focus();
  await detail.press("End");
  await expect
    .poll(() => detail.evaluate((el) => el.scrollTop))
    .toBeGreaterThan(0);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.getByRole("button", { name: "查看会话证据", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "按当前授权读取此会话正文", exact: true }),
  ).toHaveCount(0);
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter(
      (c) =>
        c.startsWith("body:") || c.startsWith("consent:") || c === "refresh",
    ),
  ).toEqual([]);
});

test("gateway attempts and logical requests remain separate and live samples expire locally", async ({
  page,
}) => {
  await page.clock.install();
  await page.goto("/tests/frontend/domain-readonly.html?unified=1&builtins=1");
  await page.locator("summary").filter({ hasText: "选择来源" }).click();
  await page.getByRole("checkbox", { name: /Reader · 已授权/ }).uncheck();
  await page.getByRole("checkbox", { name: /OpenCode · 待授权/ }).check();
  await page.getByRole("button", { name: "查看活动快照", exact: true }).click();
  const row = page.getByRole("row").filter({ hasText: "原生活动-000" });
  await row.click();
  const detail = page.getByLabel("活动观测详情", { exact: true });
  await expect(detail).toContainText("42 Token/s · 5 秒窗口");
  await page
    .getByRole("checkbox", { name: "包含所选 Agent 的网关观测" })
    .check();
  await expect(
    page.getByLabel("活动范围与单位说明", { exact: true }),
  ).toContainText("网关尝试：已观测 2 条");
  await expect(
    page.getByLabel("活动范围与单位说明", { exact: true }),
  ).toContainText("逻辑请求 1 个");
  await page.clock.fastForward(6100);
  await expect(detail).toContainText("实时速率未知");
  await expect(detail).toContainText("历史正文均速：17 Token/s");
  await expect(detail).toContainText("原生事件首出 55 ms");
  await expect(detail).toContainText("运行采样 TTFT 80 ms");
  await expect
    .poll(() =>
      page.evaluate(
        () => window.domainFixture.activityQueries.at(-1)?.includeGateway,
      ),
    )
    .toBe(true);
});

test("late activity query cannot restore a deselected historical scope or revoked content", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/domain-readonly.html?unified=1&activityDelay=1",
  );
  await page.getByRole("button", { name: "查看活动快照", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() => window.domainFixture.activityQueries.length),
    )
    .toBe(1);
  await page.getByRole("checkbox", { name: "包含来源历史事件" }).uncheck();
  await expect(
    page.getByLabel("活动范围与单位说明", { exact: true }),
  ).toContainText("已观测 未知 条");
  await page.evaluate(() => window.domainFixture.release());
  await expect(
    page.getByRole("row").filter({ hasText: "历史事件-000" }),
  ).toHaveCount(0);
  await page.evaluate(() => window.domainFixture.revoke());
  await expect(page.getByLabel("统一活动快照", { exact: true })).toHaveCount(0);
  await expect(page.getByText("当前范围尚无可读会话")).toBeVisible();
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter(
      (c) => c.startsWith("body:") || c.startsWith("consent:"),
    ),
  ).toEqual([]);
});
