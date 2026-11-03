import { test, expect, type Page } from "@playwright/test";
import type {} from "./provider-auto-connect-harness";
import {
  credentialReceipt,
  credentialOperation,
} from "../../src/features/providers/credential-receipt";

async function begin(page: Page, mode: string) {
  await page.goto("/tests/frontend/provider-auto-connect.html?mode=" + mode);
  await page
    .getByRole("button", { name: "自动识别并验证", exact: true })
    .click();
  await page
    .getByRole("textbox", { name: "服务地址 URL", exact: true })
    .fill("https://fixture.invalid/v1");
  await page
    .getByRole("textbox", { name: "API Key", exact: true })
    .fill("disposable-fixture-key");
  await page.getByRole("button", { name: "连接并保存", exact: true }).click();
}
const markers = (page: Page) =>
  page.evaluate(() =>
    Object.entries(localStorage).filter(([key]) =>
      key.startsWith("local-agent.credential-operation.v3."),
    ),
  );
async function openRecovery(page: Page) {
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
}

test("a lost credential reply is confirmed by its receipt without another create or business retry", async ({
  page,
}) => {
  await begin(page, "credential-lost");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const result = await page.evaluate(() => window.autoConnectFixture);
  expect(result.credentials).toHaveLength(1);
  expect(result.credentialChecks).toEqual([result.credentials[0].operationId]);
  expect(result.connections).toHaveLength(1);
  expect(await markers(page)).toHaveLength(0);
});

test("an unknown result survives reload without storing or replaying a key and requires explicit adoption", async ({
  page,
}) => {
  await begin(page, "credential-query");
  await expect(page.getByText("凭据结果待核查", { exact: true })).toBeVisible();
  const original = await page.evaluate(
    () => window.autoConnectFixture.credentials[0].operationId,
  );
  const stored = await markers(page);
  expect(stored).toHaveLength(1);
  expect(stored[0][0]).toContain(original);
  expect(JSON.stringify(stored)).not.toContain("disposable-fixture-key");
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=credential-restored",
  );
  await page
    .getByRole("button", { name: "自动识别并验证", exact: true })
    .click();
  await openRecovery(page);
  await expect(
    page.getByRole("button", { name: "核查保存结果", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(0);
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections),
  ).toHaveLength(0);
  await page.getByRole("button", { name: "核查保存结果", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "采用到当前连接", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections),
  ).toHaveLength(0);
  await page
    .getByRole("button", { name: "采用到当前连接", exact: true })
    .click();
  await page
    .getByRole("textbox", { name: "服务地址 URL", exact: true })
    .fill("https://fixture.invalid/v1");
  await page.getByRole("button", { name: "连接并保存", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(0);
  expect(await markers(page)).toHaveLength(0);
});

test("explicit retry uses the original operation ID, rejects a different key, and confirms only one credential", async ({
  page,
}) => {
  await begin(page, "credential-retry");
  await expect(page.getByText("凭据结果待核查", { exact: true })).toBeVisible();
  await openRecovery(page);
  await expect(
    page.getByRole("button", { name: "连接并保存", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("textbox", { name: "API Key", exact: true })
    .fill("wrong-fixture-key");
  await page
    .getByRole("button", { name: "用同一密钥重试", exact: true })
    .click();
  await expect(
    page.getByText("本次操作的密钥不同，请输入原密钥。", { exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections),
  ).toHaveLength(0);
  await page
    .getByRole("textbox", { name: "API Key", exact: true })
    .fill("disposable-fixture-key");
  await page
    .getByRole("button", { name: "用同一密钥重试", exact: true })
    .click();
  await expect(page.getByText(/当前草稿已采用/)).toBeVisible();
  const writes = await page.evaluate(
    () => window.autoConnectFixture.credentials,
  );
  expect(writes).toHaveLength(3);
  expect(new Set(writes.map((row) => row.operationId)).size).toBe(1);
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections),
  ).toHaveLength(0);
  await page.getByRole("button", { name: "连接并保存", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(await markers(page)).toHaveLength(0);
});

test("cancelling an unconfirmed operation clears its marker without creating or probing again", async ({
  page,
}) => {
  await begin(page, "credential-error");
  await expect(page.getByText("凭据结果待核查", { exact: true })).toBeVisible();
  await openRecovery(page);
  await page
    .getByRole("button", { name: "取消未完成保存", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "核查保存结果", exact: true }),
  ).toHaveCount(0);
  expect(await markers(page)).toHaveLength(0);
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(1);
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections),
  ).toHaveLength(0);
});

test("a cancelled issued write stays visible and a late materialized credential can be adopted", async ({
  page,
}) => {
  await begin(page, "credential-cancelled-late");
  await expect(page.getByText("凭据结果待核查", { exact: true })).toBeVisible();
  await openRecovery(page);
  await page
    .getByRole("button", { name: "取消未完成保存", exact: true })
    .click();
  await expect(
    page.getByText(/已停止后续保存，已发出写入仍待核查/),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "用同一密钥重试", exact: true }),
  ).toHaveCount(0);
  expect(await markers(page)).toHaveLength(1);
  await page.evaluate(() => window.autoConnectFixture.materialize?.());
  await page.getByRole("button", { name: "核查保存结果", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "采用到当前连接", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(1);
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections),
  ).toHaveLength(0);
});

test("a stopped reminder can be dismissed only after the backend re-confirms its cancellation", async ({
  page,
}) => {
  await begin(page, "credential-cancelled-late");
  await expect(page.getByText("凭据结果待核查", { exact: true })).toBeVisible();
  await openRecovery(page);
  await page
    .getByRole("button", { name: "取消未完成保存", exact: true })
    .click();
  await page.getByRole("button", { name: "移除本机提醒", exact: true }).click();
  expect(await markers(page)).toHaveLength(0);
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentialCancels),
  ).toHaveLength(2);
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections),
  ).toHaveLength(0);
});

test("a late credential result after unmount remains recoverable and does not start a connection", async ({
  page,
}) => {
  await begin(page, "credential-hold");
  await expect
    .poll(() =>
      page.evaluate(() => window.autoConnectFixture.credentials.length),
    )
    .toBe(1);
  await page.evaluate(() => window.autoConnectFixture.unmount?.());
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.evaluate(() => window.autoConnectFixture.release?.());
  await expect
    .poll(() =>
      page.evaluate(() => window.autoConnectFixture.connections.length),
    )
    .toBe(0);
  expect(await markers(page)).toHaveLength(1);
});

test("recovery metadata cleanup cannot turn a confirmed provider save into failure", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const remove = Storage.prototype.removeItem;
    Storage.prototype.removeItem = function (key) {
      if (key.startsWith("local-agent.credential-operation.v3."))
        throw new Error("fixture storage unavailable");
      return remove.call(this, key);
    };
  });
  await begin(page, "normal");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(await page.evaluate(() => window.autoConnectFixture.saved)).toBe(1);
});

test("credential receipts reject a wrong identity, missing status receipt and malformed references", () => {
  const id = "cf45b4a0-4dac-4bd5-b276-3181d3e1514e";
  const saved = {
    operationId: id,
    credentialRef: "e1b71318-84c0-4f5d-b399-aee182f69ade",
    credentialLabel: "Fixture",
  };
  expect(credentialReceipt(saved, id)).toEqual(saved);
  expect(() =>
    credentialReceipt({ ...saved, operationId: "wrong" }, id),
  ).toThrow();
  expect(() =>
    credentialReceipt({ ...saved, credentialRef: "wrong" }, id),
  ).toThrow();
  expect(() =>
    credentialOperation(
      { operationId: id, status: "ready", receipt: null },
      id,
    ),
  ).toThrow();
  expect(() =>
    credentialOperation(
      { operationId: id, status: "pending", receipt: saved },
      id,
    ),
  ).toThrow();
  expect(
    credentialOperation(
      { operationId: id, status: "removed", receipt: null },
      id,
    ).status,
  ).toBe("removed");
});
