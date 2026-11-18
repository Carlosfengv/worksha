import { expect, test } from "@playwright/test";

const clientId = "7bc3273c-8ca4-4b6b-9de9-5da3876ccbe1";
test("revoked keys retain scoped usage, requests and CSV export", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=workspace#/lan-gateway");
  const row = page.getByRole("row").filter({ hasText: "已撤销 Mac" });
  await expect(row).toContainText("120 tok");
  await expect(row).toContainText("0.45");
  await row.getByRole("link").click();
  await expect(page).toHaveURL(new RegExp(`lan-gateway/${clientId}`));
  const audit = page.getByRole("region", { name: "密钥请求审计" });
  await expect(audit).toContainText(`client:${clientId}`);
  await expect
    .poll(() => page.evaluate(() => window.workspaceAuditClient))
    .toBe(clientId);
  await expect(audit.locator("[data-request-id]")).toHaveCount(1);
  await audit.getByRole("button", { name: "导出此密钥 CSV" }).click();
  await expect
    .poll(() => page.evaluate(() => window.workspaceExported))
    .toBe(`audit-${clientId}`);
  await audit.locator('[data-request-id="req-2"]').click();
  await expect(
    page.getByRole("link", { name: "打开客户端密钥审计" }),
  ).toBeVisible();
});

test("one-time issue receipt is cleared after acknowledgement", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=workspace#/lan-gateway");
  await page.getByRole("button", { name: /签发客户端密钥/ }).click();
  const form = page.getByRole("dialog", {
    name: "签发客户端密钥",
    exact: true,
  });
  await form.getByLabel("客户端名称", { exact: true }).fill("审计测试 Mac");
  await form.getByRole("combobox", { name: "Provider" }).fill("连接 000");
  await page.getByRole("option", { name: "连接 000", exact: true }).click();
  await form.getByRole("combobox", { name: "可调用模型" }).fill("model-000");
  await page.getByRole("option", { name: "model-000", exact: true }).click();
  await form.getByRole("button", { name: "签发密钥", exact: true }).click();
  const receipt = page.getByRole("dialog", { name: "客户端密钥已签发" });
  await expect(receipt).toBeVisible();
  await expect(receipt.getByText(/ANTHROPIC_AUTH_TOKEN/)).toBeVisible();
  await receipt.getByRole("button", { name: "完成", exact: true }).click();
  await expect(receipt).toHaveCount(0);
  const client = page.getByRole("link", { name: "审计测试 Mac", exact: true });
  await expect(client).toBeVisible();
  await client.click();
  await expect(page.getByText("客户端 UUID", { exact: true })).toBeVisible();
  await expect(page.getByText(/ANTHROPIC_AUTH_TOKEN/)).toHaveCount(0);
});
