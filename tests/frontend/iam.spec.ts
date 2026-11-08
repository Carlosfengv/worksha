import { expect, test } from "@playwright/test";

test("系统 IAM 按需开启，Owner 分配模型服务后成员可查看创建者", async ({
  page,
}) => {
  await page.goto("/tests/frontend/iam.html#/plugins");
  const toggle = page.getByRole("switch", {
    name: "启用 账号管理",
    exact: true,
  });
  await expect(toggle).not.toBeChecked();
  await expect(
    page.getByRole("link", { name: "账号管理", exact: true }),
  ).toHaveCount(0);
  await toggle.click();
  await expect(page.getByRole("dialog")).toContainText("无法关闭或卸载");
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await expect(toggle).not.toBeChecked();

  await toggle.click();
  await page.getByRole("button", { name: "继续设置" }).click();
  const setup = page.getByRole("dialog");
  await setup.getByLabel("登录名", { exact: true }).fill("owner");
  await setup.getByLabel("显示名", { exact: true }).fill("林青");
  await setup.getByLabel("密码", { exact: true }).fill("fixture-password");
  await setup.getByRole("checkbox", { name: "我已保存恢复码" }).click();
  await setup.getByRole("button", { name: "创建并开启" }).click();
  await expect(page.getByRole("heading", { name: "登录账号" })).toBeVisible();
  await expect(page.getByRole("navigation", { name: "主导航" })).toHaveCount(0);

  await page.getByLabel("登录名", { exact: true }).fill("owner");
  await page.getByLabel("密码", { exact: true }).fill("fixture-password");
  await page.getByRole("button", { name: "登录", exact: true }).click();
  await expect(toggle).toBeChecked();
  await expect(toggle).toBeDisabled();
  await page.getByRole("link", { name: "账号管理", exact: true }).click();
  await page.getByRole("button", { name: "新建账号" }).click();
  const create = page.getByRole("dialog");
  await create.getByLabel("登录名", { exact: true }).fill("member");
  await create.getByLabel("显示名", { exact: true }).fill("周宁");
  await create.getByLabel("初始密码", { exact: true }).fill("fixture-password");
  await create.getByRole("button", { name: "保存账号" }).click();
  await expect(page.getByRole("region", { name: "账号详情" })).toContainText(
    "周宁",
  );
  await page.getByRole("combobox", { name: "选择模型服务" }).fill("文档");
  await page.getByRole("option", { name: "文档助手" }).click();
  await page.getByRole("button", { name: "添加资源" }).click();
  await expect(page.getByRole("region", { name: "已关联资源" })).toContainText(
    "文档助手",
  );

  await page.getByRole("button", { name: "退出登录" }).click();
  await page.getByLabel("登录名", { exact: true }).fill("member");
  await page.getByLabel("密码", { exact: true }).fill("fixture-password");
  await page.getByRole("button", { name: "登录", exact: true }).click();
  await expect(page.getByRole("heading", { name: "我的资源" })).toBeVisible();
  await page.getByRole("row", { name: "文档助手" }).click();
  await expect(page.getByRole("dialog")).toContainText("创建者");
  await expect(page.getByRole("dialog")).toContainText("林青");
});
