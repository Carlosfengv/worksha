import { expect, test } from "@playwright/test";

test("unified request pagination preserves full source counts and unknown host evidence", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/domain-readonly.html?unified=1&fullFacts=1");
  await page.getByRole("button", { name: "查看统一请求", exact: true }).click();
  const summary = page.getByLabel("请求范围统计", { exact: true });
  await expect(summary).toContainText("83 个响应");
  await expect(summary).toContainText("尝试 未知");
  const first = page.getByRole("row").filter({ hasText: "来源响应-000" });
  await first.focus();
  await first.press("Enter");
  const detail = page.getByLabel("统一请求详情", { exact: true });
  await expect(detail).toContainText("来源报告耗时 9,007,199,254,740,993 ms");
  await expect(detail).toContainText("原请求记录耗时 未知");
  await expect(detail).toContainText("跨来源关联未经核实，费用未知");
  await expect(
    page.getByRole("link", { name: "查看原请求与费用明细" }),
  ).toHaveCount(0);
  const scroll = page.getByLabel("请求记录滚动区域", { exact: true });
  await scroll.focus();
  await scroll.press("End");
  await expect
    .poll(() => scroll.evaluate((e) => e.scrollTop))
    .toBeGreaterThan(0);
  await page.getByRole("button", { name: "下一页请求", exact: true }).click();
  await expect(
    page.getByRole("row").filter({ hasText: "来源响应-040" }),
  ).toBeVisible();
  await expect(summary).toContainText("83 个响应");
  await page.getByRole("button", { name: "下一页请求", exact: true }).click();
  await expect(
    page.getByRole("row").filter({ hasText: "来源响应-082" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "下一页请求", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "上一页请求", exact: true }).click();
  const row = page.getByRole("row").filter({ hasText: "来源响应-040" });
  await row.focus();
  await row.press("Space");
  await page.getByRole("button", { name: "查看会话证据", exact: true }).click();
  await page.getByRole("button", { name: "返回请求列表", exact: true }).click();
  await expect(
    page.getByRole("row").filter({ hasText: "来源响应-040" }),
  ).toBeVisible();
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter(
      (c) => c.startsWith("body:") || c === "refresh",
    ),
  ).toEqual([]);
});

test("request units and permission failures keep unavailable counts unknown", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/domain-readonly.html?unified=1&unapproved=1",
  );
  await page.getByRole("button", { name: "查看统一请求", exact: true }).click();
  const summary = page.getByLabel("请求范围统计", { exact: true });
  await expect(summary).toContainText("未知 个响应");
  await expect(summary).toContainText("待授权");
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).some(
      (c) => c === "refresh",
    ),
  ).toBe(false);
  await page.goto("/tests/frontend/domain-readonly.html?unified=1");
  await page.getByRole("button", { name: "查看统一请求", exact: true }).click();
  await page.getByRole("combobox", { name: "请求统计单位" }).click();
  await page.getByRole("option", { name: "网关逻辑请求", exact: true }).click();
  await expect(summary).toContainText("未知 个逻辑请求");
  await expect(summary).toContainText("不支持当前统计单位");
});

test("returning from evidence restores the triggering keyboard focus and request scroll", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/domain-readonly.html?unified=1&fullFacts=1");
  await page.getByRole("button", { name: "查看统一请求", exact: true }).click();
  await page.getByRole("button", { name: "下一页请求", exact: true }).click();
  const row = page.getByRole("row").filter({ hasText: "来源响应-075" });
  await row.focus();
  await row.press("Enter");
  const list = page.getByLabel("请求记录滚动区域", { exact: true });
  const position = await list.evaluate((e) => e.scrollTop);
  expect(position).toBeGreaterThan(0);
  const evidence = page.getByRole("button", {
    name: "查看会话证据",
    exact: true,
  });
  await evidence.focus();
  await evidence.press("Enter");
  const back = page.getByRole("button", { name: "返回请求列表", exact: true });
  await back.focus();
  await back.press("Enter");
  await expect(evidence).toBeFocused();
  await expect.poll(() => list.evaluate((e) => e.scrollTop)).toBe(position);
});

test("metric sorting resets frozen pages and reaches the full backend scope", async ({
  page,
}) => {
  await page.goto("/tests/frontend/domain-readonly.html?unified=1&fullFacts=1");
  await page.getByRole("button", { name: "查看统一请求", exact: true }).click();
  await page.getByRole("button", { name: "下一页请求", exact: true }).click();
  await expect(
    page.getByRole("row").filter({ hasText: "来源响应-040" }),
  ).toBeVisible();
  await page.getByRole("combobox", { name: "请求排序", exact: true }).click();
  await page
    .getByRole("option", { name: "总 Token从高到低", exact: true })
    .click();
  await expect(
    page.getByRole("row").filter({ hasText: "来源响应-082" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "上一页请求", exact: true }),
  ).toBeDisabled();
  const queries = await page.evaluate(
    () => window.domainFixture.requestQueries,
  );
  expect(queries.at(-1)?.sort).toBe("tokens");
  expect(queries.at(-1)?.includeEvidence).toBe(true);
  expect(queries.at(-1)?.cursor).toBeNull();
  await expect(page.getByRole("listbox")).toBeHidden();
  await page.getByRole("combobox", { name: "请求排序", exact: true }).click();
  await page
    .getByRole("option", { name: "记录耗时从高到低", exact: true })
    .click();
  await expect(
    page.getByRole("row").filter({ hasText: "来源响应-000" }),
  ).toBeVisible();
  await expect(page.getByLabel("请求范围统计", { exact: true })).toContainText(
    "来源报告耗时不作宿主耗时排序",
  );
});

for (const invalid of [false, true])
  test(`host evidence keeps independent currencies, zero and unknown parts: invalid=${invalid}`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 720, height: 560 });
    await page.goto(
      `/tests/frontend/domain-readonly.html?unified=1&builtins=1&requestHost=1${invalid ? "&invalidRequestMoney=1" : ""}`,
    );
    await page.locator("summary").filter({ hasText: "选择来源" }).click();
    await page.getByRole("checkbox", { name: /Codex CLI/ }).check();
    await page.locator("summary").filter({ hasText: "选择来源" }).click();
    await page
      .getByRole("button", { name: "查看统一请求", exact: true })
      .click();
    await page
      .getByRole("combobox", { name: "请求统计单位", exact: true })
      .click();
    await page
      .getByRole("option", { name: "网关逻辑请求", exact: true })
      .click();
    const summary = page.getByLabel("请求范围统计", { exact: true });
    if (invalid) {
      await expect(summary).toContainText("统一来源响应无效");
      await expect(
        page.getByRole("row").filter({ hasText: "网关费用夹具" }),
      ).toHaveCount(0);
      return;
    }
    const row = page.getByRole("row").filter({ hasText: "网关费用夹具" });
    await row.focus();
    await row.press("Space");
    const detail = page.getByLabel("冻结关联与费用证据", { exact: true });
    await expect(detail).toContainText("实付：0 USD（1 项）");
    await expect(detail).toContainText("估算：2.5 CNY（1 项）");
    await expect(detail).toContainText("费用未知项：1");
    await expect(detail).toContainText("用量冲突");
    await expect(summary).toContainText("有精确关联的逻辑请求 1 个");
    await expect(summary).toContainText("关联原生响应 1 个");
    await expect(summary).toContainText("尝试 3");
    await expect(summary).toContainText("1 条记录总用量不完整");
    await expect(summary).toContainText("底层总用量缺失项：2");
    await expect(
      page.getByRole("link", { name: "查看原请求与费用明细", exact: true }),
    ).toHaveAttribute("href", "#/requests/gateway-fixture");
  });
