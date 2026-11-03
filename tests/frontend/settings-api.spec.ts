import { test, expect, type Page } from "@playwright/test";
import type {} from "./settings-api-harness";
const request = {
  storeId: "00000000-0000-4000-8000-000000000001",
  operationId: "1cece095-c5a6-4dfc-8cf7-f14099531504",
  expectedRevision: 0,
  value: { captureContent: false, retentionDays: 7 as const },
};
async function save(page: Page, value = request) {
  return page.evaluate(async (input) => {
    try {
      await window.settingsScopedClient.saveSettings(input);
      return "saved";
    } catch (e) {
      return e && typeof e === "object" && "code" in e
        ? String(e.code)
        : String(e);
    }
  }, value);
}
for (const [mode, code] of [
  ["unsupported", "settings_protocol_unavailable"],
  ["wrong-store", "operation_store_changed"],
]) {
  test(`${mode} preflight never transmits a settings mutation`, async ({
    page,
  }) => {
    await page.goto(`/tests/frontend/settings-api.html?mode=${mode}`);
    expect(await save(page)).toBe(code);
    expect(
      await page.evaluate(() =>
        window.settingsScopedCommands.map((x) => x.name),
      ),
    ).toEqual(["get_scoped_settings_operation"]);
  });
}
test("a supported missing operation sends the original scope and an existing receipt does not skip backend conflict checks", async ({
  page,
}) => {
  for (const mode of ["missing", "existing", "conflict"]) {
    await page.goto(`/tests/frontend/settings-api.html?mode=${mode}`);
    expect(await save(page)).toBe(
      mode === "conflict" ? "operation_conflict" : "saved",
    );
    const commands = await page.evaluate(() => window.settingsScopedCommands);
    expect(commands.map((x) => x.name)).toEqual([
      "get_scoped_settings_operation",
      "save_settings",
    ]);
    expect(commands[0].args).toEqual({
      storeId: request.storeId,
      operationId: request.operationId,
    });
    expect(commands[1].args).toEqual({ settings: request });
  }
});
test("request mutation during preflight cannot redirect the pending save", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings-api.html?mode=held");
  await page.evaluate((input) => {
    window.mutableSettingsRequest = input;
    window.pendingSettingsSave =
      window.settingsScopedClient.saveSettings(input);
  }, request);
  await expect
    .poll(() => page.evaluate(() => window.settingsScopedCommands.length))
    .toBe(1);
  await page.evaluate(() => {
    window.mutableSettingsRequest!.storeId =
      "00000000-0000-4000-8000-000000000002";
    window.mutableSettingsRequest!.value.retentionDays = 90;
    window.releaseSettingsProbe();
  });
  await page.evaluate(() => window.pendingSettingsSave);
  expect(
    (await page.evaluate(() => window.settingsScopedCommands))[1].args,
  ).toEqual({ settings: request });
});
test("invalid request identity invokes nothing and a wrong-scope receipt remains unknown", async ({
  page,
}) => {
  await page.goto("/tests/frontend/settings-api.html?mode=malformed");
  expect(await save(page, { ...request, storeId: "invalid" })).toBe(
    "invalid_input",
  );
  expect(await page.evaluate(() => window.settingsScopedCommands.length)).toBe(
    0,
  );
  expect(await save(page)).toBe("transport_invalid_response");
});

test("the API distinguishes a preflight failure from an uncertain mutation response", async ({
  page,
}) => {
  for (const mode of ["probe-timeout", "malformed"]) {
    await page.goto(`/tests/frontend/settings-api.html?mode=${mode}`);
    const result = await page.evaluate(async (input) => {
      try {
        await window.settingsScopedClient.saveSettings(input);
        return null;
      } catch (e) {
        const error = e as { phase?: string; outcomeUnknown: boolean };
        return {
          phase: error.phase ?? null,
          outcomeUnknown: error.outcomeUnknown,
        };
      }
    }, request);
    expect(result).toEqual(
      mode === "probe-timeout"
        ? { phase: "preflight", outcomeUnknown: false }
        : { phase: null, outcomeUnknown: true },
    );
    expect(
      await page.evaluate(() => window.settingsScopedCommands.length),
    ).toBe(mode === "probe-timeout" ? 1 : 2);
  }
});
