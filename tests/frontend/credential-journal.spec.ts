import { test, expect, type Page } from "@playwright/test";
import type {} from "./credential-api-harness";
import type {} from "./provider-auto-connect-harness";
import {
  PROVIDER_FIXTURE_STORE_ID as A,
  PROVIDER_FIXTURE_OTHER_STORE_ID as B,
  PROVIDER_FIXTURE_NAMESPACE_ID as N,
  PROVIDER_FIXTURE_OTHER_NAMESPACE_ID as M,
} from "./provider-operation-fixture";
const ID = "cf45b4a0-4dac-4bd5-b276-3181d3e1514e";
const PREFIX = "local-agent.credential-operation.v3.";
const request = {
  storeId: A,
  namespaceId: N,
  operationId: ID,
  label: "Fixture",
  secret: "disposable-journal-key",
};
async function submit(page: Page) {
  return page.evaluate(async (request) => {
    try {
      return await window.credentialClient.createCredentialV3(request);
    } catch (reason) {
      return reason && typeof reason === "object" && "code" in reason
        ? String(reason.code)
        : String(reason);
    }
  }, request);
}
for (const [mode, code] of [
  ["journal-unsupported", "credential_protocol_unavailable"],
  ["journal-future-protocol", "credential_protocol_unavailable"],
  ["journal-foreign-store", "credential_protocol_unavailable"],
  ["journal-bad-limit", "credential_protocol_unavailable"],
  ["journal-foreign-namespace", "credential_namespace_changed"],
  ["journal-unverified", "credential_namespace_unverified"],
])
  test(`${mode}: v3 preflight prevents secret transmission and fallback`, async ({
    page,
  }) => {
    await page.goto("/tests/frontend/credential-api.html?mode=" + mode);
    expect(await submit(page)).toBe(code);
    const commands = await page.evaluate(() => window.credentialCommands);
    expect(commands.map((row) => row.name)).toEqual([
      "get_provider_credential_protocol_v3",
    ]);
    expect(JSON.stringify(commands)).not.toContain(request.secret);
  });
for (const mode of [
  "journal-bad-version",
  "journal-bad-store",
  "journal-bad-namespace",
  "journal-bad-id",
  "journal-bad-reference",
  "journal-bad-status",
  "journal-bad-label",
  "journal-extra-field",
])
  test(`${mode}: malformed original-namespace receipt stays unconfirmed`, async ({
    page,
  }) => {
    await page.goto("/tests/frontend/credential-api.html?mode=" + mode);
    expect(await submit(page)).toBe("transport_invalid_response");
    expect(
      (await page.evaluate(() => window.credentialCommands)).map(
        (row) => row.name,
      ),
    ).toEqual([
      "get_provider_credential_protocol_v3",
      "create_provider_credential_v3",
    ]);
  });
test("v3 create query cancel and initialization retain original source and namespace", async ({
  page,
}) => {
  await page.goto("/tests/frontend/credential-api.html?mode=journal-ok");
  expect(await submit(page)).toMatchObject({
    version: 3,
    storeId: A,
    namespaceId: N,
    operationId: ID,
  });
  const result = await page.evaluate(
    async ({ storeId, namespaceId, operationId }) => {
      const identity = { storeId, namespaceId, operationId };
      return [
        await window.credentialClient.credentialOperationV3(identity),
        await window.credentialClient.cancelCredentialOperationV3(identity),
        await window.credentialClient.initializeCredentialNamespace({
          storeId,
          namespaceId,
          choice: "restore",
        }),
      ];
    },
    request,
  );
  expect(
    result.every(
      (row) => row.version === 3 && row.storeId === A && row.namespaceId === N,
    ),
  ).toBe(true);
  expect(result[2]).toMatchObject({
    namespaceState: "active",
    legacyState: "imported",
  });
  expect(
    (await page.evaluate(() => window.credentialCommands)).map(
      (row) => row.name,
    ),
  ).toEqual([
    "get_provider_credential_protocol_v3",
    "create_provider_credential_v3",
    "get_provider_credential_protocol_v3",
    "get_provider_credential_operation_v3",
    "get_provider_credential_protocol_v3",
    "cancel_provider_credential_operation_v3",
    "get_provider_credential_protocol_v3",
    "initialize_provider_credential_namespace",
  ]);
});
const oldRaw = JSON.stringify({
  version: 2,
  storeId: A,
  operationId: ID,
  label: "Old pending",
  cancelledPending: false,
});
async function seedOld(page: Page) {
  await page.addInitScript(
    ({ a, id, raw }) =>
      localStorage.setItem(
        "local-agent.credential-operation.v2." + a + "." + id,
        raw,
      ),
    { a: A, id: ID, raw: oldRaw },
  );
}
async function initCommands(page: Page) {
  return page.evaluate(() =>
    window.providerOperationsFixture.commands.filter(
      (row) => row.name === "initialize_provider_credential_namespace",
    ),
  );
}
async function keyActions(page: Page) {
  return page.evaluate(() => ({
    credentials: window.autoConnectFixture.credentials.length,
    connections: window.autoConnectFixture.connections.length,
    checks: window.autoConnectFixture.credentialChecks.length,
    cancels: window.autoConnectFixture.credentialCancels.length,
  }));
}
test("unverified namespace is read only until explicit confirmation; cancellation preserves records", async ({
  page,
}) => {
  await seedOld(page);
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=credential-unverified",
  );
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  await expect(
    page.getByText("凭据恢复空间尚未核查", { exact: true }),
  ).toBeVisible();
  expect(await initCommands(page)).toHaveLength(0);
  await page
    .getByRole("button", { name: "建立新的恢复空间", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.getByRole("button", { name: "暂不处理", exact: true }).click();
  expect(await initCommands(page)).toHaveLength(0);
  expect(await keyActions(page)).toEqual({
    credentials: 0,
    connections: 0,
    checks: 0,
    cancels: 0,
  });
  expect(
    await page.evaluate(
      ({ a, id }) =>
        localStorage.getItem(
          "local-agent.credential-operation.v2." + a + "." + id,
        ),
      { a: A, id: ID },
    ),
  ).toBe(oldRaw);
});
for (const mode of ["credential-unverified", "credential-unverified-init-lost"])
  test(`${mode}: explicit new space preserves old reminders and never sends or replays keys`, async ({
    page,
  }) => {
    await seedOld(page);
    await page.goto("/tests/frontend/provider-auto-connect.html?mode=" + mode);
    await page
      .getByRole("button", { name: "API Key 保存与恢复", exact: true })
      .click();
    await page
      .getByRole("button", { name: "建立新的恢复空间", exact: true })
      .click();
    await page
      .getByRole("button", { name: "确认建立新空间", exact: true })
      .click();
    await expect(
      page.getByRole("dialog", {
        name: "确认建立新的凭据恢复空间",
        exact: true,
      }),
    ).toHaveCount(0);
    await expect(
      page.getByText("旧凭据恢复材料尚未核查", { exact: true }),
    ).toBeVisible();
    expect(await initCommands(page)).toHaveLength(1);
    expect(await keyActions(page)).toEqual({
      credentials: 0,
      connections: 0,
      checks: 0,
      cancels: 0,
    });
    const state = await page.evaluate(() =>
      window.providerOperationsFixture.snapshot(),
    );
    expect(state.stores[A].namespaceId).toBe(N);
    expect(state.stores[A].namespaceState).toBe("active");
    expect(state.stores[A].legacyState).toBe("unverified");
    expect(
      await page.evaluate(
        ({ a, id }) =>
          localStorage.getItem(
            "local-agent.credential-operation.v2." + a + "." + id,
          ),
        { a: A, id: ID },
      ),
    ).toBe(oldRaw);
  });
test("failed restore remains unverified with its original reminder and no key actions", async ({
  page,
}) => {
  await seedOld(page);
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=credential-unverified-restore-error",
  );
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  await page
    .getByRole("button", { name: "核查原恢复材料", exact: true })
    .click();
  await page
    .getByRole("button", { name: "确认核查原材料", exact: true })
    .click();
  await expect(
    page
      .getByRole("dialog")
      .getByText("原恢复材料暂不可用，已保留原记录", { exact: true }),
  ).toBeVisible();
  expect(await initCommands(page)).toHaveLength(1);
  expect(await keyActions(page)).toEqual({
    credentials: 0,
    connections: 0,
    checks: 0,
    cancels: 0,
  });
  expect(
    (await page.evaluate(() => window.providerOperationsFixture.snapshot()))
      .stores[A].namespaceState,
  ).toBe("unverified");
});
test("a changed namespace freezes an already-open initialization confirmation", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=credential-unverified",
  );
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  await page
    .getByRole("button", { name: "建立新的恢复空间", exact: true })
    .click();
  await page.evaluate(
    (m) => window.providerOperationsFixture.switchNamespace(m),
    M,
  );
  await expect(
    page.getByRole("button", { name: "确认建立新空间", exact: true }),
  ).toBeDisabled();
  expect(await initCommands(page)).toHaveLength(0);
  expect(await keyActions(page)).toEqual({
    credentials: 0,
    connections: 0,
    checks: 0,
    cancels: 0,
  });
});
test("late initialization after a database change cannot authorize the active connection", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=credential-unverified-init-hold",
  );
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  await page
    .getByRole("button", { name: "建立新的恢复空间", exact: true })
    .click();
  await page
    .getByRole("button", { name: "确认建立新空间", exact: true })
    .click();
  await expect
    .poll(() => initCommands(page).then((rows) => rows.length))
    .toBe(1);
  await page.evaluate((b) => {
    window.providerOperationsFixture.switchStore(b);
    window.autoConnectFixture.sourceChange?.(b);
  }, B);
  await page.evaluate(() => window.providerOperationsFixture.release());
  await expect(
    page.getByText("配置来源尚未核对", { exact: true }),
  ).toBeVisible();
  expect(await keyActions(page)).toEqual({
    credentials: 0,
    connections: 0,
    checks: 0,
    cancels: 0,
  });
});
test("original v3 records cannot be queried or adopted in a different namespace", async ({
  page,
}) => {
  const key = PREFIX + A + "." + M + "." + ID;
  const raw = JSON.stringify({
    version: 3,
    storeId: A,
    namespaceId: M,
    operationId: ID,
    label: "Old space",
    cancelledPending: false,
  });
  await page.addInitScript(({ key, raw }) => localStorage.setItem(key, raw), {
    key,
    raw,
  });
  await page.goto("/tests/frontend/provider-auto-connect.html");
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  await expect(
    page.getByText("属于之前的恢复空间，未采用", { exact: true }),
  ).toBeVisible();
  for (const name of [
    "核查保存结果",
    "用同一密钥重试",
    "取消未完成保存",
    "采用到当前连接",
  ])
    await expect(page.getByRole("button", { name, exact: true })).toHaveCount(
      0,
    );
  expect(await keyActions(page)).toEqual({
    credentials: 0,
    connections: 0,
    checks: 0,
    cancels: 0,
  });
  expect(await page.evaluate((key) => localStorage.getItem(key), key)).toBe(
    raw,
  );
});
test("v2 and v3 reminders with the same operation ID remain separate table rows", async ({
  page,
}) => {
  await seedOld(page);
  await page.addInitScript(
    ({ prefix, a, n, id }) =>
      localStorage.setItem(
        prefix + a + "." + n + "." + id,
        JSON.stringify({
          version: 3,
          storeId: a,
          namespaceId: n,
          operationId: id,
          label: "New pending",
          cancelledPending: false,
        }),
      ),
    { prefix: PREFIX, a: A, n: N, id: ID },
  );
  await page.goto("/tests/frontend/provider-auto-connect.html");
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  await expect(
    page.getByRole("cell", { name: "Old pending", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("cell", { name: "New pending", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "核查保存结果", exact: true }),
  ).toHaveCount(2);
  expect(
    await page.evaluate(
      (prefix) =>
        Object.keys(localStorage).filter(
          (key) =>
            key.startsWith(prefix) ||
            key.startsWith("local-agent.credential-operation.v2."),
        ).length,
      PREFIX,
    ),
  ).toBe(2);
});

test("explicit restore enables original v2 checks without automatic key requests or v3 conversion", async ({
  page,
}) => {
  await seedOld(page);
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=credential-unverified",
  );
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  await page
    .getByRole("button", { name: "核查原恢复材料", exact: true })
    .click();
  await page
    .getByRole("button", { name: "确认核查原材料", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "核查保存结果", exact: true }),
  ).toHaveCount(1);
  expect(await keyActions(page)).toEqual({
    credentials: 0,
    connections: 0,
    checks: 0,
    cancels: 0,
  });
  expect(
    (await page.evaluate(() => window.providerOperationsFixture.snapshot()))
      .stores[A].legacyState,
  ).toBe("imported");
  expect(
    await page.evaluate(
      (prefix) =>
        Object.keys(localStorage).filter((key) => key.startsWith(prefix))
          .length,
      PREFIX,
    ),
  ).toBe(0);
});
test("namespace recovery confirmation supports narrow dark layout and keyboard cancellation", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=credential-unverified",
  );
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  const open = page.getByRole("button", {
    name: "建立新的恢复空间",
    exact: true,
  });
  await open.focus();
  await page.keyboard.press("Enter");
  const dialog = page.getByRole("dialog", {
    name: "确认建立新的凭据恢复空间",
    exact: true,
  });
  await expect(dialog).toBeVisible();
  const cancel = page.getByRole("button", { name: "暂不处理", exact: true });
  await cancel.focus();
  await expect(cancel).toBeFocused();
  expect(
    await dialog.evaluate(
      (element) => element.scrollWidth <= element.clientWidth + 1,
    ),
  ).toBe(true);
  await page.keyboard.press("Enter");
  await expect(dialog).toHaveCount(0);
  expect(await initCommands(page)).toHaveLength(0);
});
