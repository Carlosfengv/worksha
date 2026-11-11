import { iamApi } from "@/features/iam/api";
import { providersApi } from "@/features/providers/api";
import {
  installProviderOperationFixture,
  PROVIDER_FIXTURE_STORE_ID,
} from "../../provider-operation-fixture";
import { installAssociatedCosts } from "../../associated-cost-fixture";
import { createTableFixtureContext } from "./context";
import { installTableShell } from "./shell";
import { installTableProviders } from "./providers";
import { installTableActivity } from "./activity";
import { installTableProjects } from "./projects";
import { installPluginNavigation } from "./plugin-navigation";
import { appApi } from "@/app/api";
import {
  agentConnectionApi,
  type AgentConnectionReview,
  type AgentConnectionStatus,
  type ConnectionTarget,
} from "@/features/agents/api";
import { modelServicesApi } from "@/features/model-services/api";
import { defaultCapabilities } from "@/features/model-services/types";
import { fullPluginsApi } from "@/features/plugins/api";
import { bundledDshManifest } from "@/features/plugins/bundled-dsh";
import type { Manifest } from "../../../../contracts/plugins/sources/types";
import { fixtureProvider } from "./data";
import type { UpstreamAccount } from "@/features/providers/types";

export const applicationTableModes = new Set([
  "shell",
  "agents",
  "mcp-page",
  "projects-page",
  "provider-history",
  "activity",
  "plugin-navigation",
]);
export function installTableFixtures() {
  iamApi.status = async () => ({
    enabled: false,
    available: true,
    account: null,
  });
  const context = createTableFixtureContext();
  installTableShell(context);
  if (context.mode === "agents")
    agentConnectionApi.status = async () => ({
      state: "authorization-required",
    });
  if (context.mode === "agents" && context.params.has("uploadedAgent")) {
    const snapshot = appApi.snapshot;
    const plugin = context.params.has("genericAgent")
      ? { id: "org.example.agent", name: "Example Agent" }
      : { id: "net.worksha.dsh-desktop", name: "DeepSeek Harness 桌面端" };
    const id = `${plugin.id}:desktop`;
    fullPluginsApi.list = async () => ({
      apiVersion: "1.0.0",
      items: [
        {
          manifest: { ...bundledDshManifest, id: plugin.id } as Manifest,
          digest: "c".repeat(64),
          signingKey: "b".repeat(64),
          enabled: true,
          runtimeActive: true,
          publisherTrusted: true,
          activationEpoch: "1",
          grantRevision: "0",
          sourceAuthorized: false,
          unifiedAuthorized: false,
          persistMetadata: false,
          freshness: "unread",
        },
      ],
    });
    const connection = context.params.has("connectionAdapter");
    const cleanDsh = context.params.has("cleanDsh");
    if (cleanDsh)
      fullPluginsApi.list = async () => ({ apiVersion: "1.0.0", items: [] });
    if (connection) {
      const original = {
        provider: "dsh",
        model: "native",
      };
      let selected = original;
      let authorized = false;
      let preview: AgentConnectionReview | null = null;
      let choice: ConnectionTarget | null = null;
      let accountModel: string | null = null;
      let accountReads = 0;
      const provider = fixtureProvider(0);
      let verified: string | null = null;
      providersApi.verifyProvider = async (id, revision, model) => {
        if (id !== provider.id || revision !== provider.revision || !model)
          throw Error("wrong verification target");
        verified = model;
        return {
          providerId: id,
          model,
          fingerprint: "fixture",
          state: "verified",
          checkedAt: new Date().toISOString(),
          message: "已验证",
        };
      };
      const service = {
        id: "fixture-service",
        versionId: "fixture-service-v1",
        revision: 1,
        name: "Fixture model service",
        defaultModelId: "fixture-default",
        models: [
          {
            id: "fixture-default",
            name: "Fixture default model",
            provider,
            capabilities: defaultCapabilities(),
          },
        ],
        publishedAt: new Date().toISOString(),
      };
      modelServicesApi.catalog = async () => ({
        services: [service],
        attachments: [],
      });
      agentConnectionApi.status = async (
        agentId,
        action = { kind: "status" },
      ): Promise<AgentConnectionStatus> => {
        if (agentId !== id) throw Error("wrong Agent");
        if (action.kind === "authorize") authorized = true;
        if (action.kind === "revoke") authorized = false;
        if (!authorized) return { state: "authorization-required" };
        if (action.kind === "apply") {
          if (!preview || preview.reviewId !== action.reviewId)
            throw Error("missing preview");
          selected = accountModel
            ? { provider: "deepseek-account", model: accountModel }
            : { provider: "local-agent-fixture", model: "local-agent-default" };
        }
        if (action.kind === "restore") selected = original;
        const applied = selected !== original;
        return {
          state: "authorized",
          selected,
          configured: applied,
          selectedTarget: applied,
          ...(applied && accountModel && preview?.before
            ? {
                currentTarget: {
                  kind: "account" as const,
                  name: "DeepSeek 官方账号",
                  model: accountModel,
                },
                recovery: {
                  kind: "account" as const,
                  reviewId: preview.reviewId,
                  before: preview.before,
                },
              }
            : {}),
          ...(applied && preview?.before && choice
            ? {
                currentTarget: {
                  kind: choice.kind,
                  name: preview.targetName,
                  model:
                    choice.kind === "provider"
                      ? choice.model
                      : service.models[0].name,
                },
                recovery: {
                  reviewId: preview.reviewId,
                  before: preview.before,
                },
              }
            : {}),
        };
      };
      agentConnectionApi.preview = async (agentId, target) => {
        if (!authorized || agentId !== id) throw Error("not authorized");
        if (
          verified !==
          (target.kind === "provider" ? target.model : provider.model)
        )
          throw Error("not verified");
        choice = target;
        accountModel = null;
        preview = {
          reviewId: "owned-review",
          before: selected,
          targetName: target.kind === "provider" ? provider.name : service.name,
          models: [
            target.kind === "provider" ? target.model : service.models[0].name,
          ],
          scope: "new-sessions",
        };
        return preview;
      };
      agentConnectionApi.accountModels = async (agentId) => {
        if (!authorized || agentId !== id) throw Error("not authorized");
        const signedIn = ++accountReads > 1;
        return {
          signedIn,
          models: signedIn
            ? [{ id: "deepseek-fixture", name: "DeepSeek Fixture" }]
            : [],
        };
      };
      agentConnectionApi.previewAccount = async (agentId, model) => {
        if (!authorized || agentId !== id || model !== "deepseek-fixture")
          throw Error("wrong account model");
        choice = null;
        accountModel = model;
        preview = {
          reviewId: "account-review",
          before: selected,
          targetName: "DeepSeek 官方账号",
          models: ["DeepSeek Fixture"],
          scope: "new-sessions",
        };
        return preview;
      };
    }
    appApi.snapshot = async () => {
      const data = await snapshot();
      if (cleanDsh) return data;
      return {
        ...data,
        agents: [
          ...data.agents,
          {
            id,
            configId: id,
            family: plugin.id,
            name: plugin.name,
            pluginId: plugin.id,
            readonly: true,
            ...(connection
              ? { connectionAdapter: "agent.connection@1" as const }
              : {}),
            sourceId: id,
            found: true,
            location: null,
            connected: false,
            capabilities: [
              "domain.agent-directory@1",
              "domain.session-source@1",
            ],
          },
        ],
      };
    };
  }
  if (context.mode === "plugin-navigation") installPluginNavigation();
  const provider = installTableProviders();
  if (context.mode === "upstream-account-verification") {
    const providerId = provider.getProvider().id;
    const account: UpstreamAccount = {
      id: "00000000-0000-4000-8000-000000000101",
      providerId,
      label: "验证账号",
      revision: 3,
      state: "enabled",
      verifications: [
        {
          model: "model-000",
          state: "expired",
          checkedAt: "2026-11-01T00:00:00Z",
        },
      ],
      cooldowns: [
        {
          model: null,
          reason: "upstream_rate_limit",
          untilMs: Date.now() + 60_000,
        },
      ],
    };
    const paused: UpstreamAccount = {
      ...account,
      id: "00000000-0000-4000-8000-000000000102",
      label: "暂停账号",
      state: "paused",
      cooldowns: null,
    };
    providersApi.upstreamAccount = async (id, input) => {
      if (id !== providerId) throw Error("Unexpected provider");
      if (input.kind === "verify") {
        window.scopeQueries.push({ feature: "upstream-account", query: input });
        account.verifications = [
          {
            model: input.model,
            state: "verified",
            checkedAt: new Date().toISOString(),
          },
          ...(account.verifications ?? []),
        ];
        window.dispatchEvent(new Event("upstream-accounts-changed"));
      }
      return structuredClone({
        storeId: PROVIDER_FIXTURE_STORE_ID,
        accounts: [account, paused],
      });
    };
  }
  if (context.mode === "activity") installTableActivity(context);
  if (["mcp-page", "projects-page"].includes(context.mode ?? ""))
    installTableProjects(context);
  if (context.params.has("fees")) installAssociatedCosts();
  if (!applicationTableModes.has(context.mode ?? ""))
    window.providerOperationsFixture = installProviderOperationFixture({
      save: (input) => providersApi.saveProvider(input),
      connect: (input) => providersApi.connectProvider(input),
    });
  return { ...context, provider };
}
