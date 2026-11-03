import { test, expect, type Page } from "@playwright/test";
import type {} from "./credential-api-harness";
import type {} from "./provider-auto-connect-harness";
import {
  PROVIDER_FIXTURE_STORE_ID as A,
  PROVIDER_FIXTURE_OTHER_STORE_ID as B,
  PROVIDER_FIXTURE_NAMESPACE_ID as N,
} from "./provider-operation-fixture";
const ID = "364a8375-9ec2-4c92-9b76-ed6e42294ab6",
  D = "a".repeat(64),
  PREFIX = "local-agent.credential-operation.v3.";
const identity = { storeId: A, namespaceId: N, operationId: ID };
const oldKey = "local-agent.credential-operation.v1." + ID;
const raw = JSON.stringify({
  version: 1,
  label: "Legacy fixture",
  cancelledPending: false,
});
async function start(page: Page, mode = "legacy-ok") {
  await page.addInitScript(({ key, raw }) => localStorage.setItem(key, raw), {
    key: oldKey,
    raw,
  });
  await page.goto("/tests/frontend/provider-auto-connect.html?mode=" + mode);
  await openRecovery(page);
}
async function openRecovery(page: Page) {
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
}
async function open(page: Page) {
  await page
    .getByRole("button", { name: "核查旧记录来源", exact: true })
    .click();
  return page.getByRole("dialog", {
    name: "确认旧凭据的恢复来源",
    exact: true,
  });
}
async function consent(page: Page) {
  await page
    .getByRole("checkbox", {
      name: "我确认这份旧记录属于当前连接的后台，同意保留原材料并按原引用恢复。",
      exact: true,
    })
    .check();
}
async function posts(page: Page) {
  return page.evaluate(() =>
    window.providerOperationsFixture.commands.filter(
      (row) => row.name === "confirm_legacy_provider_credential",
    ),
  );
}
async function markers(page: Page) {
  return page.evaluate(
    (prefix) =>
      Object.entries(localStorage).filter(([key]) => key.startsWith(prefix)),
    PREFIX,
  );
}
test("a legacy reply after confirmation remains unknown and cannot erase recovery evidence", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/credential-api.html?mode=legacy-api-old-confirm",
  );
  const result = await page.evaluate(async (identity) => {
    try {
      await window.credentialClient.confirmLegacyCredential({
        ...identity,
        sourceDigest: "a".repeat(64),
        confirmed: true,
      });
      return null;
    } catch (reason) {
      return {
        code: String((reason as { code: unknown }).code),
        outcomeUnknown: (reason as { outcomeUnknown: unknown }).outcomeUnknown,
      };
    }
  }, identity);
  expect(result).toEqual({
    code: "transport_invalid_response",
    outcomeUnknown: true,
  });
  expect(
    (await page.evaluate(() => window.credentialCommands)).filter(
      (row) => row.name === "confirm_legacy_provider_credential",
    ),
  ).toHaveLength(1);
});
test("a weak backend keeps the original local hint and offers no migration confirmation", async ({
  page,
}) => {
  await start(page, "legacy-weak-backend");
  await open(page);
  await expect(
    page.getByText("当前后台尚未提供旧凭据持久防重，未发送密钥或确认。", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "确认来源并恢复", exact: true }),
  ).toHaveCount(0);
  expect(await posts(page)).toHaveLength(0);
  expect(await markers(page)).toHaveLength(0);
  expect(await page.evaluate((key) => localStorage.getItem(key), oldKey)).toBe(
    raw,
  );
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(0);
});
for (const mode of [
  "legacy-api-future",
  "legacy-api-v1",
  "legacy-api-foreign",
  "legacy-api-bad-digest",
  "legacy-api-extra",
  "legacy-api-changed",
])
  test(`${mode}: legacy source validation cannot send confirmation or create a credential`, async ({
    page,
  }) => {
    await page.goto("/tests/frontend/credential-api.html?mode=" + mode);
    const code = await page.evaluate(
      async (request) => {
        try {
          await window.credentialClient.confirmLegacyCredential(request);
          return "saved";
        } catch (reason) {
          return reason && typeof reason === "object" && "code" in reason
            ? String(reason.code)
            : String(reason);
        }
      },
      { ...identity, sourceDigest: D, confirmed: true as const },
    );
    expect(code).toBe(
      mode === "legacy-api-changed"
        ? "credential_legacy_source_changed"
        : ["legacy-api-future", "legacy-api-v1"].includes(mode)
          ? "credential_protocol_unavailable"
          : "transport_invalid_response",
    );
    const commands = await page.evaluate(() => window.credentialCommands);
    expect(commands.map((row) => row.name)).toEqual([
      "get_provider_credential_protocol_v3",
      "preview_legacy_provider_credential",
    ]);
    expect(JSON.stringify(commands)).not.toContain("secret");
  });
test("valid legacy DTO keeps source and material identity; an uncommitted reply stays unknown", async ({
  page,
}) => {
  for (const mode of ["legacy-api-ok", "legacy-api-uncommitted"]) {
    await page.goto("/tests/frontend/credential-api.html?mode=" + mode);
    const result = await page.evaluate(
      async (request) => {
        try {
          return await window.credentialClient.confirmLegacyCredential(request);
        } catch (reason) {
          return reason && typeof reason === "object" && "code" in reason
            ? String(reason.code)
            : String(reason);
        }
      },
      { ...identity, sourceDigest: D, confirmed: true as const },
    );
    if (mode === "legacy-api-ok")
      expect(result).toMatchObject({
        ...identity,
        sourceDigest: D,
        stage: "committed",
      });
    else expect(result).toBe("transport_invalid_response");
    expect(
      (await page.evaluate(() => window.credentialCommands)).map(
        (row) => row.name,
      ),
    ).toEqual([
      "get_provider_credential_protocol_v3",
      "preview_legacy_provider_credential",
      "confirm_legacy_provider_credential",
    ]);
  }
});
test("legacy preview is read-only and confirmation requires explicit consent", async ({
  page,
}) => {
  await start(page);
  const dialog = await open(page);
  await expect(dialog).toBeVisible();
  await expect(
    page.getByRole("button", { name: "确认来源并恢复", exact: true }),
  ).toBeDisabled();
  expect(await posts(page)).toHaveLength(0);
  expect(await markers(page)).toHaveLength(0);
  expect(await page.evaluate((key) => localStorage.getItem(key), oldKey)).toBe(
    raw,
  );
  await page.getByRole("button", { name: "暂不处理", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(await posts(page)).toHaveLength(0);
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(0);
});
for (const mode of ["legacy-ok", "legacy-lost"])
  test(`${mode}: explicit migration restores a normal hint and original reference without business replay`, async ({
    page,
  }) => {
    await start(page, mode);
    await open(page);
    await consent(page);
    await page
      .getByRole("button", { name: "确认来源并恢复", exact: true })
      .click();
    await expect(
      page.getByRole("button", { name: "采用到当前连接", exact: true }),
    ).toBeVisible();
    expect(await posts(page)).toHaveLength(1);
    expect(
      await page.evaluate((key) => localStorage.getItem(key), oldKey),
    ).toBeNull();
    const saved = await markers(page);
    expect(saved).toHaveLength(1);
    expect(JSON.parse(saved[0][1])).toMatchObject({
      version: 3,
      ...identity,
      label: "Legacy fixture",
    });
    expect(JSON.parse(saved[0][1])).not.toHaveProperty("legacySourceDigest");
    expect(JSON.stringify(saved)).not.toContain("credentialRef");
    expect(
      await page.evaluate(() => window.autoConnectFixture.credentials),
    ).toHaveLength(0);
    expect(
      await page.evaluate(() => window.autoConnectFixture.connections),
    ).toHaveLength(0);
    await page
      .getByRole("button", { name: "采用到当前连接", exact: true })
      .click();
    expect(
      await page.evaluate(() => window.autoConnectFixture.connections),
    ).toHaveLength(0);
  });
test("an interrupted migration has recovery actions and cannot enter ordinary create or cancel", async ({
  page,
}) => {
  await start(page, "legacy-pending");
  const dialog = await open(page);
  await consent(page);
  await page
    .getByRole("button", { name: "确认来源并恢复", exact: true })
    .click();
  await expect(
    dialog.getByText("旧凭据迁移尚未完成，请继续恢复", { exact: true }),
  ).toBeVisible();
  expect(await posts(page)).toHaveLength(1);
  const saved = await markers(page);
  expect(JSON.parse(saved[0][1]).legacySourceDigest).toBe(D);
  expect(await page.evaluate((key) => localStorage.getItem(key), oldKey)).toBe(
    raw,
  );
  await page.getByRole("button", { name: "暂不处理", exact: true }).click();
  for (const name of ["取消未完成保存", "用同一密钥重试", "核查保存结果"])
    await expect(page.getByRole("button", { name, exact: true })).toHaveCount(
      0,
    );
  await page
    .getByRole("button", { name: "继续旧凭据恢复", exact: true })
    .click();
  await consent(page);
  await page
    .getByRole("button", { name: "确认来源并恢复", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "采用到当前连接", exact: true }),
  ).toBeVisible();
  expect(await posts(page)).toHaveLength(2);
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(0);
});
test("a pending migration hint survives reload without offering ordinary actions", async ({
  page,
}) => {
  await page.addInitScript(
    ({ identity, digest, prefix }) =>
      localStorage.setItem(
        prefix +
          identity.storeId +
          "." +
          identity.namespaceId +
          "." +
          identity.operationId,
        JSON.stringify({
          version: 3,
          ...identity,
          label: "Legacy fixture",
          cancelledPending: false,
          legacySourceDigest: digest,
        }),
      ),
    { identity, digest: D, prefix: PREFIX },
  );
  await page.goto("/tests/frontend/provider-auto-connect.html?mode=legacy-ok");
  await openRecovery(page);
  await expect(
    page.getByText("旧凭据迁移尚待核查", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText(
      "已连接原恢复空间。请继续核对旧记录来源并明确确认，再核查凭据保存结果。",
      { exact: true },
    ),
  ).toBeVisible();
  await expect(
    page.getByText(
      "请连接原恢复空间或恢复原数据库后核查，原请求不会自动转入新空间。",
      { exact: true },
    ),
  ).toHaveCount(0);
  for (const name of ["取消未完成保存", "用同一密钥重试", "核查保存结果"])
    await expect(page.getByRole("button", { name, exact: true })).toHaveCount(
      0,
    );
  expect(await posts(page)).toHaveLength(0);
  await page
    .getByRole("button", { name: "继续旧凭据恢复", exact: true })
    .click();
  await consent(page);
  await page
    .getByRole("button", { name: "确认来源并恢复", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "采用到当前连接", exact: true }),
  ).toBeVisible();
});
for (const source of [
  { storeId: B, namespaceId: N },
  { storeId: A, namespaceId: "518a44ac-79f8-4e1a-923a-82a3e7a4f14b" },
])
  test(`pending migration from ${source.storeId}.${source.namespaceId} keeps the original-space instruction`, async ({
    page,
  }) => {
    await page.addInitScript(
      ({ identity, source, digest, prefix }) => {
        const saved = { ...identity, ...source };
        localStorage.setItem(
          prefix +
            saved.storeId +
            "." +
            saved.namespaceId +
            "." +
            saved.operationId,
          JSON.stringify({
            version: 3,
            ...saved,
            label: "Legacy fixture",
            cancelledPending: false,
            legacySourceDigest: digest,
          }),
        );
      },
      { identity, source, digest: D, prefix: PREFIX },
    );
    await page.goto(
      "/tests/frontend/provider-auto-connect.html?mode=legacy-ok",
    );
    await openRecovery(page);
    await expect(
      page.getByText(
        "请连接原恢复空间或恢复原数据库后核查，原请求不会自动转入新空间。",
        { exact: true },
      ),
    ).toBeVisible();
    await expect(
      page.getByText(
        "已连接原恢复空间。请继续核对旧记录来源并明确确认，再核查凭据保存结果。",
        { exact: true },
      ),
    ).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: "继续旧凭据恢复", exact: true }),
    ).toBeDisabled();
    expect(await posts(page)).toHaveLength(0);
  });

test("source change while confirmation is open disables the original claim", async ({
  page,
}) => {
  await start(page);
  await open(page);
  await consent(page);
  await page.evaluate((b) => {
    window.providerOperationsFixture.switchStore(b);
    window.autoConnectFixture.sourceChange?.(b);
  }, B);
  await expect(
    page.getByRole("button", { name: "确认来源并恢复", exact: true }),
  ).toBeDisabled();
  expect(await posts(page)).toHaveLength(0);
  expect(await page.evaluate((key) => localStorage.getItem(key), oldKey)).toBe(
    raw,
  );
});
test("a late confirmed result from another source cannot adopt or clear its original hints", async ({
  page,
}) => {
  await start(page, "legacy-hold");
  await open(page);
  await consent(page);
  await page
    .getByRole("button", { name: "确认来源并恢复", exact: true })
    .click();
  await expect.poll(() => posts(page).then((rows) => rows.length)).toBe(1);
  await page.evaluate((b) => {
    window.providerOperationsFixture.switchStore(b);
    window.autoConnectFixture.sourceChange?.(b);
    window.providerOperationsFixture.release();
  }, B);
  await expect(
    page.getByText("配置来源尚未核对", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "采用到当前连接", exact: true }),
  ).toHaveCount(0);
  expect(await page.evaluate((key) => localStorage.getItem(key), oldKey)).toBe(
    raw,
  );
  expect(
    (await markers(page)).every(([, value]) => JSON.parse(value).storeId === A),
  ).toBe(true);
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections),
  ).toHaveLength(0);
});
test("local recovery storage failure prevents sending migration confirmation", async ({
  page,
}) => {
  await page.addInitScript((prefix) => {
    const set = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (key.startsWith(prefix)) throw Error("fixture migration quota");
      return set.call(this, key, value);
    };
  }, PREFIX);
  await start(page);
  const dialog = await open(page);
  await consent(page);
  await page
    .getByRole("button", { name: "确认来源并恢复", exact: true })
    .click();
  await expect(
    dialog.getByText("fixture migration quota", { exact: true }),
  ).toBeVisible();
  expect(await posts(page)).toHaveLength(0);
  expect(await markers(page)).toHaveLength(0);
  expect(await page.evaluate((key) => localStorage.getItem(key), oldKey)).toBe(
    raw,
  );
});
test("unknown target after a committed migration remains recoverable and unadopted", async ({
  page,
}) => {
  await start(page, "legacy-query-unavailable");
  await open(page);
  await consent(page);
  await page
    .getByRole("button", { name: "确认来源并恢复", exact: true })
    .click();
  await expect(
    page.getByRole("dialog", { name: "确认旧凭据的恢复来源", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "核查保存结果", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "采用到当前连接", exact: true }),
  ).toHaveCount(0);
  expect(await posts(page)).toHaveLength(1);
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(0);
  expect(await markers(page)).toHaveLength(1);
});
test("legacy confirmation fits a narrow dark window and supports keyboard cancellation", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await start(page);
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  const dialog = await open(page);
  await expect
    .poll(() => dialog.evaluate((element) => getComputedStyle(element).opacity))
    .toBe("1");
  expect(
    await dialog.evaluate(
      (element) => element.scrollWidth <= element.clientWidth + 1,
    ),
  ).toBe(true);
  const cancel = page.getByRole("button", { name: "暂不处理", exact: true });
  await cancel.focus();
  await page.keyboard.press("Enter");
  await expect(dialog).toHaveCount(0);
  expect(await posts(page)).toHaveLength(0);
});

test("full recovery capacity prevents starting a migration without a recoverable local hint", async ({
  page,
}) => {
  await page.addInitScript(
    ({ key, raw, other }) => {
      localStorage.setItem(key, raw);
      for (let n = 0; n < 15; n++) {
        const id = crypto.randomUUID();
        localStorage.setItem(
          "local-agent.credential-operation.v2." + other + "." + id,
          JSON.stringify({
            version: 2,
            storeId: other,
            operationId: id,
            label: "Other source",
            cancelledPending: false,
          }),
        );
      }
    },
    { key: oldKey, raw, other: B },
  );
  await page.goto("/tests/frontend/provider-auto-connect.html?mode=legacy-ok");
  await openRecovery(page);
  const dialog = await open(page);
  await consent(page);
  await page
    .getByRole("button", { name: "确认来源并恢复", exact: true })
    .click();
  await expect(
    dialog.getByText("恢复提示已满，请先核查并整理其他提示，再恢复旧记录。", {
      exact: true,
    }),
  ).toBeVisible();
  expect(await posts(page)).toHaveLength(0);
  expect(await markers(page)).toHaveLength(0);
  expect(await page.evaluate((key) => localStorage.getItem(key), oldKey)).toBe(
    raw,
  );
});
test("a changed local hint during confirmation is preserved after the backend commits", async ({
  page,
}) => {
  await start(page, "legacy-hold");
  const dialog = await open(page);
  await consent(page);
  await page
    .getByRole("button", { name: "确认来源并恢复", exact: true })
    .click();
  await expect.poll(() => posts(page).then((rows) => rows.length)).toBe(1);
  const rows = await markers(page);
  expect(rows).toHaveLength(1);
  const changed = JSON.stringify({
    ...JSON.parse(rows[0][1]),
    label: "Changed hint",
  });
  await page.evaluate(
    ({ key, changed }) => {
      localStorage.setItem(key, changed);
      window.providerOperationsFixture.release();
    },
    { key: rows[0][0], changed },
  );
  await expect(
    dialog.getByText(
      "后台已记录迁移结果，但本机提示已变化；原提示已保留，请重新核查。",
      { exact: true },
    ),
  ).toBeVisible();
  expect(
    await page.evaluate((key) => localStorage.getItem(key), rows[0][0]),
  ).toBe(changed);
  expect(await page.evaluate((key) => localStorage.getItem(key), oldKey)).toBe(
    raw,
  );
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentialChecks),
  ).toHaveLength(0);
  await expect(
    page.getByRole("button", { name: "采用到当前连接", exact: true }),
  ).toHaveCount(0);
});

test("legacy IPC projects exact request fields even when a structural object contains extra private data", async ({
  page,
}) => {
  await page.goto("/tests/frontend/credential-api.html?mode=legacy-api-ok");
  await page.evaluate(async (identity) => {
    const request = {
      ...identity,
      sourceDigest: "a".repeat(64),
      confirmed: true as const,
      version: 3,
      label: "Extra label",
      secret: "never-send-this-private-fixture",
    };
    await window.credentialClient.previewLegacyCredential(request);
    await window.credentialClient.confirmLegacyCredential(request);
  }, identity);
  const commands = await page.evaluate(() => window.credentialCommands);
  expect(JSON.stringify(commands)).not.toContain(
    "never-send-this-private-fixture",
  );
  expect(JSON.stringify(commands)).not.toContain("Extra label");
  for (const row of commands) {
    if (row.name === "preview_legacy_provider_credential")
      expect(
        Object.keys((row.args as { request: object }).request).sort(),
      ).toEqual(["namespaceId", "operationId", "storeId"]);
    if (row.name === "confirm_legacy_provider_credential")
      expect(
        Object.keys((row.args as { request: object }).request).sort(),
      ).toEqual([
        "confirmed",
        "namespaceId",
        "operationId",
        "sourceDigest",
        "storeId",
      ]);
  }
});
