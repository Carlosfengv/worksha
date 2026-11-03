import {
  createReadResource,
  createReadResourceFamily,
} from "@/shared/lib/read-resource";
import { modelServicesApi, routingPolicyApi } from "./api";

export const modelServicesResource = createReadResource(
  () => modelServicesApi.catalog(),
  { events: ["model-services-changed", "provider-status-changed"] },
);

export const routingPoliciesResource = createReadResourceFamily(
  (storeId: string) => routingPolicyApi.inventory(storeId),
  {
    events: ["routing-policy-changed", "model-services-changed"],
    interval: false,
  },
);
