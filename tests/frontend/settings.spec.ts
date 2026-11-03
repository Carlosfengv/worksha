import { expect, test, type Page } from "@playwright/test";
import type { SettingsUpdate } from "../../src/features/settings/types";
import type {} from "./settings-harness";
import {
  settingsReceipt,
  scopedSettingsReceipt,
  freezeSettingsRequest,
  settingsProtocol,
} from "../../src/features/settings/save-result";

const toggle = (page: Page) =>
  page.getByRole("switch", { name: "额外保存消息与工具内容" });
async function finish(
  page: Page,
  index = 0,
  kind: "writes" | "checks" = "writes",
) {
  await page.evaluate(
    ({ index, kind }) => {
      const target =
        kind === "writes" ? window.settingsWrites : window.settingsChecks;
      const request = target[index].request;
      target[index].resolve({
        storeId: request.storeId,
        value: request.value,
        revision: request.expectedRevision + 1,
        operationId: request.operationId,
      });
    },
    { index, kind },
  );
}

test("pristine preferences follow snapshots; edited drafts keep their base and cannot overwrite newer settings", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings.html");
  await page.evaluate(() =>
    window.publishSettings({ captureContent: true, retentionDays: 7 }, 1),
  );
  await expect(toggle(page)).toBeChecked();
  await expect(
    page.getByRole("combobox", { name: "记录保留期" }),
  ).toContainText("7 天");
  await expect(page.getByRole("button", { name: "保存偏好" })).toBeDisabled();
  await toggle(page).click();
  await page.evaluate(() =>
    window.publishSettings({ captureContent: true, retentionDays: 90 }, 2),
  );
  await expect(toggle(page)).not.toBeChecked();
  await expect(page.getByText("设置已有新版本", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "保存偏好" })).toBeDisabled();
  await page.getByRole("button", { name: "撤销修改" }).click();
  await expect(toggle(page)).toBeChecked();
  await expect(
    page.getByRole("combobox", { name: "记录保留期" }),
  ).toContainText("90 天");
});

test("a durable save updates its own baseline even when snapshot refresh fails", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings.html");
  await toggle(page).click();
  await page.getByRole("button", { name: "保存偏好" }).click();
  await expect
    .poll(() => page.evaluate(() => window.settingsWrites.length))
    .toBe(1);
  await expect(toggle(page)).toBeDisabled();
  await finish(page);
  await expect(page.getByText("偏好已保存", { exact: true })).toBeVisible();
  await expect(toggle(page)).toBeChecked();
  await expect(page.getByRole("button", { name: "保存偏好" })).toBeDisabled();
  await expect(page.getByTestId("refreshes")).toHaveText("1");
  await expect(
    page.getByText("自动到期处理尚未启用。", { exact: true }),
  ).toBeVisible();
});

test("transport timeout checks the committed operation instead of claiming failure or resending", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings.html");
  await toggle(page).click();
  await page.getByRole("button", { name: "保存偏好" }).click();
  await page.evaluate(() =>
    window.settingsWrites[0].reject("transport_timeout", true),
  );
  await expect
    .poll(() => page.evaluate(() => window.settingsChecks.length))
    .toBe(1);
  await finish(page, 0, "checks");
  await expect(page.getByText("偏好已保存", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => window.settingsWrites.length)).toBe(1);
  await expect(page.getByText("未能保存", { exact: true })).toHaveCount(0);
});

test("unknown result preserves the draft, and an explicit retry reuses the same operation", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings.html");
  await toggle(page).click();
  await page.getByRole("button", { name: "保存偏好" }).click();
  await page.evaluate(() =>
    window.settingsWrites[0].reject("transport_connection", true),
  );
  await expect
    .poll(() => page.evaluate(() => window.settingsChecks.length))
    .toBe(1);
  await page.evaluate(() =>
    window.settingsChecks[0].reject("operation_not_found"),
  );
  await expect(page.getByText("保存结果待确认", { exact: true })).toBeVisible();
  await expect(toggle(page)).toBeChecked();
  await expect(toggle(page)).toBeDisabled();
  await page.getByRole("button", { name: "核查保存结果" }).click();
  await expect
    .poll(() => page.evaluate(() => window.settingsChecks.length))
    .toBe(2);
  await page.evaluate(() =>
    window.settingsChecks[1].reject("operation_not_found"),
  );
  await page.getByRole("button", { name: "重试同一保存" }).click();
  await expect
    .poll(() => page.evaluate(() => window.settingsWrites.length))
    .toBe(2);
  expect(await page.evaluate(() => window.settingsWrites[1].request)).toEqual(
    await page.evaluate(() => window.settingsWrites[0].request),
  );
  await finish(page, 1);
  await expect(page.getByText("偏好已保存", { exact: true })).toBeVisible();
});

test("a rejected stale write keeps input, while late unmounted replies do not refresh", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings.html");
  await toggle(page).click();
  await page.getByRole("button", { name: "保存偏好" }).click();
  await page.evaluate(() => window.settingsWrites[0].reject("conflict"));
  await expect(page.getByText("未能保存", { exact: true })).toBeVisible();
  await expect(toggle(page)).toBeChecked();
  expect(await page.evaluate(() => window.settingsChecks.length)).toBe(0);
  await page.getByRole("button", { name: "保存偏好" }).click();
  await page.getByRole("button", { name: "切换挂载" }).click();
  await finish(page, 1);
  await expect(page.getByTestId("refreshes")).toHaveText("1");
});

test("a legacy service cannot receive an unversioned settings write", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings.html?legacy");
  await expect(toggle(page)).toBeDisabled();
  await expect(page.getByRole("button", { name: "保存偏好" })).toBeDisabled();
  await expect(
    page.getByText("当前后台暂不支持安全保存设置", { exact: true }),
  ).toBeVisible();
  expect(await page.evaluate(() => window.settingsWrites.length)).toBe(0);
});

test("a backend with revisions but no database-scoped protocol cannot receive a settings write", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings.html?legacy-scope");
  await expect(toggle(page)).toBeDisabled();
  await expect(page.getByRole("button", { name: "保存偏好" })).toBeDisabled();
  expect(await page.evaluate(() => window.settingsWrites.length)).toBe(0);
});

test("a failed preflight is an editable failure without claiming an uncertain write", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings.html");
  await toggle(page).click();
  await page.getByRole("button", { name: "保存偏好" }).click();
  await page.evaluate(() =>
    window.settingsWrites[0].reject("transport_timeout", false, true),
  );
  await expect(page.getByText("未能保存", { exact: true })).toBeVisible();
  await expect(page.getByText("保存结果待确认", { exact: true })).toHaveCount(
    0,
  );
  await expect(toggle(page)).toBeEnabled();
  expect(await page.evaluate(() => window.settingsChecks.length)).toBe(0);
});

test("settings receipts validate operation identity, nullability, revision and values", () => {
  const value = {
    value: { captureContent: false, retentionDays: 7 },
    revision: 1,
    operationId: "op",
  };
  expect(settingsReceipt(value, "op")).toEqual(value);
  for (const bad of [
    null,
    {},
    { ...value, operationId: "other" },
    { ...value, revision: 0 },
    { ...value, revision: Number.MAX_SAFE_INTEGER + 1 },
    { ...value, value: { captureContent: "false", retentionDays: 7 } },
    { ...value, value: { captureContent: false, retentionDays: 1 } },
  ]) {
    expect(() => settingsReceipt(bad, "op")).toThrow(/回执/);
  }
});

const databaseB = "00000000-0000-4000-8000-000000000002";
test("scoped settings receipts validate the source, original revision and frozen values", () => {
  const request: SettingsUpdate = {
    storeId: "00000000-0000-4000-8000-000000000001",
    operationId: "1cece095-c5a6-4dfc-8cf7-f14099531504",
    expectedRevision: 0,
    value: { captureContent: false, retentionDays: 7 },
  };
  const receipt = {
    storeId: request.storeId,
    operationId: request.operationId,
    revision: 1,
    value: request.value,
  };
  expect(scopedSettingsReceipt(receipt, request)).toEqual(receipt);
  for (const bad of [
    { ...receipt, storeId: databaseB },
    { ...receipt, storeId: undefined },
    { ...receipt, revision: 2 },
    { ...receipt, value: { captureContent: true, retentionDays: 7 } },
  ])
    expect(() => scopedSettingsReceipt(bad, request)).toThrow(/回执/);
  const frozen = freezeSettingsRequest(request);
  request.value.retentionDays = 90;
  expect(frozen.value.retentionDays).toBe(7);
  expect(
    settingsProtocol({
      version: 1,
      storeId: frozen.storeId,
      receiptLimit: 256,
    }),
  ).toBe(true);
  for (const protocol of [
    null,
    {},
    { version: 1, storeId: databaseB, receiptLimit: 0 },
    { version: 2, storeId: databaseB, receiptLimit: 256 },
  ])
    expect(settingsProtocol(protocol)).toBe(false);
});
test("an edited draft keeps its original database even when a replacement has the same revision", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings.html");
  await toggle(page).click();
  await page.evaluate(
    (id) =>
      window.publishSettings(
        { captureContent: false, retentionDays: 90 },
        0,
        id,
      ),
    databaseB,
  );
  await expect(
    page.getByText("设置所属后台已变化", { exact: true }),
  ).toBeVisible();
  await expect(toggle(page)).toBeChecked();
  await expect(toggle(page)).toBeDisabled();
  await expect(
    page.getByRole("combobox", { name: "记录保留期" }),
  ).toContainText("30 天");
  await expect(page.getByRole("button", { name: "保存偏好" })).toBeDisabled();
  expect(await page.evaluate(() => window.settingsWrites.length)).toBe(0);
  await page.getByRole("button", { name: "撤销修改" }).click();
  await expect(toggle(page)).not.toBeChecked();
  await expect(
    page.getByRole("combobox", { name: "记录保留期" }),
  ).toContainText("90 天");
  await toggle(page).click();
  await page.getByRole("button", { name: "保存偏好" }).click();
  expect(
    await page.evaluate(() => window.settingsWrites[0].request),
  ).toMatchObject({
    storeId: databaseB,
    expectedRevision: 0,
    value: { captureContent: true, retentionDays: 90 },
  });
  await finish(page);
});

test("a confirmed higher revision from a previous database never masks a new database", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings.html");
  await toggle(page).click();
  await page.getByRole("button", { name: "保存偏好" }).click();
  await finish(page);
  await expect(page.getByText("偏好已保存", { exact: true })).toBeVisible();
  await page.evaluate(
    (id) =>
      window.publishSettings(
        { captureContent: false, retentionDays: 90 },
        0,
        id,
      ),
    databaseB,
  );
  await expect(toggle(page)).not.toBeChecked();
  await expect(
    page.getByRole("combobox", { name: "记录保留期" }),
  ).toContainText("90 天");
  await expect(
    page.getByText("原后台的设置已确认保存；此结果不替换当前后台设置。", {
      exact: true,
    }),
  ).toBeVisible();
  await toggle(page).click();
  await page.getByRole("button", { name: "保存偏好" }).click();
  expect(
    await page.evaluate(() => window.settingsWrites[1].request),
  ).toMatchObject({ storeId: databaseB, expectedRevision: 0 });
  await finish(page, 1);
  await expect(toggle(page)).toBeChecked();
});

for (const kind of ["writes", "checks"] as const) {
  test(`a late ${kind} receipt remains attributed to the original database`, async ({
    page,
  }) => {
    await page.goto("/tests/frontend/settings.html");
    await toggle(page).click();
    await page.getByRole("button", { name: "保存偏好" }).click();
    if (kind === "checks") {
      await page.evaluate(() =>
        window.settingsWrites[0].reject("transport_timeout", true),
      );
      await expect
        .poll(() => page.evaluate(() => window.settingsChecks.length))
        .toBe(1);
    }
    await page.evaluate(
      (id) =>
        window.publishSettings(
          { captureContent: false, retentionDays: 90 },
          0,
          id,
        ),
      databaseB,
    );
    await finish(page, 0, kind);
    await expect(
      page.getByText("原后台的设置已确认保存；此结果不替换当前后台设置。", {
        exact: true,
      }),
    ).toBeVisible();
    await expect(toggle(page)).not.toBeChecked();
    await expect(
      page.getByRole("combobox", { name: "记录保留期" }),
    ).toContainText("90 天");
    expect(await page.evaluate(() => window.settingsWrites.length)).toBe(1);
  });
}

test("an unknown save stays bound to its original database and cannot be retried into a replacement", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings.html");
  await toggle(page).click();
  await page.getByRole("button", { name: "保存偏好" }).click();
  await page.evaluate(() =>
    window.settingsWrites[0].reject("transport_connection", true),
  );
  await expect
    .poll(() => page.evaluate(() => window.settingsChecks.length))
    .toBe(1);
  await page.evaluate(() =>
    window.settingsChecks[0].reject("operation_not_found"),
  );
  await expect(page.getByText("保存结果待确认", { exact: true })).toBeVisible();
  await page.evaluate(
    (id) =>
      window.publishSettings(
        { captureContent: false, retentionDays: 90 },
        0,
        id,
      ),
    databaseB,
  );
  await page.getByRole("button", { name: "重试同一保存" }).click();
  expect(await page.evaluate(() => window.settingsWrites.length)).toBe(1);
  await page.getByRole("button", { name: "核查保存结果" }).click();
  await expect
    .poll(() => page.evaluate(() => window.settingsChecks.length))
    .toBe(2);
  expect(await page.evaluate(() => window.settingsChecks[1].request)).toEqual(
    await page.evaluate(() => window.settingsWrites[0].request),
  );
  await page.evaluate(() =>
    window.settingsChecks[1].reject("operation_store_changed"),
  );
  await expect(page.getByText("保存结果待确认", { exact: true })).toBeVisible();
  await page.evaluate(() =>
    window.publishSettings({ captureContent: false, retentionDays: 30 }, 0),
  );
  await page.getByRole("button", { name: "重试同一保存" }).click();
  await expect
    .poll(() => page.evaluate(() => window.settingsWrites.length))
    .toBe(2);
  expect(await page.evaluate(() => window.settingsWrites[1].request)).toEqual(
    await page.evaluate(() => window.settingsWrites[0].request),
  );
  await page.evaluate(() =>
    window.settingsWrites[1].reject("transport_timeout", false, true),
  );
  await expect
    .poll(() => page.evaluate(() => window.settingsChecks.length))
    .toBe(3);
  expect(await page.evaluate(() => window.settingsChecks[2].request)).toEqual(
    await page.evaluate(() => window.settingsWrites[0].request),
  );
  await page.evaluate(() =>
    window.settingsChecks[2].reject("operation_not_found"),
  );
  await expect(page.getByText("保存结果待确认", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "重试同一保存" }).click();
  await expect
    .poll(() => page.evaluate(() => window.settingsWrites.length))
    .toBe(3);
  expect(await page.evaluate(() => window.settingsWrites[2].request)).toEqual(
    await page.evaluate(() => window.settingsWrites[0].request),
  );
  await finish(page, 2);
  await expect(page.getByText("偏好已保存", { exact: true })).toBeVisible();
});
