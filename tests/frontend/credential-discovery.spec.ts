import { test, expect, type Page } from "@playwright/test";
import type {} from "./provider-auto-connect-harness";
import type {} from "./credential-api-harness";
import {
  PROVIDER_FIXTURE_STORE_ID as A,
  PROVIDER_FIXTURE_OTHER_STORE_ID as B,
  PROVIDER_FIXTURE_NAMESPACE_ID as N,
} from "./provider-operation-fixture";
import { DISCOVERY_FIXTURE_ID as ID } from "./credential-discovery-fixture";
const prefix = "local-agent.credential-operation.",
  key = `${prefix}v3.${A}.${N}.${ID}`;
async function hints(page: Page) {
  return page.evaluate(
    (prefix) =>
      Object.entries(localStorage).filter(([key]) => key.startsWith(prefix)),
    prefix,
  );
}
async function start(page: Page, mode = "discovery-ok") {
  await page.goto("/tests/frontend/provider-auto-connect.html?mode=" + mode);
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  const button = page.getByRole("button", {
    name: "找回后台凭据记录",
    exact: true,
  });
  await expect(button).toBeEnabled();
  await button.click();
  return page.getByRole("dialog", { name: "找回后台凭据记录", exact: true });
}
async function select(page: Page) {
  await page
    .getByRole("dialog", { name: "找回后台凭据记录", exact: true })
    .getByRole("button", { name: "添加恢复提示", exact: true })
    .first()
    .click();
  return page.getByRole("dialog", {
    name: "添加这条本机恢复提示",
    exact: true,
  });
}
async function consent(page: Page) {
  await page
    .getByRole("checkbox", {
      name: "我已核对这条记录及当前后台，同意仅添加本机恢复提示。",
      exact: true,
    })
    .check();
}
async function confirm(page: Page) {
  await consent(page);
  await page.getByRole("button", { name: "确认添加提示", exact: true }).click();
}
async function assertNoReplay(page: Page) {
  const fixture = await page.evaluate(() => ({
    credentials: window.autoConnectFixture.credentials,
    connections: window.autoConnectFixture.connections,
    credentialCancels: window.autoConnectFixture.credentialCancels,
  }));
  expect(fixture.credentials).toHaveLength(0);
  expect(fixture.connections).toHaveLength(0);
  expect(fixture.credentialCancels).toHaveLength(0);
}
test("all local hints lost: explicit selection reconstructs only the original scoped reminder", async ({
  page,
}) => {
  await start(page);
  expect(await hints(page)).toHaveLength(0);
  const dialog = await select(page);
  await expect(
    dialog.getByRole("button", { name: "确认添加提示", exact: true }),
  ).toBeDisabled();
  expect(await hints(page)).toHaveLength(0);
  await confirm(page);
  await expect(
    page.getByText(
      "已添加本机恢复提示。请核查保存结果或旧记录来源，再明确采用到连接。",
      { exact: true },
    ),
  ).toBeVisible();
  const saved = await hints(page);
  expect(saved).toHaveLength(1);
  expect(saved[0][0]).toBe(key);
  expect(JSON.parse(saved[0][1])).toEqual({
    version: 3,
    storeId: A,
    namespaceId: N,
    operationId: ID,
    label: "Current fixture",
    cancelledPending: false,
  });
  expect(saved[0][1]).not.toContain("credentialRef");
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentialChecks),
  ).toHaveLength(0);
  await assertNoReplay(page);
  await expect(
    page.getByRole("button", { name: "核查保存结果", exact: true }),
  ).toBeVisible();
});
for (const mode of [
  "discovery-v2",
  "discovery-prepared",
  "discovery-committed",
])
  test(`${mode}: restored hints retain protocol and migration boundaries without a key query`, async ({
    page,
  }) => {
    await start(page, mode);
    await select(page);
    await confirm(page);
    await expect.poll(() => hints(page).then((v) => v.length)).toBe(1);
    const saved = JSON.parse((await hints(page))[0][1]);
    expect(saved.version).toBe(mode === "discovery-v2" ? 2 : 3);
    expect(saved.storeId).toBe(A);
    expect(saved.operationId).toBe(ID);
    if (mode === "discovery-prepared") {
      expect(saved.legacySourceDigest).toBe("a".repeat(64));
      await expect(
        page.getByRole("button", { name: "继续旧凭据恢复", exact: true }),
      ).toBeVisible();
      await expect(
        page.getByRole("button", { name: "核查保存结果", exact: true }),
      ).toHaveCount(0);
    } else {
      expect(saved).not.toHaveProperty("legacySourceDigest");
      await expect(
        page.getByRole("button", { name: "核查保存结果", exact: true }),
      ).toBeVisible();
    }
    expect(
      await page.evaluate(() => window.autoConnectFixture.credentialChecks),
    ).toHaveLength(0);
    await assertNoReplay(page);
  });
test("legacy discovery cannot migrate or adopt before the existing separate source consent", async ({
  page,
}) => {
  await start(page, "discovery-legacy");
  const dialog = await select(page);
  await expect(dialog.getByText(/旧版记录没有历史后台身份/)).toBeVisible();
  await confirm(page);
  await expect(
    page.getByRole("button", { name: "继续旧凭据恢复", exact: true }),
  ).toBeVisible();
  expect(JSON.parse((await hints(page))[0][1]).legacySourceDigest).toBe(
    "a".repeat(64),
  );
  expect(
    (await page.evaluate(() => window.providerOperationsFixture.commands)).some(
      (row) => row.name === "confirm_legacy_provider_credential",
    ),
  ).toBe(false);
  await page
    .getByRole("button", { name: "继续旧凭据恢复", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "确认来源并恢复", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("checkbox", {
      name: "我确认这份旧记录属于当前连接的后台，同意保留原材料并按原引用恢复。",
      exact: true,
    })
    .check();
  await page
    .getByRole("button", { name: "确认来源并恢复", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "采用到当前连接", exact: true }),
  ).toBeVisible();
  await assertNoReplay(page);
});
for (const mode of ["discovery-changed", "discovery-label-changed"])
  test(`${mode}: material recheck rejects old selection before any local write`, async ({
    page,
  }) => {
    await start(page, mode);
    const dialog = await select(page);
    await confirm(page);
    await expect(
      dialog.getByText(/后台记录已变化|列表的来源或格式无法核查/),
    ).toBeVisible();
    expect(await hints(page)).toHaveLength(0);
    await expect(
      dialog.getByRole("button", { name: "确认添加提示", exact: true }),
    ).toBeDisabled();
    await assertNoReplay(page);
  });
test("same-store reconnection invalidates already selected consent until a fresh lookup", async ({
  page,
}) => {
  await start(page);
  const dialog = await select(page);
  await consent(page);
  await page.evaluate(
    (a) => window.providerOperationsFixture.switchStore(a),
    A,
  );
  await expect(
    dialog.getByRole("button", { name: "确认添加提示", exact: true }),
  ).toBeDisabled();
  expect(await hints(page)).toHaveLength(0);
  await assertNoReplay(page);
});
for (const mode of ["discovery-hold", "discovery-restore-hold"])
  test(`${mode}: late original-source replies cannot populate another editor or local hints`, async ({
    page,
  }) => {
    await start(page, mode);
    if (mode === "discovery-restore-hold") {
      await select(page);
      await confirm(page);
    }
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.providerOperationsFixture.commands.filter(
              (row) => row.name === "discover_provider_credentials",
            ).length,
        ),
      )
      .toBe(mode === "discovery-hold" ? 1 : 2);
    await page.evaluate((b) => {
      window.providerOperationsFixture.switchStore(b);
      window.autoConnectFixture.sourceChange?.(b);
      window.providerOperationsFixture.release();
    }, B);
    await expect(
      page.getByText("配置来源尚未核对", { exact: true }),
    ).toBeVisible();
    expect(await hints(page)).toHaveLength(0);
    await assertNoReplay(page);
  });
test("full quota preserves all original hints and sends no credential operation", async ({
  page,
}) => {
  await page.addInitScript((b) => {
    for (let index = 0; index < 16; index++) {
      const id = crypto.randomUUID();
      localStorage.setItem(
        `local-agent.credential-operation.v2.${b}.${id}`,
        JSON.stringify({
          version: 2,
          storeId: b,
          operationId: id,
          label: "Other fixture",
          cancelledPending: false,
        }),
      );
    }
  }, B);
  await start(page);
  const before = await hints(page);
  const dialog = await select(page);
  await confirm(page);
  await expect(dialog.getByText(/本机提示已满/)).toBeVisible();
  expect(await hints(page)).toEqual(before);
  await assertNoReplay(page);
});
test("local storage failure leaves backend discovery intact and no hint is fabricated", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const set = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (key.startsWith("local-agent.credential-operation.v3."))
        throw Error("fixture discovery quota");
      return set.call(this, key, value);
    };
  });
  await start(page);
  const dialog = await select(page);
  await confirm(page);
  await expect(
    dialog.getByText("fixture discovery quota", { exact: true }),
  ).toBeVisible();
  expect(await hints(page)).toHaveLength(0);
  await assertNoReplay(page);
});
test("same-id local material conflicts are retained without overwrite", async ({
  page,
}) => {
  const raw = JSON.stringify({
    version: 3,
    storeId: A,
    namespaceId: N,
    operationId: ID,
    label: "Different existing label",
    cancelledPending: false,
  });
  await page.addInitScript(({ key, raw }) => localStorage.setItem(key, raw), {
    key,
    raw,
  });
  await start(page);
  const dialog = await select(page);
  await confirm(page);
  await expect(
    dialog.getByText(/本机已有内容或阶段不同的同编号提示/),
  ).toBeVisible();
  expect(await page.evaluate((key) => localStorage.getItem(key), key)).toBe(
    raw,
  );
  await assertNoReplay(page);
});
test("cursor pages retain all 70 records with real previous/next boundaries", async ({
  page,
}) => {
  const dialog = await start(page, "discovery-many");
  await expect(
    dialog.getByText(/第 1 页；本页检查 32 项，显示 32 条记录/),
  ).toBeVisible();
  await expect(
    dialog.getByRole("button", { name: "添加恢复提示", exact: true }),
  ).toHaveCount(32);
  await expect(
    dialog.getByRole("button", { name: "上一页", exact: true }),
  ).toBeDisabled();
  await dialog.getByRole("button", { name: "下一页", exact: true }).click();
  await expect(dialog.getByText(/第 2 页/)).toBeVisible();
  await dialog.getByRole("button", { name: "下一页", exact: true }).click();
  await expect(
    dialog.getByText(/第 3 页；本页检查 6 项，显示 6 条记录/),
  ).toBeVisible();
  await expect(
    dialog.getByRole("button", { name: "下一页", exact: true }),
  ).toBeDisabled();
  await dialog.getByRole("button", { name: "上一页", exact: true }).click();
  await expect(dialog.getByText(/第 2 页/)).toBeVisible();
  expect(await hints(page)).toHaveLength(0);
  await assertNoReplay(page);
});
for (const mode of [
  "discovery-empty",
  "discovery-unrecognized",
  "discovery-fail",
  "discovery-bad-row",
])
  test(`${mode}: empty, incomplete and failed discovery have distinct states`, async ({
    page,
  }) => {
    const dialog = await start(page, mode);
    if (mode === "discovery-empty")
      await expect(
        dialog.getByText("当前后台未发现凭据保存记录。", { exact: true }),
      ).toBeVisible();
    else {
      await expect(
        dialog.getByText("当前后台未发现凭据保存记录。", { exact: true }),
      ).toHaveCount(0);
      if (mode === "discovery-unrecognized")
        await expect(dialog.getByText(/材料名称无法识别/)).toBeVisible();
      if (mode === "discovery-fail")
        await expect(
          dialog.getByText(/后台记录暂时无法完整读取/),
        ).toBeVisible();
      if (mode === "discovery-bad-row") {
        await expect(
          dialog
            .getByRole("button", { name: "添加恢复提示", exact: true })
            .nth(0),
        ).toBeDisabled();
        await expect(
          dialog
            .getByRole("button", { name: "添加恢复提示", exact: true })
            .nth(1),
        ).toBeEnabled();
      }
    }
    expect(await hints(page)).toHaveLength(0);
  });
for (const mode of [
  "discovery-api-foreign",
  "discovery-api-future",
  "discovery-api-extra",
  "discovery-api-duplicates",
  "discovery-api-counts",
  "discovery-api-phase",
  "discovery-api-digest",
  "discovery-api-cursor",
])
  test(`${mode}: strict list decoder rejects mismatched or future replies`, async ({
    page,
  }) => {
    await page.goto("/tests/frontend/credential-api.html?mode=" + mode);
    const result = await page.evaluate(
      async (input) => {
        try {
          await window.credentialClient.discoverCredentials(input);
          return "accepted";
        } catch (reason) {
          return (reason as { code: string }).code;
        }
      },
      { storeId: A, namespaceId: N, cursor: null },
    );
    expect(result).toBe("transport_invalid_response");
    expect(
      await page.evaluate(() =>
        window.credentialCommands.map((row) => row.name),
      ),
    ).toEqual(["discover_provider_credentials"]);
  });
test("discovery IPC projects only request identity and cursor, even for structural extra data", async ({
  page,
}) => {
  await page.goto("/tests/frontend/credential-api.html?mode=discovery-ok");
  await page.evaluate(
    async (input) => {
      const request = {
        ...input,
        secret: "never-transmit-discovery-extra",
      };
      await window.credentialClient.discoverCredentials(request);
    },
    { storeId: A, namespaceId: N, cursor: null },
  );
  const commands = await page.evaluate(() => window.credentialCommands);
  expect(JSON.stringify(commands)).not.toContain(
    "never-transmit-discovery-extra",
  );
  expect(
    Object.keys((commands[0].args as { request: object }).request).sort(),
  ).toEqual(["cursor", "namespaceId", "storeId"]);
});
test("discovery supports narrow dark layout and keyboard cancellation without adding a hint", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const dialog = await start(page);
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  await expect(
    dialog.getByRole("button", { name: "添加恢复提示", exact: true }),
  ).toBeVisible();
  await expect
    .poll(() => dialog.evaluate((element) => getComputedStyle(element).opacity))
    .toBe("1");
  expect(
    await dialog.evaluate(
      (element) => element.scrollWidth <= element.clientWidth + 1,
    ),
  ).toBe(true);
  const close = dialog.getByRole("button", { name: "关闭查找", exact: true });
  await close.focus();
  await page.keyboard.press("Enter");
  await expect(dialog).toHaveCount(0);
  expect(await hints(page)).toHaveLength(0);
});
