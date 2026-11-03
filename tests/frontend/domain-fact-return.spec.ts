import { expect, test } from "@playwright/test";

for (const secondPage of [false, true]) {
  test(`fact navigation returns to the same frozen response ${secondPage ? "page two" : "page one"}`, async ({
    page,
  }) => {
    await page.goto(
      "/tests/frontend/domain-readonly.html?unified=1&factsMany=1",
    );
    await page
      .getByRole("button", { name: "查看项目与响应事实", exact: true })
      .click();
    await page.getByLabel("事实种类", { exact: true }).click();
    await page.getByRole("option", { name: "模型响应", exact: true }).click();
    if (secondPage)
      await page
        .getByRole("button", { name: "下一页事实", exact: true })
        .click();
    const name = secondPage ? "Fixture-040" : "Fixture-000";
    const row = page.getByRole("row").filter({ hasText: name });
    await row.focus();
    await row.press("Enter");
    await page
      .getByRole("button", { name: "定位所属会话", exact: true })
      .click();
    await expect(
      page.getByRole("row").filter({ hasText: "Alpha" }),
    ).toBeVisible();
    const back = page.getByRole("button", {
      name: "返回上个事实范围",
      exact: true,
    });
    await back.focus();
    await back.press("Enter");
    await expect(row).toBeVisible();
    await expect(row).toHaveAttribute("aria-selected", "true");
    await expect(row).toBeFocused();
    await expect(
      page.getByLabel("来源事实详情", { exact: true }),
    ).toContainText(name);
    const query = await page.evaluate(() =>
      window.domainFixture.factQueries
        .filter((q) => q.kind === "response")
        .at(-1),
    );
    expect(query?.snapshotToken).toBe("facts-1");
    expect(query?.cursor).toBe(secondPage ? "40" : null);
    expect(query?.scope.sourceIds).toEqual(["org.example.reader:source"]);
    expect(
      (await page.evaluate(() => window.domainFixture.calls)).filter(
        (c) => c.startsWith("body:") || c === "refresh",
      ),
    ).toEqual([]);
  });
}

test("returning to a revoked fact range never restores private detail or body", async ({
  page,
}) => {
  await page.goto("/tests/frontend/domain-readonly.html?unified=1");
  await page
    .getByRole("button", { name: "查看项目与响应事实", exact: true })
    .click();
  await page.getByRole("row").filter({ hasText: "Reported workspace" }).click();
  await page.getByRole("button", { name: "查看项目会话", exact: true }).click();
  await page.getByRole("row").filter({ hasText: "Alpha" }).click();
  await page
    .getByRole("button", { name: "按当前授权读取此会话正文", exact: true })
    .click();
  await expect(
    page.getByText("Alpha private body", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "返回上个事实范围", exact: true })
    .click();
  await page.evaluate(() => window.domainFixture.revoke());
  await expect(
    page.getByText("Reported workspace", { exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByText("Alpha private body", { exact: true }),
  ).toHaveCount(0);
});
