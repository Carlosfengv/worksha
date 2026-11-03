import { expect, test, type Page } from "@playwright/test";
import type {} from "./settings-harness";
import type { SettingsUpdate } from "../../src/features/settings/types";

const prefix = "local-agent.settings-operation.v1.";
const databaseA = "00000000-0000-4000-8000-000000000001";
const databaseB = "00000000-0000-4000-8000-000000000002";
const original: SettingsUpdate = {
  storeId: databaseA,
  operationId: "11111111-1111-4111-8111-111111111111",
  expectedRevision: 0,
  value: { captureContent: true, retentionDays: 7 },
};
const toggle = (page: Page) =>
  page.getByRole("switch", { name: "额外保存消息与工具内容" });
const records = (page: Page) =>
  page.evaluate(() => window.settingsRecovery.read());
async function seed(page: Page, request = original) {
  await page.addInitScript(
    ({ prefix, request }) => {
      localStorage.setItem(
        prefix + request.operationId,
        JSON.stringify({ version: 1, request }),
      );
    },
    { prefix, request },
  );
}
async function finish(
  page: Page,
  kind: "writes" | "checks" = "checks",
  index = 0,
) {
  await page.evaluate(
    ({ kind, index }) => {
      const pending = (
        kind === "writes" ? window.settingsWrites : window.settingsChecks
      )[index];
      pending.resolve({
        ...pending.request,
        revision: pending.request.expectedRevision + 1,
      });
    },
    { kind, index },
  );
}
async function startUnknown(page: Page) {
  await page.goto("/tests/frontend/settings.html");
  await toggle(page).click();
  await page.getByRole("button", { name: "保存偏好", exact: true }).click();
  const request = await page.evaluate(() => window.settingsWrites[0].request);
  await page.evaluate(() =>
    window.settingsWrites[0].reject("transport_timeout", true),
  );
  await expect
    .poll(() => page.evaluate(() => window.settingsChecks.length))
    .toBe(1);
  await page.evaluate(() =>
    window.settingsChecks[0].reject("operation_not_found"),
  );
  await expect(page.getByText("保存结果待确认", { exact: true })).toBeVisible();
  return request;
}

test("the exact whitelisted request survives navigation and reload without automatic calls", async ({
  page,
}) => {
  const request = await startUnknown(page);
  expect((await records(page)).requests).toEqual([request]);
  const raw = await page.evaluate(
    (prefix) =>
      localStorage.getItem(
        prefix + window.settingsWrites[0].request.operationId,
      ),
    prefix,
  );
  expect(JSON.parse(raw!)).toEqual({ version: 1, request });
  await page.getByRole("button", { name: "切换挂载" }).click();
  await page.getByRole("button", { name: "切换挂载" }).click();
  await expect(
    page.getByText("保留的设置保存待核查", { exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => [
      window.settingsWrites.length,
      window.settingsChecks.length,
    ]),
  ).toEqual([1, 1]);
  await page.reload();
  await expect(
    page.getByText("保留的设置保存待核查", { exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => [
      window.settingsWrites.length,
      window.settingsChecks.length,
    ]),
  ).toEqual([0, 0]);
  await expect(toggle(page)).toBeDisabled();
  await page.getByRole("button", { name: "核查保留的保存" }).click();
  expect(await page.evaluate(() => window.settingsChecks[0].request)).toEqual(
    request,
  );
  await finish(page);
  await expect(page.getByText("偏好已保存", { exact: true })).toBeVisible();
  expect((await records(page)).requests).toEqual([]);
  await expect(toggle(page)).toBeChecked();
  await expect(toggle(page)).toBeEnabled();
});

test("leaving during a save retains the request even if the old component later receives success", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings.html");
  await toggle(page).click();
  await page.getByRole("button", { name: "保存偏好", exact: true }).click();
  const request = await page.evaluate(() => window.settingsWrites[0].request);
  await page.getByRole("button", { name: "切换挂载" }).click();
  await finish(page, "writes");
  await expect(page.getByTestId("refreshes")).toHaveText("0");
  expect((await records(page)).requests).toEqual([request]);
  await page.getByRole("button", { name: "切换挂载" }).click();
  await expect(
    page.getByText("保留的设置保存待核查", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "核查保留的保存" }).click();
  await finish(page);
  await expect(page.getByText("偏好已保存", { exact: true })).toBeVisible();
});

test("a restored retry keeps the original identity and values; missing or failed preflight never clears uncertainty", async ({
  page,
}) => {
  const request = await startUnknown(page);
  await page.reload();
  await page.getByRole("button", { name: "核查保留的保存" }).click();
  await page.evaluate(() =>
    window.settingsChecks[0].reject("operation_not_found"),
  );
  await expect(page.getByText("保存结果待确认", { exact: true })).toBeVisible();
  expect((await records(page)).requests).toEqual([request]);
  await expect(
    page.getByText(`操作编号：${request.operationId}`, { exact: true }),
  ).toBeVisible();
  await expect(page.getByText(/原偏好：/)).toContainText("额外保存内容开启");
  await expect(page.getByText(/原偏好：/)).toContainText("30 天");
  await page.getByRole("button", { name: "重试同一保存" }).click();
  expect(await page.evaluate(() => window.settingsWrites[0].request)).toEqual(
    request,
  );
  await page.evaluate(() =>
    window.settingsWrites[0].reject("transport_timeout", false, true),
  );
  await expect
    .poll(() => page.evaluate(() => window.settingsChecks.length))
    .toBe(2);
  await page.evaluate(() =>
    window.settingsChecks[1].reject("operation_not_found"),
  );
  await expect(page.getByText("保存结果待确认", { exact: true })).toBeVisible();
  expect((await records(page)).requests).toEqual([request]);
  await page.getByRole("button", { name: "重试同一保存" }).click();
  expect(await page.evaluate(() => window.settingsWrites[1].request)).toEqual(
    request,
  );
  await finish(page, "writes", 1);
  expect((await records(page)).requests).toEqual([]);
});

test("an old recovered receipt never masks newer settings or another database", async ({
  page,
}) => {
  await seed(page);
  await page.goto("/tests/frontend/settings.html");
  await page.evaluate(() =>
    window.publishSettings({ captureContent: false, retentionDays: 90 }, 20),
  );
  await page.getByRole("button", { name: "核查保留的保存" }).click();
  await finish(page);
  await expect(page.getByText("偏好已保存", { exact: true })).toBeVisible();
  await expect(toggle(page)).not.toBeChecked();
  await expect(
    page.getByRole("combobox", { name: "记录保留期" }),
  ).toContainText("90 天");
  expect((await records(page)).requests).toEqual([]);
});

test("a replacement database cannot retry a restored foreign operation, including with equal revisions", async ({
  page,
}) => {
  await seed(page);
  await page.goto("/tests/frontend/settings.html");
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
    page.getByRole("button", { name: "重试保留的保存" }),
  ).toBeDisabled();
  await expect(toggle(page)).toBeEnabled();
  await page.getByRole("button", { name: "核查保留的保存" }).click();
  expect(await page.evaluate(() => window.settingsChecks[0].request)).toEqual(
    original,
  );
  await page.evaluate(() =>
    window.settingsChecks[0].reject("operation_store_changed"),
  );
  await expect(page.getByText("保存结果待确认", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "重试同一保存" }).click();
  expect(await page.evaluate(() => window.settingsWrites.length)).toBe(0);
  expect((await records(page)).requests).toEqual([original]);
});

test("recovery remains accessible when the snapshot disappears, and read failures preserve the stored request", async ({
  page,
}) => {
  await seed(page);
  await page.goto("/tests/frontend/settings.html");
  await page.evaluate(() => window.setSettingsAvailable(false));
  await expect(page.getByRole("switch")).toHaveCount(0);
  await expect(
    page.getByText("保留的设置保存待核查", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "重试保留的保存" }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "核查保留的保存" }).click();
  await page.evaluate(() =>
    window.settingsChecks[0].reject("transport_connection", true),
  );
  await expect(page.getByText(/^尚未确认保存结果/)).toBeVisible();
  expect((await records(page)).requests).toEqual([original]);
  await page.getByRole("button", { name: "核查保留的保存" }).click();
  await finish(page, "checks", 1);
  await expect(page.getByText(/^偏好已保存到本机/)).toBeVisible();
  expect((await records(page)).requests).toEqual([]);
});

test("quota failure before dispatch sends no read or write and keeps the editable draft", async ({
  page,
}) => {
  await page.addInitScript((prefix) => {
    const set = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (key.startsWith(prefix))
        throw new DOMException("fixture", "QuotaExceededError");
      return set.call(this, key, value);
    };
  }, prefix);
  await page.goto("/tests/frontend/settings.html");
  await toggle(page).click();
  await page.getByRole("button", { name: "保存偏好", exact: true }).click();
  await expect(page.getByText("未能保存", { exact: true })).toBeVisible();
  await expect(toggle(page)).toBeChecked();
  await expect(toggle(page)).toBeEnabled();
  expect(
    await page.evaluate(() => [
      window.settingsWrites.length,
      window.settingsChecks.length,
    ]),
  ).toEqual([0, 0]);
});

test("unavailable recovery storage disables new saves instead of treating it as an empty history", async ({
  page,
}) => {
  await page.addInitScript(() =>
    Object.defineProperty(window, "localStorage", {
      get() {
        throw new DOMException("fixture", "SecurityError");
      },
    }),
  );
  await page.goto("/tests/frontend/settings.html");
  await expect(
    page.getByText("设置恢复记录暂不可用", { exact: true }),
  ).toBeVisible();
  await toggle(page).click();
  await expect(
    page.getByRole("button", { name: "保存偏好", exact: true }),
  ).toBeDisabled();
  expect(
    await page.evaluate(() => [
      window.settingsWrites.length,
      window.settingsChecks.length,
    ]),
  ).toEqual([0, 0]);
});

test("corrupt and oversized records stay byte-for-byte intact and do not appear in interface text", async ({
  page,
}) => {
  await page.addInitScript(
    ({ prefix, request }) => {
      localStorage.setItem(
        prefix + request.operationId,
        "PRIVATE_FIXTURE".repeat(600),
      );
      localStorage.setItem(
        prefix + "bad",
        JSON.stringify({ version: 99, request }),
      );
    },
    { prefix, request: original },
  );
  await page.goto("/tests/frontend/settings.html");
  await expect(
    page.getByText("设置恢复记录暂不可用", { exact: true }),
  ).toBeVisible();
  await expect(page.locator("body")).not.toContainText("PRIVATE_FIXTURE");
  await toggle(page).click();
  await expect(
    page.getByRole("button", { name: "保存偏好", exact: true }),
  ).toBeDisabled();
  expect((await records(page)).invalid).toBe(2);
  expect(
    await page.evaluate(
      ({ prefix, request }) =>
        localStorage.getItem(prefix + request.operationId),
      { prefix, request: original },
    ),
  ).toBe("PRIVATE_FIXTURE".repeat(600));
  expect(
    await page.evaluate(() => [
      window.settingsWrites.length,
      window.settingsChecks.length,
    ]),
  ).toEqual([0, 0]);
});

test("a changed saved request cannot replace an in-memory unknown retry or be removed by its old receipt", async ({
  page,
}) => {
  const request = await startUnknown(page);
  await page.evaluate(
    ({ prefix, request }) => {
      localStorage.setItem(
        prefix + request.operationId,
        JSON.stringify({
          version: 1,
          request: {
            ...request,
            value: { ...request.value, retentionDays: 90 },
          },
        }),
      );
      window.dispatchEvent(new Event("storage"));
    },
    { prefix, request },
  );
  await page.getByRole("button", { name: "重试同一保存" }).click();
  await expect(page.getByText("保存结果待确认", { exact: true })).toBeVisible();
  expect(
    await page.evaluate(() => [
      window.settingsWrites.length,
      window.settingsChecks.length,
    ]),
  ).toEqual([1, 1]);
  await page.getByRole("button", { name: "核查保存结果" }).click();
  await finish(page, "checks", 1);
  await expect(page.getByText("偏好已保存", { exact: true })).toBeVisible();
  expect((await records(page)).requests[0].value.retentionDays).toBe(90);
  await expect(page.locator("body")).toContainText("恢复记录未能移除");
});

test("a confirmed receipt stays successful if recovery metadata removal fails", async ({
  page,
}) => {
  await page.addInitScript((prefix) => {
    const remove = Storage.prototype.removeItem;
    Storage.prototype.removeItem = function (key) {
      if (key.startsWith(prefix))
        throw new DOMException("fixture", "SecurityError");
      return remove.call(this, key);
    };
  }, prefix);
  await page.goto("/tests/frontend/settings.html");
  await toggle(page).click();
  await page.getByRole("button", { name: "保存偏好", exact: true }).click();
  await finish(page, "writes");
  await expect(page.getByText("偏好已保存", { exact: true })).toBeVisible();
  await expect(page.getByText("保存结果待确认", { exact: true })).toHaveCount(
    0,
  );
  expect((await records(page)).requests).toHaveLength(1);
  await expect(toggle(page)).toBeChecked();
});

test("storage events retain distinct operation keys and enforce the capacity without evicting old requests", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings.html");
  await page.evaluate(
    ({ prefix, request }) => {
      for (let n = 1; n <= 16; n++) {
        const id = `10000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
        const storeId = `20000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
        localStorage.setItem(
          prefix + id,
          JSON.stringify({
            version: 1,
            request: { ...request, operationId: id, storeId },
          }),
        );
      }
      window.dispatchEvent(new Event("storage"));
    },
    { prefix, request: original },
  );
  await expect(
    page.getByText("待核查设置操作已达上限", { exact: true }),
  ).toBeVisible();
  expect((await records(page)).requests).toHaveLength(16);
  await expect(page.getByRole("table").getByRole("row")).toHaveCount(17);
  await expect(
    page.getByRole("button", { name: "核查保留的保存" }),
  ).toHaveCount(16);
  await toggle(page).click();
  await expect(
    page.getByRole("button", { name: "保存偏好", exact: true }),
  ).toBeDisabled();
  expect(
    await page.evaluate(() => [
      window.settingsWrites.length,
      window.settingsChecks.length,
    ]),
  ).toEqual([0, 0]);
});

test("record parsing projects only allowed fields and rejects mismatched identities without overwriting existing records", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings.html");
  const result = await page.evaluate(
    ({ request, prefix }) => {
      window.settingsRecovery.remember({
        ...request,
        ignored: "PRIVATE_FIXTURE",
        value: { ...request.value, ignored: "PRIVATE_FIXTURE" },
      } as SettingsUpdate);
      const raw = localStorage.getItem(prefix + request.operationId);
      let rejected = false;
      try {
        window.settingsRecovery.remember({
          ...request,
          value: { captureContent: false, retentionDays: 90 },
        });
      } catch {
        rejected = true;
      }
      return {
        raw,
        rejected,
        count: window.settingsRecovery.read().requests.length,
      };
    },
    { request: original, prefix },
  );
  expect(JSON.parse(result.raw!)).toEqual({ version: 1, request: original });
  expect(result.rejected).toBe(true);
  expect(result.count).toBe(1);
});

test("a foreign recovered receipt preserves a current-store confirmation and exposes cleanup failure", async ({
  page,
}) => {
  await seed(page);
  await page.goto("/tests/frontend/settings.html");
  await page.evaluate(
    (id) =>
      window.publishSettings(
        { captureContent: false, retentionDays: 90 },
        0,
        id,
      ),
    databaseB,
  );
  await toggle(page).click();
  await page.getByRole("button", { name: "保存偏好", exact: true }).click();
  await finish(page, "writes");
  await expect(toggle(page)).toBeChecked();
  await page.evaluate((prefix) => {
    const remove = Storage.prototype.removeItem;
    Storage.prototype.removeItem = function (key) {
      if (key.startsWith(prefix))
        throw new DOMException("fixture", "SecurityError");
      return remove.call(this, key);
    };
  }, prefix);
  await page.getByRole("button", { name: "核查保留的保存" }).click();
  expect(await page.evaluate(() => window.settingsChecks[0].request)).toEqual(
    original,
  );
  await finish(page);
  await expect(page.getByText("偏好已保存", { exact: true })).toBeVisible();
  await expect(toggle(page)).toBeChecked();
  await expect(
    page.getByRole("combobox", { name: "记录保留期" }),
  ).toContainText("90 天");
  await expect(page.locator("body")).toContainText("恢复记录未能移除");
  expect((await records(page)).requests).toEqual([original]);
});

test("an older recovered receipt cannot replace a higher confirmed revision when refresh remains unavailable", async ({
  page,
}) => {
  const newer = { ...original, expectedRevision: 4 };
  await seed(page, newer);
  await page.goto("/tests/frontend/settings.html");
  await page.getByRole("button", { name: "核查保留的保存" }).click();
  await finish(page);
  await expect(toggle(page)).toBeChecked();
  await expect(
    page.getByRole("combobox", { name: "记录保留期" }),
  ).toContainText("7 天");
  const older: SettingsUpdate = {
    ...original,
    operationId: "22222222-2222-4222-8222-222222222222",
    value: { captureContent: false, retentionDays: 90 },
  };
  await page.evaluate(
    (request) => window.settingsRecovery.remember(request),
    older,
  );
  await page.getByRole("button", { name: "核查保留的保存" }).click();
  await finish(page, "checks", 1);
  await expect(page.getByText("偏好已保存", { exact: true })).toBeVisible();
  await expect(toggle(page)).toBeChecked();
  await expect(
    page.getByRole("combobox", { name: "记录保留期" }),
  ).toContainText("7 天");
  expect((await records(page)).requests).toEqual([]);
});

test("the recovery table contains narrow-screen overflow and preserves keyboard focus through storage refresh", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await seed(page);
  await page.goto("/tests/frontend/settings.html");
  const check = page.getByRole("button", { name: "核查保留的保存" });
  await expect(page.getByRole("table")).toBeVisible();
  await check.focus();
  await page.evaluate(() => window.dispatchEvent(new Event("storage")));
  await expect(check).toBeFocused();
  expect(
    await page.evaluate(
      () =>
        document.documentElement.scrollWidth <=
        document.documentElement.clientWidth,
    ),
  ).toBe(true);
  await page.keyboard.press("Enter");
  await expect
    .poll(() => page.evaluate(() => window.settingsChecks.length))
    .toBe(1);
  await finish(page);
  await expect(page.getByText("偏好已保存", { exact: true })).toBeVisible();
});
