import { test, expect, type Page } from "@playwright/test";
import type {} from "./credential-api-harness";
const id = "cf45b4a0-4dac-4bd5-b276-3181d3e1514e";
async function create(page: Page, operationId = id) {
  return page.evaluate(async (operationId) => {
    try {
      await window.credentialClient.createProviderCredential(
        "Fixture",
        "fixture-private-key",
        operationId,
      );
      return "saved";
    } catch (reason) {
      return reason && typeof reason === "object" && "code" in reason
        ? String(reason.code)
        : String(reason);
    }
  }, operationId);
}
test("an unsupported service is rejected before a secret is sent", async ({
  page,
}) => {
  await page.goto("/tests/frontend/credential-api.html?mode=unsupported");
  expect(await create(page)).toBe("credential_protocol_unavailable");
  const commands = await page.evaluate(() => window.credentialCommands);
  expect(commands.map((row) => row.name)).toEqual([
    "get_provider_credential_operation",
  ]);
  expect(JSON.stringify(commands)).not.toContain("fixture-private-key");
});
test("a typed not-found reply confirms support and preserves the operation ID on create", async ({
  page,
}) => {
  await page.goto("/tests/frontend/credential-api.html");
  expect(await create(page)).toBe("saved");
  const commands = await page.evaluate(() => window.credentialCommands);
  expect(commands.map((row) => row.name)).toEqual([
    "get_provider_credential_operation",
    "create_provider_credential",
  ]);
  expect(commands[1].args).toMatchObject({ operationId: id });
});
test("an existing receipt does not bypass payload conflict checks", async ({
  page,
}) => {
  await page.goto("/tests/frontend/credential-api.html?mode=conflict");
  expect(await create(page)).toBe("operation_conflict");
  expect(
    await page.evaluate(() => window.credentialCommands.map((row) => row.name)),
  ).toEqual([
    "get_provider_credential_operation",
    "create_provider_credential",
  ]);
});
test("invalid IDs never invoke creation and malformed receipts stay unknown", async ({
  page,
}) => {
  await page.goto("/tests/frontend/credential-api.html?mode=malformed");
  expect(await create(page, "invalid")).toContain("凭据操作编号无效");
  expect(await page.evaluate(() => window.credentialCommands)).toHaveLength(0);
  expect(await create(page)).toBe("transport_invalid_response");
});
