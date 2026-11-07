import { createReadResource } from "@/shared/lib/read-resource";
import {
  pluginsApi,
  fullPluginsApi,
  objectContributionsApi,
  builtinPluginsApi,
} from "./api";

export const builtinPluginResource = createReadResource(() =>
  builtinPluginsApi.catalog(),
);

export const objectContributionResource = createReadResource(async () => {
  const reply = await objectContributionsApi.perform({ kind: "catalog" });
  if (reply.kind !== "catalog") throw new Error("对象扩展目录响应无效。");
  return reply.catalog;
});

export const fullPluginDirectoryResource = createReadResource(() =>
  fullPluginsApi.list(),
);

export const pluginBackupResource = createReadResource(
  () => pluginsApi.extensionHistory(),
  { interval: false },
);

export const pluginDirectoryResource = createReadResource(() =>
  pluginsApi.list(),
);
