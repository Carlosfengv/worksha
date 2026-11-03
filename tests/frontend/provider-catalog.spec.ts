import { test, expect } from "@playwright/test";

test("catalog metadata never reads models, pagination and explicit reads retain the selected binding page", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-catalog.html");
  await expect(
    page.getByRole("button", { name: "读取 连接 000 的模型目录" }),
  ).toBeVisible();
  expect(await page.evaluate(() => window.catalogCalls)).not.toContain("read");
  await page.getByRole("button", { name: "下一页", exact: true }).click();
  await page.getByRole("button", { name: "读取 连接 010 的模型目录" }).click();
  await expect(
    page.getByText("本次目录读取完成；目录声明不代表模型能力或连接已验证。"),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "读取 连接 010 的模型目录" }),
  ).toBeVisible();
  await expect(
    page.getByRole("cell", { name: "不支持", exact: true }),
  ).toBeVisible();
  expect(
    (await page.evaluate(() => window.catalogCalls)).filter(
      (c) => c === "read",
    ),
  ).toHaveLength(1);
});
test("revoked catalog bindings remain removable without enabling or reading", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-catalog.html?mode=disabled");
  await expect(
    page.getByRole("button", { name: "读取 连接 000 的模型目录" }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "解除 连接 000 的目录绑定" }).click();
  await expect(
    page.getByRole("button", { name: "解除 连接 000 的目录绑定" }),
  ).toHaveCount(0);
  expect(await page.evaluate(() => window.catalogCalls)).toContain("unbind");
  expect(await page.evaluate(() => window.catalogCalls)).not.toContain("read");
});
test("directory failures do not become empty success or trigger an automatic retry", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-catalog.html?mode=error");
  await page.getByRole("button", { name: "读取 连接 000 的模型目录" }).click();
  await expect(page.getByText("目录读取失败，原连接保持")).toBeVisible();
  await expect(page.getByText("本次读取确认目录为空")).toHaveCount(0);
  expect(
    (await page.evaluate(() => window.catalogCalls)).filter(
      (c) => c === "read",
    ),
  ).toHaveLength(1);
});
test("backend replacement retires late catalog reads and their success notice", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-catalog.html?mode=late");
  await page.getByRole("button", { name: "读取 连接 000 的模型目录" }).click();
  await expect
    .poll(() => page.evaluate(() => window.catalogCalls.includes("read")))
    .toBe(true);
  await page.evaluate(() => window.recoverCatalogBackend());
  await expect(
    page.getByRole("button", { name: "读取 连接 000 的模型目录" }),
  ).toBeDisabled();
  await page.evaluate(() => window.finishCatalogRead());
  await expect(
    page.getByText("本次目录读取完成；目录声明不代表模型能力或连接已验证。"),
  ).toHaveCount(0);
  expect(await page.evaluate(() => window.catalogCalls)).not.toContain(
    "changed",
  );
});
test("confirmed empty catalogs and a 720px keyboard-operated picker remain distinct from failures", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/provider-catalog.html?mode=empty");
  const picker = page.getByRole("combobox", { name: "目录连接", exact: true });
  await picker.fill("连接 030");
  await picker.press("ArrowDown");
  await picker.press("Enter");
  await page.getByRole("button", { name: "授权模型目录范围" }).click();
  await expect(
    page.getByText("目录范围已授权。点击对应记录的“读取目录”后才会发送请求。"),
  ).toBeVisible();
  expect(await page.evaluate(() => window.catalogCalls)).not.toContain("read");
  await page.getByRole("button", { name: "读取 连接 000 的模型目录" }).click();
  await expect(page.getByText("本次读取确认目录为空")).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
});
