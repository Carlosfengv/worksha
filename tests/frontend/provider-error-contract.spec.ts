import { expect, test } from "@playwright/test";
import contract from "../../resources/provider-error-contract.json" with { type: "json" };
import {
  providerErrorPolicy,
  providerWriteRejected,
} from "../../src/features/providers/error-contract";
import { DesktopError } from "../../src/shared/lib/desktop-command";

test("shared error policies classify only proven first-submission rejections", () => {
  expect(contract.version).toBe(1);
  for (const [code, policy] of Object.entries(contract.errors)) {
    expect(providerErrorPolicy(code)).toEqual(policy);
    expect(policy.httpStatus).toBeGreaterThanOrEqual(400);
    expect(policy.httpStatus).toBeLessThan(600);
    expect(
      providerWriteRejected(new DesktopError({ code, outcomeUnknown: false })),
    ).toBe(policy.initialProviderWrite === "reject");
    expect(
      providerWriteRejected(new DesktopError({ code, outcomeUnknown: true })),
    ).toBe(false);
    if (policy.outcomeUnknown)
      expect(policy.initialProviderWrite).toBe("query");
  }
  for (const code of ["future_error", "constructor", "__proto__"])
    expect(providerWriteRejected(new DesktopError({ code }))).toBe(false);
  expect(providerWriteRejected(new Error("conflict"))).toBe(false);
  expect(providerErrorPolicy("future_error")).toEqual(contract.fallback);
});
