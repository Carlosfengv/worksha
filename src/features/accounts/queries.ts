import { createReadResource } from "@/shared/lib/read-resource";
import { accountsApi } from "./api";

export const accountsResource = createReadResource(
  () => accountsApi.accounts(),
  { events: ["upstream-accounts-changed", "provider-status-changed"] },
);
