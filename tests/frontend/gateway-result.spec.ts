import { expect, test } from "@playwright/test";
import { gatewayFileResult } from "../../src/features/settings/gateway-result";
import { DesktopError } from "../../src/shared/lib/desktop-command";
const id = "11111111-1111-4111-8111-111111111111";
test("gateway file results preserve legacy metadata and explicit original source while projecting only public fields", () => {
  expect(gatewayFileResult({ status: "cancelled" })).toEqual({
    status: "cancelled",
  });
  const saved = { status: "saved", id, fileName: "client.json" };
  expect(gatewayFileResult(saved)).toEqual(saved);
  const scoped = { ...saved, sourceInstanceId: id, sourceCurrent: false };
  expect(gatewayFileResult({ ...scoped, extra: "not-retained" })).toEqual(
    scoped,
  );
});
test("malformed or contradictory creation results are unknown rather than successful cancellation", () => {
  const saved = { status: "saved", id, fileName: "client.json" };
  for (const value of [
    null,
    [],
    {},
    { status: "pending" },
    { status: "cancelled", id },
    { status: "cancelled", key: "fixture" },
    { ...saved, id: "other" },
    { ...saved, fileName: null },
    { ...saved, fileName: "" },
    { ...saved, apiKey: "fixture-must-not-cross-native-boundary" },
    { ...saved, sourceInstanceId: id },
    { ...saved, sourceInstanceId: "other", sourceCurrent: true },
  ]) {
    try {
      gatewayFileResult(value);
      throw new Error("invalid fixture accepted");
    } catch (error) {
      expect(error).toBeInstanceOf(DesktopError);
      expect((error as DesktopError).code).toBe("transport_invalid_response");
      expect((error as DesktopError).outcomeUnknown).toBe(true);
    }
  }
});
