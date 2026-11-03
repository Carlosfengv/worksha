import { expect, test } from "@playwright/test";

for (const failRefresh of [false, true]) {
  test(`committed model changes clear prior verification notice even when refresh ${failRefresh ? "fails" : "succeeds"}`, async ({
    page,
  }) => {
    await page.goto(
      `/tests/frontend/provider-verification-notice.html${failRefresh ? "?fail-refresh" : ""}`,
    );
    await page
      .getByRole("button", { name: "验证默认模型", exact: true })
      .click();
    await page
      .getByRole("button", { name: "验证协议与模型", exact: true })
      .click();
    await expect(
      page.getByText("验证成功：model-a", { exact: true }),
    ).toBeVisible();
    await page.getByRole("tab", { name: "模型", exact: true }).click();
    const model = page.getByRole("row").filter({
      has: page.getByRole("cell", { name: "model-b", exact: true }),
    });
    await model.getByRole("button", { name: "设为默认", exact: true }).click();
    await expect(
      page.getByText(
        failRefresh ? /模型设置已保存，列表暂未刷新/ : "模型设置已保存。",
        { exact: !failRefresh },
      ),
    ).toBeVisible();
    await expect(
      page.getByText("验证成功：model-a", { exact: true }),
    ).toHaveCount(0);
    if (!failRefresh) {
      await page
        .getByRole("button", { name: "验证默认模型", exact: true })
        .click();
      await expect(page.getByRole("dialog")).toContainText("model-b");
      await page
        .getByRole("button", { name: "验证协议与模型", exact: true })
        .click();
      await expect(
        page.getByText("验证成功：model-b", { exact: true }),
      ).toBeVisible();
    }
  });
}
