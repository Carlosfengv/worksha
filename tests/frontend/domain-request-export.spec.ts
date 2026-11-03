import { expect, test } from "@playwright/test";

test("unified CSV exports the frozen full range from the final page with keyboard at minimum size", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto(
    "/tests/frontend/domain-readonly.html?unified=1&fullFacts=1&requestExport=hold",
  );
  await page.getByRole("button", { name: "查看统一请求", exact: true }).click();
  for (let n = 0; n < 2; n++)
    await page.getByRole("button", { name: "下一页请求", exact: true }).click();
  await expect(
    page.getByRole("row").filter({ hasText: "来源响应-082" }),
  ).toBeVisible();
  const button = page.getByRole("button", {
    name: "导出冻结范围 CSV",
    exact: true,
  });
  await button.focus();
  await button.press("Enter");
  await expect(button).toBeDisabled();
  await page.evaluate(() => window.domainFixture.release());
  await expect(button).toBeFocused();
  await expect(
    page
      .getByRole("status")
      .filter({ hasText: "已保存冻结范围内 83 条模型响应" }),
  ).toBeVisible();
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter((c) =>
      c.startsWith("export:"),
    ),
  ).toHaveLength(1);
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter(
      (c) => c === "refresh" || c.startsWith("body:"),
    ),
  ).toEqual([]);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});

for (const mode of ["cancel", "revoke"])
  test(`native-panel focus return waits for the enabled trigger: ${mode}`, async ({
    page,
  }) => {
    await page.goto(
      `/tests/frontend/domain-readonly.html?unified=1&requestExport=hold-${mode}`,
    );
    await page
      .getByRole("button", { name: "查看统一请求", exact: true })
      .click();
    const button = page.getByRole("button", {
      name: "导出冻结范围 CSV",
      exact: true,
    });
    await button.focus();
    await button.press("Enter");
    await expect(button).toBeDisabled();
    await page.evaluate(() => window.domainFixture.release());
    await expect(button).toBeEnabled();
    await expect(button).toBeFocused();
    await expect(
      page
        .getByRole(mode === "cancel" ? "status" : "alert")
        .filter({ hasText: mode === "cancel" ? "已取消保存" : "导出未保存" }),
    ).toBeVisible();
  });

test("export completion respects a later user focus and preserves a complete row at minimum height", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto(
    "/tests/frontend/domain-readonly.html?unified=1&requestExport=hold",
  );
  await page.getByRole("button", { name: "查看统一请求", exact: true }).click();
  const button = page.getByRole("button", {
    name: "导出冻结范围 CSV",
    exact: true,
  });
  await button.focus();
  await button.press("Enter");
  await expect(button).toBeDisabled();
  const search = page.getByRole("textbox", {
    name: "搜索统一请求",
    exact: true,
  });
  await search.focus();
  await page.evaluate(() => window.domainFixture.release());
  await expect(button).toBeEnabled();
  await expect(search).toBeFocused();
  const visibleRow = await page
    .getByLabel("请求记录滚动区域", { exact: true })
    .evaluate((root) => {
      const body = root.querySelector("tbody");
      const row = body?.querySelector("tr");
      if (!body || !row) return false;
      return (
        root.clientHeight -
          (body.getBoundingClientRect().top -
            root.getBoundingClientRect().top) >=
        row.getBoundingClientRect().height
      );
    });
  expect(visibleRow).toBe(true);
});

for (const mode of ["cancel", "revoke", "numeric"])
  test(`unified CSV feedback preserves cancellation and rejected results: ${mode}`, async ({
    page,
  }) => {
    await page.goto(
      `/tests/frontend/domain-readonly.html?unified=1&requestExport=${mode}`,
    );
    await page
      .getByRole("button", { name: "查看统一请求", exact: true })
      .click();
    const button = page.getByRole("button", {
      name: "导出冻结范围 CSV",
      exact: true,
    });
    await button.click();
    await expect(
      page.getByRole(mode === "cancel" ? "status" : "alert").filter({
        hasText:
          mode === "cancel"
            ? "已取消保存"
            : mode === "revoke"
              ? "来源授权已变化，导出未保存"
              : "统一来源响应无效",
      }),
    ).toBeVisible();
    await expect(button).toBeEnabled();
    await expect(
      page.getByText("已保存冻结范围内", { exact: false }),
    ).toHaveCount(0);
    await expect(
      page.getByLabel("请求范围统计", { exact: true }),
    ).toContainText("83 个响应");
  });
