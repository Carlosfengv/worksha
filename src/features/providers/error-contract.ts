import contract from "../../../resources/provider-error-contract.json" with { type: "json" };
import { DesktopError } from "@/shared/lib/desktop-command";

interface ErrorPolicy {
  httpStatus: number;
  outcomeUnknown: boolean;
  initialProviderWrite: string;
}
const policies: Record<string, ErrorPolicy> = contract.errors;
export function providerErrorPolicy(code: string): Readonly<ErrorPolicy> {
  return Object.hasOwn(policies, code) ? policies[code] : contract.fallback;
}

// Admission applies only to the first submission. Explicit retries still query
// the original operation, even when a known rejection is returned.
export function providerWriteRejected(reason: unknown) {
  return (
    reason instanceof DesktopError &&
    !reason.outcomeUnknown &&
    providerErrorPolicy(reason.code).initialProviderWrite === "reject"
  );
}
