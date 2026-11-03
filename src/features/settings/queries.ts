import { createReadResource } from "@/shared/lib/read-resource";
import { settingsApi } from "./api";

export const gatewayStatusResource = createReadResource(() =>
  settingsApi.gatewayStatus(),
);
