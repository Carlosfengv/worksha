import { test, expect } from "@playwright/test";
import type {} from "./provider-auto-connect-harness";
import { PROVIDER_FIXTURE_STORE_ID as A } from "./provider-operation-fixture";
test("a late rejected restore cannot invalidate a fresh page after same-source reconnect", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=discovery-restore-hold",
  );
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  const find = page.getByRole("button", {
    name: "找回后台凭据记录",
    exact: true,
  });
  await expect(find).toBeEnabled();
  await find.click();
  let lookup = page.getByRole("dialog", {
    name: "找回后台凭据记录",
    exact: true,
  });
  await lookup
    .getByRole("button", { name: "添加恢复提示", exact: true })
    .click();
  await page
    .getByRole("checkbox", {
      name: "我已核对这条记录及当前后台，同意仅添加本机恢复提示。",
      exact: true,
    })
    .check();
  await page.getByRole("button", { name: "确认添加提示", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.providerOperationsFixture.commands.filter(
            (row) => row.name === "discover_provider_credentials",
          ).length,
      ),
    )
    .toBe(2);
  await page.evaluate(
    (a) => window.providerOperationsFixture.switchStore(a),
    A,
  );
  const cancel = page.getByRole("button", { name: "暂不添加", exact: true });
  await expect(cancel).toBeEnabled();
  await cancel.click();
  await lookup.getByRole("button", { name: "关闭查找", exact: true }).click();
  await expect(find).toBeEnabled();
  await find.click();
  lookup = page.getByRole("dialog", { name: "找回后台凭据记录", exact: true });
  const add = lookup.getByRole("button", { name: "添加恢复提示", exact: true });
  await expect(add).toBeEnabled();
  await page.evaluate(async () => {
    window.providerOperationsFixture.release();
    await new Promise(requestAnimationFrame);
    await new Promise(requestAnimationFrame);
  });
  await expect(
    lookup.getByText(/凭据操作仍属于原数据库和恢复空间/),
  ).toHaveCount(0);
  await expect(add).toBeEnabled();
  expect(
    await page.evaluate(() =>
      Object.keys(localStorage).filter((key) =>
        key.startsWith("local-agent.credential-operation."),
      ),
    ),
  ).toHaveLength(0);
});
