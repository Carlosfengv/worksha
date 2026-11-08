import {
  createReadResource,
  createReadResourceFamily,
} from "@/shared/lib/read-resource";
import { iamApi } from "./api";

export const accountsResource = createReadResource(() => iamApi.accounts());
export const grantsResource = createReadResourceFamily((id: string) =>
  iamApi.grants(id),
);
export const iamResources = createReadResource(() => iamApi.resources(), {
  interval: 5000,
});
export const iamResource = createReadResourceFamily(
  (id: string) => iamApi.resource(id),
  { interval: 5000 },
);
