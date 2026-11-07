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
  const context = createTableFixtureContext();
  installTableShell(context);
  if (context.mode === "agents" && context.params.has("uploadedAgent")) {
    const snapshot = appApi.snapshot;
    const plugin = context.params.has("genericAgent")
      ? { id: "org.example.agent", name: "Example Agent" }
      : { id: "net.worksha.dsh-desktop", name: "DeepSeek Harness 桌面端" };
    const id = `${plugin.id}:desktop`;
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
