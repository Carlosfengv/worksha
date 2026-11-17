import { expect, test } from "@playwright/test";

test("local connection lifecycle and evidence-based budget release", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=agents&localConnections=1#/agents",
  );
  await page.getByRole("button", { name: "添加本机连接 / 管理预算" }).click();
  await page
    .getByRole("textbox", { name: "本机连接名称" })
    .fill("本机测试连接");
  await page.getByRole("combobox", { name: "本机连接模型服务" }).click();
  await page.getByRole("option", { name: "测试模型服务" }).click();
  await page.getByRole("button", { name: "保存连接文件并激活" }).click();
  const detail = page.getByRole("region", { name: "本机连接详情" });
  await expect(detail).toContainText("Key v1 · 已激活");
  await page.getByRole("button", { name: "轮换 Key" }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "确认", exact: true })
    .click();
  await expect(detail).toContainText("Key v2 · 已激活");

  await page.getByRole("combobox", { name: "选择待核查预算尝试" }).click();
  await page.getByRole("option", { name: /fixture-model/ }).click();
  await expect(page.getByText("冻结 200 Tokens")).toBeVisible();
  await page.getByRole("combobox", { name: "Token核查方式" }).click();
  await page.getByRole("option", { name: "确认零用量并释放" }).click();
  await expect(page.getByRole("button", { name: "提交核查" })).toBeDisabled();
  await page
    .getByRole("textbox", { name: "证据出处" })
    .fill("模拟上游同请求账单");
  await page.getByRole("button", { name: "提交核查" }).click();
  await expect(page.getByText("冻结 200 Tokens")).toHaveCount(0);

  await page.getByRole("button", { name: "撤销绑定" }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "确认", exact: true })
    .click();
  await expect(detail).toContainText("Key v2 · 已撤销");
});
