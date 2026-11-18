import { test, expect } from "@playwright/test";

test("gateway revocation requires confirmation and preserves the client history", async ({
  page,
}) => {
  await page.goto("/tests/frontend/gateway.html");
  const client = page.getByRole("link", { name: "Laptop A", exact: true });
  const revoke = page.getByRole("button", { name: "撤销此密钥", exact: true });
  await client.click();
  await revoke.click();
  await expect(page.getByRole("dialog")).toContainText(
    "其他客户端和已有请求继续运行",
  );
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await expect(page.getByText(/^已启用 ·/)).toBeVisible();
  await revoke.click();
  await page.getByRole("button", { name: "确认撤销", exact: true }).click();
  await expect(page.getByText(/^已撤销 ·/)).toBeVisible();
  await expect(revoke).toBeDisabled();
  await page.evaluate(() => {
    location.hash = "/lan-gateway";
  });
  await expect(page.getByRole("row").filter({ has: client })).toContainText(
    "已撤销",
  );
});

test("gateway keeps stale data on read failure and distinguishes disabled from unknown", async ({
  page,
}) => {
  await page.goto("/tests/frontend/gateway.html?mode=stale");
  await expect(page.getByText("Laptop A", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "刷新状态" }).click();
  await expect(page.getByRole("alert")).toContainText("已有状态可能已过期");
  await expect(page.getByText("Laptop A", { exact: true })).toBeVisible();
  await page.goto("/tests/frontend/gateway.html?mode=disabled");
  await expect(page.getByText("此服务尚未开启局域网共享。")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "创建并保存连接文件" }),
  ).toHaveCount(0);
  await page.goto("/tests/frontend/gateway.html?mode=error");
  await expect(page.getByRole("alert")).toContainText("读取失败");
  await expect(page.getByText("此服务尚未开启局域网共享。")).toHaveCount(0);
});
