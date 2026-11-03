import * as shared from "@/shared/lib/recovery-storage";
import * as providers from "@/features/providers/pending-operations";
import { freezeProviderRequest } from "@/features/providers/operation-receipt";
import { responseProvider } from "./provider-response-fixture";

const request = freezeProviderRequest(
  {
    version: 1,
    storeId: "c9e027fe-4536-4d08-8a55-74e6cbaf6313",
    sequence: 0,
    receiptLimit: 256,
    receiptBytes: 2097152,
    pendingLimit: 4,
    pendingOperationIds: [],
  },
  "save",
  responseProvider,
);
const fixture = { shared, providers, request };
declare global {
  interface Window {
    recoveryStorageFixture: typeof fixture;
  }
}
window.recoveryStorageFixture = fixture;
