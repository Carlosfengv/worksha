import { createReadResource } from "@/shared/lib/read-resource";
import { accountsApi } from "./api";

export const accountsResource = createReadResource(() =>
  accountsApi.accounts(),
);
