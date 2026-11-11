import {
  createReadResource,
  createReadResourceFamily,
} from "@/shared/lib/read-resource";
import { modelServicesApi, routingPolicyApi } from "./api";
import type { ServicePreviewRequest } from "./types";

export const modelServicesResource = createReadResource(
  () => modelServicesApi.catalog(),
  { events: ["model-services-changed", "provider-status-changed"] },
);

export const servicePreviewResource = createReadResourceFamily(
  (input: ServicePreviewRequest | null) => {
    if (!input) throw Error("尚未选择预览范围。");
    return modelServicesApi.preview(input);
  },
  {
    events: [
      "model-services-changed",
      "provider-status-changed",
      "upstream-accounts-changed",
      "account-quota-changed",
    ],
    interval: false,
  },
);

export const routingPoliciesResource = createReadResourceFamily(
  (storeId: string) => routingPolicyApi.inventory(storeId),
  {
    events: ["routing-policy-changed", "model-services-changed"],
    interval: false,
  },
);
