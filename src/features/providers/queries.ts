import {
  createReadResource,
  createReadResourceFamily,
} from "@/shared/lib/read-resource";
import { providersApi } from "./api";
import type { ProviderUsageQuery } from "./types";

const events = ["provider-status-changed", "model-services-changed"];
export const providerStatusResource = createReadResource(
  () => providersApi.providerStatus(),
  { events },
);
export const providerUsageResource = createReadResourceFamily(
  (query: ProviderUsageQuery) => providersApi.providerUsage(query),
  { events },
);
