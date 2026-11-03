import { expect, test, type Page } from "@playwright/test";
import type {} from "./gateway-operations-harness";
import type { PendingGatewayOperation } from "../../src/features/settings/types";
import {
  gatewayStoreA as A,
  gatewayStoreB as B,
  gatewayClientId,
} from "./gateway-operation-fixture";
const prefix = "local-agent.gateway-operation.v1.";
const original: PendingGatewayOperation = {
  request: {
    storeId: A,
    operationId: "33333333-3333-4333-8333-333333333333",
    action: {
      kind: "create",
      draft: {
        name: "原客户端",
        agent: "claude",
        providerId: "fixture",
        models: ["fixture-model"],
        concurrency: 2,
      },
    },
  },
  compensationOperationId: "44444444-4444-4444-8444-444444444444",
};
const createButton = (page: Page) =>
  page.getByRole("button", { name: "创建并保存连接文件", exact: true });
const region = (page: Page) =>
  page.getByRole("region", { name: "保留的客户端操作", exact: true });
const records = (page: Page) => page.evaluate(() => window.gatewayOps.read());
async function open(page: Page) {
  await page.goto("/tests/frontend/gateway-operations.html");
  await expect(page.getByText("Laptop A", { exact: true })).toBeVisible();
}
async function fill(page: Page, name = "新客户端") {
  await expect(page.getByLabel("客户端名称", { exact: true })).toBeEnabled();
  await page.getByLabel("客户端名称", { exact: true }).fill(name);
  await page.getByRole("combobox", { name: "Provider", exact: true }).click();
  await page
    .getByRole("option", { name: "Fixture provider", exact: true })
    .click();
  await page.getByRole("combobox", { name: "可调用模型", exact: true }).click();
  await page
    .getByRole("option", { name: "fixture-model", exact: true })
    .click();
}
async function start(page: Page) {
  await open(page);
  await fill(page);
  await createButton(page).click();
  await expect
    .poll(() => page.evaluate(() => window.gatewayOps.writes.length))
    .toBe(1);
  return page.evaluate(
    () => window.gatewayOps.writes[0].args.input as PendingGatewayOperation,
  );
}
async function seed(page: Page, row = original) {
  await page.evaluate((row) => window.gatewayOps.remember(row), row);
  await expect(
    region(page).getByText(`操作编号：${row.request.operationId}`, {
      exact: true,
    }),
  ).toBeVisible();
}
async function saveReply(
  page: Page,
  status: "saved" | "issued" | "cancelled" | "notIssued",
  index = 0,
) {
  await page.evaluate(
    ({ status, index }) => {
      const pending = window.gatewayOps.writes[index],
        input = pending.args.input as PendingGatewayOperation;
      if (status === "cancelled") {
        pending.resolve({ status });
        return;
      }
      if (status === "notIssued") {
        pending.resolve({ status, reason: "fixture explicit no new issuance" });
        return;
      }
      pending.resolve({
        status,
        outcome: window.gatewayOps.outcome(input.request),
        sourceInstanceId: input.request.storeId,
        sourceCurrent: true,
        ...(status === "saved" ? { fileName: "client.json" } : {}),
      });
    },
    { status, index },
  );
}
async function queryReply(page: Page, row: PendingGatewayOperation, index = 0) {
  await page.evaluate(
    ({ row, index }) =>
      window.gatewayOps.checks[index].resolve(
        window.gatewayOps.outcome(row.request),
      ),
    { row, index },
  );
}

test("original request and compensation identity are durably whitelisted before the native save begins", async ({
  page,
}) => {
  const row = await start(page);
  const call = await page.evaluate(() => window.gatewayOps.writes[0].persisted);
  expect(call.requests).toEqual([row]);
  expect(row.compensationOperationId).not.toBe(row.request.operationId);
  const raw = await page.evaluate(
    (key) => localStorage.getItem(key),
    prefix + row.request.operationId,
  );
  expect(JSON.parse(raw!)).toEqual({ version: 1, ...row });
  expect(raw).not.toContain("credentialRef");
  expect(raw).not.toContain("fixture.invalid");
  expect(raw).not.toContain("must-not-persist");
  await saveReply(page, "saved");
  await expect.poll(async () => (await records(page)).requests.length).toBe(0);
  await expect(page.getByText(/连接文件已保存为 client.json/)).toBeVisible();
  await expect(page.getByLabel("客户端名称", { exact: true })).toHaveValue("");
});
test("unknown save survives unmount and reload with no automatic operation queries or writes", async ({
  page,
}) => {
  const row = await start(page);
  await page.evaluate(() => window.gatewayOps.writes[0].reject());
  await expect(
    page.getByRole("alert").filter({ hasText: "transport_timeout" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "切换客户端挂载" }).click();
  await page.getByRole("button", { name: "切换客户端挂载" }).click();
  expect((await records(page)).requests).toEqual([row]);
  expect(await page.evaluate(() => window.gatewayOps.checks.length)).toBe(0);
  await page.reload();
  await expect(
    region(page).getByText(`操作编号：${row.request.operationId}`, {
      exact: true,
    }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => [
      window.gatewayOps.writes.length,
      window.gatewayOps.revokes.length,
      window.gatewayOps.checks.length,
    ]),
  ).toEqual([0, 0, 0]);
});
test("not found does not clear the original identity and cancelling a retry keeps it", async ({
  page,
}) => {
  await open(page);
  await seed(page);
  await region(page).getByRole("button", { name: "核查原客户端操作" }).click();
  await expect
    .poll(() => page.evaluate(() => window.gatewayOps.checks.length))
    .toBe(1);
  await page.evaluate(() =>
    window.gatewayOps.checks[0].reject("operation_not_found", false),
  );
  await expect(
    page.getByRole("alert").filter({ hasText: "operation_not_found" }),
  ).toBeVisible();
  await region(page).getByRole("button", { name: "重试原客户端操作" }).click();
  await expect
    .poll(() => page.evaluate(() => window.gatewayOps.writes.length))
    .toBe(1);
  expect(
    await page.evaluate(() => window.gatewayOps.writes[0].args.input),
  ).toEqual(original);
  await saveReply(page, "cancelled");
  expect((await records(page)).requests).toEqual([original]);
  await expect(page.getByText(/原客户端操作仍待核查/)).toBeVisible();
});
test("fresh cancellation releases its unused identity without issuing another operation", async ({
  page,
}) => {
  await start(page);
  await saveReply(page, "cancelled");
  await expect.poll(async () => (await records(page)).requests.length).toBe(0);
  await expect(page.getByText("已取消本次选择，未创建客户端。")).toBeVisible();
  expect(
    await page.evaluate(() => [
      window.gatewayOps.checks.length,
      window.gatewayOps.revokes.length,
    ]),
  ).toEqual([0, 0]);
});
test("definite rejection permits fresh draft correction while an earlier unknown retry retains its identity", async ({
  page,
}) => {
  await start(page);
  await saveReply(page, "notIssued");
  await expect.poll(async () => (await records(page)).requests.length).toBe(0);
  await expect(page.getByLabel("客户端名称", { exact: true })).toBeEnabled();
  await seed(page);
  await region(page).getByRole("button", { name: "重试原客户端操作" }).click();
  await expect
    .poll(() => page.evaluate(() => window.gatewayOps.writes.length))
    .toBe(2);
  await saveReply(page, "notIssued", 1);
  expect((await records(page)).requests).toEqual([original]);
  await expect(page.getByText(/原操作仍保留待核查/)).toBeVisible();
});
test("confirmed issuance is file-unknown and requires explicit file acknowledgement", async ({
  page,
}) => {
  const row = await start(page);
  await saveReply(page, "issued");
  await expect(
    region(page).getByText("签发已确认，原连接文件待核对。"),
  ).toBeVisible();
  expect((await records(page)).requests).toEqual([row]);
  await expect(
    region(page).getByRole("button", { name: "重试原客户端操作" }),
  ).toBeDisabled();
  await region(page)
    .getByRole("button", { name: "连接文件已核对", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toContainText("不更改客户端授权");
  await page
    .getByRole("button", { name: "确认文件已核对", exact: true })
    .click();
  await expect.poll(async () => (await records(page)).requests.length).toBe(0);
  expect(
    await page.evaluate(() => [
      window.gatewayOps.checks.length,
      window.gatewayOps.revokes.length,
    ]),
  ).toEqual([0, 0]);
});
test("explicit reconciliation and revocation use the original compensation ID and distinguish state proof from a receipt", async ({
  page,
}) => {
  await open(page);
  await seed(page);
  await region(page).getByRole("button", { name: "核查原客户端操作" }).click();
  await expect
    .poll(() => page.evaluate(() => window.gatewayOps.checks.length))
    .toBe(1);
  expect(await page.evaluate(() => window.gatewayOps.checks[0].args)).toEqual({
    operationId: original.request.operationId,
    storeId: A,
  });
  await queryReply(page, original);
  await expect
    .poll(() => page.evaluate(() => window.gatewayOps.checks.length))
    .toBe(2);
  expect(await page.evaluate(() => window.gatewayOps.checks[1].args)).toEqual({
    operationId: original.compensationOperationId,
    storeId: A,
  });
  await page.evaluate(() =>
    window.gatewayOps.checks[1].reject("operation_not_found", false),
  );
  await region(page)
    .getByRole("button", { name: "撤销本次签发", exact: true })
    .click();
  await page
    .getByRole("button", { name: "确认撤销本次签发", exact: true })
    .click();
  await expect
    .poll(() => page.evaluate(() => window.gatewayOps.revokes.length))
    .toBe(1);
  const request = {
    storeId: A,
    operationId: original.compensationOperationId,
    action: { kind: "revoke", clientId: original.request.operationId },
  };
  expect(
    await page.evaluate(() => window.gatewayOps.revokes[0].args.request),
  ).toEqual(request);
  await page.evaluate(() => {
    const p = window.gatewayOps.revokes[0];
    p.resolve({ status: "alreadyRevoked", request: p.args.request });
  });
  await expect.poll(async () => (await records(page)).requests.length).toBe(0);
  await expect(page.getByText(/原补偿编号回执未确认/)).toBeVisible();
  expect(await page.evaluate(() => window.gatewayOps.writes.length)).toBe(0);
});
test("a malformed or foreign receipt keeps the original pending request", async ({
  page,
}) => {
  await open(page);
  await seed(page);
  await region(page).getByRole("button", { name: "核查原客户端操作" }).click();
  await expect
    .poll(() => page.evaluate(() => window.gatewayOps.checks.length))
    .toBe(1);
  await page.evaluate(
    (row) =>
      window.gatewayOps.checks[0].resolve(
        window.gatewayOps.outcome({
          ...row.request,
          storeId: "00000000-0000-4000-8000-000000000002",
        }),
      ),
    original,
  );
  await expect(
    page.getByRole("alert").filter({ hasText: "原结果不可核对" }),
  ).toBeVisible();
  expect((await records(page)).requests).toEqual([original]);
  expect(await page.evaluate(() => window.gatewayOps.checks.length)).toBe(1);
});
test("an unsent draft is bound before submission and cannot move to another database", async ({
  page,
}) => {
  await open(page);
  await fill(page, "原库草稿");
  await page.evaluate((store) => window.gatewayOps.publish(store), B);
  await expect(page.getByText(/客户端草稿属于原后台/)).toBeVisible();
  await expect(createButton(page)).toBeDisabled();
  await expect(page.getByLabel("客户端名称", { exact: true })).toHaveValue(
    "原库草稿",
  );
  await createButton(page)
    .locator("xpath=ancestor::form")
    .evaluate((form) =>
      form.dispatchEvent(
        new Event("submit", { bubbles: true, cancelable: true }),
      ),
    );
  expect(await page.evaluate(() => window.gatewayOps.writes.length)).toBe(0);
  await page.getByRole("button", { name: "放弃未发送的客户端草稿" }).click();
  await fill(page, "新库草稿");
  await createButton(page).click();
  await expect
    .poll(() => page.evaluate(() => window.gatewayOps.writes.length))
    .toBe(1);
  expect(
    (
      await page.evaluate(
        () => window.gatewayOps.writes[0].args.input as PendingGatewayOperation,
      )
    ).request.storeId,
  ).toBe(B);
});
test("late saved replies after unmount do not clear a new database draft or erase the original recovery record", async ({
  page,
}) => {
  const row = await start(page);
  await page.getByRole("button", { name: "切换客户端挂载" }).click();
  await page.evaluate((store) => window.gatewayOps.publish(store), B);
  await page.getByRole("button", { name: "切换客户端挂载" }).click();
  await fill(page, "新库保留");
  await saveReply(page, "saved");
  await expect(page.getByLabel("客户端名称", { exact: true })).toHaveValue(
    "新库保留",
  );
  expect((await records(page)).requests).toEqual([row]);
  await expect(
    region(page).getByRole("button", { name: "重试原客户端操作" }),
  ).toBeDisabled();
  expect(await page.evaluate(() => window.gatewayOps.checks.length)).toBe(0);
});
test("recovery queries remain available without a root snapshot or a current status", async ({
  page,
}) => {
  await open(page);
  const row: PendingGatewayOperation = {
    request: {
      storeId: A,
      operationId: original.request.operationId,
      action: { kind: "revoke", clientId: gatewayClientId },
    },
  };
  await seed(page, row);
  await page.evaluate(() => {
    window.gatewayOps.available(false);
    window.gatewayOps.failStatus = true;
  });
  await page.getByRole("button", { name: "刷新状态" }).click();
  await expect(page.getByText(/fixture status unavailable/)).toBeVisible();
  await region(page).getByRole("button", { name: "核查原客户端操作" }).click();
  await expect
    .poll(() => page.evaluate(() => window.gatewayOps.checks.length))
    .toBe(1);
  await queryReply(page, row);
  await expect.poll(async () => (await records(page)).requests.length).toBe(0);
  expect(await page.evaluate(() => window.gatewayOps.revokes.length)).toBe(0);
});
test("storage write failure prevents any native mutation and preserves unrelated bytes", async ({
  page,
}) => {
  await open(page);
  await fill(page);
  await page.evaluate(() => {
    localStorage.setItem("unrelated.fixture", "original");
    window.gatewayOps.storage("set");
  });
  await createButton(page).click();
  await expect(
    page.getByRole("alert").filter({ hasText: "无法可靠保留客户端原请求" }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => [
      window.gatewayOps.writes.length,
      window.gatewayOps.revokes.length,
    ]),
  ).toEqual([0, 0]);
  expect(
    await page.evaluate(() => localStorage.getItem("unrelated.fixture")),
  ).toBe("original");
});
test("storage unavailable and malformed records disable new operations without deleting bytes", async ({
  page,
}) => {
  await open(page);
  const bad = '{"version":99,"request":{"secret":"fixture-must-stay"}}';
  await page.evaluate(
    ({ key, bad }) => {
      localStorage.setItem(key, bad);
      window.dispatchEvent(new Event("gateway-operations-changed"));
    },
    { key: prefix + original.request.operationId, bad },
  );
  await expect(createButton(page)).toBeDisabled();
  expect(
    await page.evaluate(
      (key) => localStorage.getItem(key),
      prefix + original.request.operationId,
    ),
  ).toBe(bad);
  await page.evaluate(() => window.gatewayOps.storage("unavailable"));
  await expect(
    region(page).getByText("客户端恢复记录暂不可用或已达上限", { exact: true }),
  ).toBeVisible();
  expect((await records(page)).available).toBe(false);
  expect(await page.evaluate(() => window.gatewayOps.writes.length)).toBe(0);
});
test("successful save remains confirmed if local cleanup fails and cleanup can be retried without another native call", async ({
  page,
}) => {
  const row = await start(page);
  await page.evaluate(() => window.gatewayOps.storage("remove"));
  await saveReply(page, "saved");
  await expect(
    page.getByText(/连接文件已保存为 client.json.*未能移除/),
  ).toBeVisible();
  expect((await records(page)).requests).toEqual([row]);
  await expect(
    region(page).getByRole("button", { name: "重试原客户端操作" }),
  ).toBeDisabled();
  await page.evaluate(() => window.gatewayOps.storage("none"));
  await region(page)
    .getByRole("button", { name: "移除已确认的恢复记录" })
    .click();
  await expect.poll(async () => (await records(page)).requests.length).toBe(0);
  expect(
    await page.evaluate(() => [
      window.gatewayOps.writes.length,
      window.gatewayOps.revokes.length,
      window.gatewayOps.checks.length,
    ]),
  ).toEqual([1, 0, 0]);
});
test("old protocol remains readable but never falls back to unscoped mutation", async ({
  page,
}) => {
  await open(page);
  await page.evaluate(() => {
    window.gatewayOps.legacy = true;
  });
  await page.getByRole("button", { name: "刷新状态" }).click();
  await expect(page.getByText(/客户端来源或回执协议尚未确认/)).toBeVisible();
  await expect(
    page.getByRole("button", { name: "撤销密钥", exact: true }),
  ).toBeDisabled();
  await expect(createButton(page)).toBeDisabled();
  expect(
    await page.evaluate(
      () =>
        window.gatewayOps.calls.filter((c) =>
          /^(create_gateway_client|revoke_gateway_client|save_gateway_client_operation|commit_gateway_client_operation)$/.test(
            c.name,
          ),
        ).length,
    ),
  ).toBe(0);
});
test("all 16 recovery rows remain visible and preserve keyboard focus in a narrow dark context", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 900 });
  await page.emulateMedia({ colorScheme: "dark" });
  await open(page);
  await page.evaluate((row) => {
    for (let n = 1; n <= 16; n++) {
      const value = structuredClone(row);
      value.request.operationId = `11111111-1111-4111-8111-${String(n).padStart(12, "0")}`;
      value.request.storeId = `00000000-0000-4000-8000-${String(n + 100).padStart(12, "0")}`;
      value.compensationOperationId = `22222222-2222-4222-8222-${String(n).padStart(12, "0")}`;
      window.gatewayOps.remember(value);
    }
  }, original);
  const checks = region(page).getByRole("button", { name: "核查原客户端操作" });
  await expect(checks).toHaveCount(16);
  await expect(createButton(page)).toBeDisabled();
  await checks.last().focus();
  await page.evaluate(() =>
    window.dispatchEvent(new Event("gateway-operations-changed")),
  );
  await expect(checks.last()).toBeFocused();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth + 1,
    ),
  ).toBe(true);
  expect(await page.evaluate(() => window.gatewayOps.checks.length)).toBe(0);
});

test("revocation dialog and durable intent remain bound to their original database before confirmation", async ({
  page,
}) => {
  await open(page);
  await page.getByRole("button", { name: "撤销密钥", exact: true }).click();
  expect(await page.evaluate(() => window.gatewayOps.revokes.length)).toBe(0);
  expect((await records(page)).requests).toHaveLength(0);
  await page.evaluate((store) => window.gatewayOps.publish(store), B);
  await expect(
    page.getByRole("button", { name: "确认撤销", exact: true }),
  ).toBeDisabled();
  await page.evaluate((store) => window.gatewayOps.publish(store), A);
  await page.getByRole("button", { name: "确认撤销", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.gatewayOps.revokes.length))
    .toBe(1);
  const native = await page.evaluate(() => {
    const { args, persisted } = window.gatewayOps.revokes[0];
    return { args, persisted };
  });
  const request = native.args.request as PendingGatewayOperation["request"];
  expect(request.storeId).toBe(A);
  expect(request.action).toEqual({ kind: "revoke", clientId: gatewayClientId });
  expect(native.persisted.requests).toEqual([{ request }]);
  await page.evaluate(() => window.gatewayOps.revokes[0].reject());
  await expect(
    page.getByRole("alert").filter({ hasText: "transport_timeout" }),
  ).toBeVisible();
  await region(page).getByRole("button", { name: "核查原客户端操作" }).click();
  await expect
    .poll(() => page.evaluate(() => window.gatewayOps.checks.length))
    .toBe(1);
  await queryReply(page, { request });
  await expect.poll(async () => (await records(page)).requests.length).toBe(0);
  expect(await page.evaluate(() => window.gatewayOps.writes.length)).toBe(0);
});

test("a compensated file failure is confirmed as revocation while preserving the unsent correction draft", async ({
  page,
}) => {
  await start(page);
  await page.evaluate(() => {
    const native = window.gatewayOps.writes[0],
      row = native.args.input as PendingGatewayOperation;
    const compensation = {
      storeId: row.request.storeId,
      operationId: row.compensationOperationId!,
      action: { kind: "revoke" as const, clientId: row.request.operationId },
    };
    native.resolve({
      status: "revoked",
      outcome: window.gatewayOps.outcome(row.request),
      compensation: {
        status: "committed",
        outcome: window.gatewayOps.outcome(compensation),
      },
      reason: "fixture file failure；原客户端撤销回执已确认",
      sourceInstanceId: row.request.storeId,
      sourceCurrent: true,
    });
  });
  await expect.poll(async () => (await records(page)).requests.length).toBe(0);
  await expect(
    page.getByText(/fixture file failure.*撤销回执已确认/),
  ).toBeVisible();
  await expect(page.getByLabel("客户端名称", { exact: true })).toHaveValue(
    "新客户端",
  );
  expect(
    await page.evaluate(() => [
      window.gatewayOps.writes.length,
      window.gatewayOps.revokes.length,
      window.gatewayOps.checks.length,
    ]),
  ).toEqual([1, 0, 0]);
});
