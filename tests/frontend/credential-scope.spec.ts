import { test, expect, type Page } from "@playwright/test";
import type {} from "./credential-api-harness";
import type {} from "./provider-auto-connect-harness";
import {
  PROVIDER_FIXTURE_STORE_ID as A,
  PROVIDER_FIXTURE_OTHER_STORE_ID as B,
} from "./provider-operation-fixture";
const ID = "cf45b4a0-4dac-4bd5-b276-3181d3e1514e";
const PREFIX = "local-agent.credential-operation.v2.";
async function create(page: Page, operationId = ID) {
  return page.evaluate(
    async ({ storeId, operationId }) => {
      try {
        await window.credentialClient.createScopedCredential(
          storeId,
          "Fixture",
          "disposable-scoped-key",
          operationId,
        );
        return "saved";
      } catch (reason) {
        return reason && typeof reason === "object" && "code" in reason
          ? String(reason.code)
          : String(reason);
      }
    },
    { storeId: A, operationId },
  );
}
for (const [mode, code] of [
  ["scoped-unsupported", "credential_protocol_unavailable"],
  ["scoped-foreign", "credential_protocol_unavailable"],
])
  test(`${mode} refuses to send a credential before original-source support is verified`, async ({
    page,
  }) => {
    await page.goto("/tests/frontend/credential-api.html?mode=" + mode);
    expect(await create(page)).toBe(code);
    const commands = await page.evaluate(() => window.credentialCommands);
    expect(commands.map((row) => row.name)).toEqual([
      "get_provider_credential_protocol",
    ]);
    expect(JSON.stringify(commands)).not.toContain("disposable-scoped-key");
  });
for (const mode of [
  "scoped-bad-version",
  "scoped-bad-id",
  "scoped-bad-reference",
  "scoped-bad-status",
])
  test(`${mode} never becomes a confirmed scoped credential`, async ({
    page,
  }) => {
    await page.goto("/tests/frontend/credential-api.html?mode=" + mode);
    expect(await create(page)).toBe("transport_invalid_response");
  });
test("a valid scoped receipt keeps store and operation identity on create, query and cancel", async ({
  page,
}) => {
  await page.goto("/tests/frontend/credential-api.html?mode=scoped-ok");
  expect(await create(page)).toBe("saved");
  const result = await page.evaluate(
    async ({ storeId, id }) => [
      await window.credentialClient.scopedCredentialOperation(storeId, id),
      await window.credentialClient.cancelScopedCredentialOperation(
        storeId,
        id,
      ),
    ],
    { storeId: A, id: ID },
  );
  expect(
    result.every(
      (row) =>
        row.storeId === A &&
        row.receipt?.storeId === A &&
        row.operationId === ID,
    ),
  ).toBe(true);
  const commands = await page.evaluate(() => window.credentialCommands);
  expect(
    commands
      .filter((row) => row.name != "get_provider_credential_protocol")
      .map((row) => row.args),
  ).toEqual([
    {
      storeId: A,
      operationId: ID,
      label: "Fixture",
      secret: "disposable-scoped-key",
    },
    { storeId: A, operationId: ID },
    { storeId: A, operationId: ID },
  ]);
});
test("invalid scoped identity is rejected before even a protocol request", async ({
  page,
}) => {
  await page.goto("/tests/frontend/credential-api.html");
  expect(await create(page, "bad-id")).toBe("invalid_credential_operation");
  expect(await page.evaluate(() => window.credentialCommands)).toHaveLength(0);
});
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
    .fill("disposable-scoped-key");
  await page.getByRole("button", { name: "连接并保存", exact: true }).click();
}
async function markers(page: Page) {
  return page.evaluate(
    (prefix) =>
      Object.entries(localStorage).filter(
        ([key]) =>
          key.startsWith(prefix) ||
          key.startsWith("local-agent.credential-operation.v3."),
      ),
    PREFIX,
  );
}
for (const source of ["unknown", "changed"])
  test(`a ${source} source suppresses a late credential result and retains original recovery data`, async ({
    page,
  }) => {
    await begin(page, "credential-hold");
    await expect
      .poll(() =>
        page.evaluate(() => window.autoConnectFixture.credentials.length),
      )
      .toBe(1);
    await page.evaluate(
      ({ source, b }) => {
        if (source === "changed")
          window.providerOperationsFixture.switchStore(b);
        window.autoConnectFixture.sourceChange?.(
          source === "unknown" ? undefined : b,
        );
      },
      { source, b: B },
    );
    await expect(
      page.getByText("配置来源尚未核对", { exact: true }),
    ).toBeVisible();
    await page.evaluate(() => window.autoConnectFixture.release?.());
    await expect(
      page.getByRole("button", { name: "采用到当前连接", exact: true }),
    ).toHaveCount(0);
    expect(
      await page.evaluate(() => window.autoConnectFixture.connections),
    ).toHaveLength(0);
    const rows = await markers(page);
    expect(rows).toHaveLength(1);
    expect(JSON.parse(rows[0][1]).storeId).toBe(A);
    expect(JSON.stringify(rows)).not.toContain("disposable-scoped-key");
  });
test("reopening another database cannot query, cancel or adopt a foreign pending credential", async ({
  page,
}) => {
  await begin(page, "credential-query");
  await expect(page.getByText("凭据结果待核查", { exact: true })).toBeVisible();
  const original = await markers(page);
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=credential-origin-b",
  );
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  await expect(
    page.getByText("属于其他后台，未采用", { exact: true }),
  ).toBeVisible();
  for (const name of [
    "核查保存结果",
    "取消未完成保存",
    "采用到当前连接",
    "用同一密钥重试",
  ])
    await expect(page.getByRole("button", { name, exact: true })).toHaveCount(
      0,
    );
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentialChecks),
  ).toHaveLength(0);
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentialCancels),
  ).toHaveLength(0);
  expect(await markers(page)).toEqual(original);
});
test("legacy unscoped reminders stay intact until explicit manual review and removal", async ({
  page,
}) => {
  const key = "local-agent.credential-operation.v1." + ID,
    value = JSON.stringify({
      version: 1,
      label: "Legacy fixture",
      cancelledPending: false,
    });
  await page.addInitScript(
    ({ key, value }) => localStorage.setItem(key, value),
    { key, value },
  );
  await page.goto("/tests/frontend/provider-auto-connect.html");
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  await expect(
    page.getByText("旧版记录：来源未绑定", { exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentialChecks),
  ).toHaveLength(0);
  await page
    .getByRole("button", { name: "人工核对后移除提示", exact: true })
    .click();
  await page.getByRole("button", { name: "保留提示", exact: true }).click();
  expect(await page.evaluate((key) => localStorage.getItem(key), key)).toBe(
    value,
  );
  await page
    .getByRole("button", { name: "人工核对后移除提示", exact: true })
    .click();
  await page
    .getByRole("button", { name: "已人工核对，移除提示", exact: true })
    .click();
  expect(
    await page.evaluate((key) => localStorage.getItem(key), key),
  ).toBeNull();
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentialCancels),
  ).toHaveLength(0);
});
test("a failed scoped recovery write prevents credential creation", async ({
  page,
}) => {
  await page.addInitScript((prefix) => {
    const set = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (
        key.startsWith(prefix) ||
        key.startsWith("local-agent.credential-operation.v3.")
      )
        throw Error("fixture recovery quota");
      return set.call(this, key, value);
    };
  }, PREFIX);
  await begin(page, "normal");
  await expect(page.getByText(/fixture recovery quota/)).toBeVisible();
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(0);
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections),
  ).toHaveLength(0);
});
test("a late explicit credential query cannot become an adopted receipt after source loss", async ({
  page,
}) => {
  await page.addInitScript(
    ({ prefix, a, id }) =>
      localStorage.setItem(
        prefix + a + "." + id,
        JSON.stringify({
          version: 2,
          storeId: a,
          operationId: id,
          label: "新连接 API Key",
          cancelledPending: false,
        }),
      ),
    { prefix: PREFIX, a: A, id: ID },
  );
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=credential-check-hold",
  );
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  await page.getByRole("button", { name: "核查保存结果", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => !!window.autoConnectFixture.release))
    .toBe(true);
  await page.evaluate(() =>
    window.autoConnectFixture.sourceChange?.(undefined),
  );
  await expect(
    page.getByText("配置来源尚未核对", { exact: true }),
  ).toBeVisible();
  await page.evaluate(() => window.autoConnectFixture.release?.());
  await expect(
    page.getByRole("button", { name: "采用到当前连接", exact: true }),
  ).toHaveCount(0);
  expect(await markers(page)).toHaveLength(1);
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections),
  ).toHaveLength(0);
});

for (const malformed of ["state-type", "extra-key-part"])
  test(`malformed scoped recovery ${malformed} stays preserved and cannot issue actions`, async ({
    page,
  }) => {
    const key =
      PREFIX +
      A +
      "." +
      ID +
      (malformed === "extra-key-part" ? ".unexpected" : "");
    const raw = JSON.stringify({
      version: 2,
      storeId: A,
      operationId: ID,
      label: "Fixture",
      cancelledPending: malformed === "state-type" ? "yes" : false,
    });
    await page.addInitScript(({ key, raw }) => localStorage.setItem(key, raw), {
      key,
      raw,
    });
    await page.goto("/tests/frontend/provider-auto-connect.html");
    await page
      .getByRole("button", { name: "API Key 保存与恢复", exact: true })
      .click();
    await expect(page.getByText("记录无法核查", { exact: true })).toBeVisible();
    await expect(
      page.getByRole("button", { name: "核查保存结果", exact: true }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: "保存连接", exact: true }),
    ).toBeDisabled();
    expect(await page.evaluate((key) => localStorage.getItem(key), key)).toBe(
      raw,
    );
    expect(
      await page.evaluate(() => window.autoConnectFixture.credentialChecks),
    ).toHaveLength(0);
  });
