import { expect, test } from "@playwright/test";

const canonical = "00000000-0000-4000-8000-000000000002";
for (const serviceId of [
  `urn:uuid:${canonical}`,
  `{${canonical}}`,
  canonical.replaceAll("-", ""),
]) {
  test(`legacy service spelling retains its unknown scope warning: ${serviceId}`, async ({
    page,
  }) => {
    await page.goto(
      `/tests/frontend/routing-policy.html?serviceId=${encodeURIComponent(serviceId)}`,
    );
    await page.getByRole("combobox", { name: "模型服务", exact: true }).click();
    await page.getByRole("option", { name: "编程服务", exact: true }).click();
    await page.evaluate((id) => {
      window.routingPolicyFixture.malformedRegistry = true;
      window.routingPolicyFixture.registryUnknownServiceId = id;
    }, canonical);
    await page
      .getByRole("button", { name: "重新读取策略", exact: true })
      .click();
    await expect(page.getByRole("alert")).toContainText("当前绑定状态未知");
    await expect(
      page.getByText("当前未绑定外部策略", { exact: true }),
    ).toHaveCount(0);
    await page.evaluate(() => {
      window.routingPolicyFixture.registryUnknownServiceId =
        "00000000-0000-4000-8000-000000000003";
    });
    await page
      .getByRole("button", { name: "重新读取策略", exact: true })
      .click();
    await expect(page.getByRole("alert")).toHaveCount(0);
    expect(
      await page.evaluate(() =>
        window.routingPolicyFixture.calls.some((c) => c.kind === "bind"),
      ),
    ).toBe(false);
  });
}
