import { createRoot } from "react-dom/client";
import App from "@/app/App";
import { iamApi } from "@/features/iam/api";
import { IAM_PLUGIN_ID, type Account } from "@/features/iam/types";
import { builtinPluginsApi } from "@/features/plugins/api";
import {
  builtinBusinessPlugins,
  emptyBuiltinCatalog,
} from "./plugin-center-peers";
import { installTableFixtures } from "./fixtures/tables/install";
import "@/styles/globals.css";

installTableFixtures();
let enabled = false;
let account: Account | null = null;
const owner: Account = {
  id: "owner",
  username: "owner",
  displayName: "林青",
  role: "owner",
  enabled: true,
  revision: 1,
};
let member: Account | undefined;
const resource = { id: "document-helper", name: "文档助手" };
let assigned = false;
const available = () =>
  account?.role === "owner" || (account?.role === "member" && assigned);
const builtin = builtinPluginsApi.catalog;
builtinPluginsApi.catalog = async () => {
  const catalog = await builtin();
  const base = builtinBusinessPlugins()[0];
  return {
    ...catalog,
    composition: {
      ...catalog.composition,
      optedInPluginIds: enabled ? [IAM_PLUGIN_ID] : [],
    },
    host: {
      ...catalog.host,
      plugins: [
        ...catalog.host.plugins,
        {
          ...base,
          id: IAM_PLUGIN_ID,
          displayName: "账号管理",
          optIn: true,
          desiredEnabled: enabled,
          runtimeActive: enabled,
          capabilities: [
            {
              ...base.capabilities[0],
              address: {
                ...base.capabilities[0].address,
                instance: IAM_PLUGIN_ID,
              },
            },
          ],
        },
      ],
    },
  };
};
iamApi.status = async () => ({ enabled, available: true, account });
iamApi.prepare = async () => ({
  operationId: emptyBuiltinCatalog.storeId,
  recoveryCode: "fixture-recovery-code",
});
iamApi.initialize = async () => {
  enabled = true;
};
iamApi.login = async (username) => {
  account =
    username === owner.username
      ? owner
      : member?.username === username
        ? member
        : null;
  if (!account) throw new Error("登录信息不正确");
};
iamApi.logout = async () => {
  account = null;
};
iamApi.accounts = async () => (member ? [owner, member] : [owner]);
iamApi.create = async ({ username, displayName }, role) => {
  member = {
    id: "member",
    username,
    displayName,
    role,
    enabled: true,
    revision: 1,
  };
  return member;
};
iamApi.resources = async () => (available() ? [resource] : []);
iamApi.grants = async () => (assigned ? [resource.id] : []);
iamApi.grant = async (_, id, allow) => {
  if (id === resource.id) assigned = allow;
};
iamApi.resource = async (id) => {
  if (!available() || id !== resource.id) throw new Error("当前资源不可查看");
  return {
    ...resource,
    creator: owner.displayName,
    models: ["Fixture model"],
    tenantId: "local",
  };
};
createRoot(document.getElementById("root")!).render(<App />);
