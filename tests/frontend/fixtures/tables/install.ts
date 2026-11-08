import { iamApi } from "@/features/iam/api";
import { providersApi } from "@/features/providers/api";
import { installProviderOperationFixture } from "../../provider-operation-fixture";
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
    if (connection) {
      const original = {
        provider: "dsh",
        model: "native",
      };
      let selected = original;
      let authorized = false;
      let preview: AgentConnectionReview | null = null;
      let choice: ConnectionTarget | null = null;
      const provider = fixtureProvider(0);
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
          selected = {
            provider: "local-agent-fixture",
            model: "local-agent-default",
          };
        }
        if (action.kind === "restore") selected = original;
        const applied = selected !== original;
        return {
          state: "authorized",
          selected,
          configured: applied,
          selectedTarget: applied,
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
        choice = target;
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
    }
    appApi.snapshot = async () => {
      const data = await snapshot();
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
