import { providersApi } from "@/features/providers/api";
import { installProviderOperationFixture } from "../../provider-operation-fixture";
import { installAssociatedCosts } from "../../associated-cost-fixture";
import { createTableFixtureContext } from "./context";
import { installTableShell } from "./shell";
import { installTableProviders } from "./providers";
import { installTableActivity } from "./activity";
import { installTableProjects } from "./projects";

export const applicationTableModes = new Set([
  "shell",
  "agents",
  "mcp-page",
  "projects-page",
  "provider-history",
  "activity",
]);
export function installTableFixtures() {
  const context = createTableFixtureContext();
  installTableShell(context);
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
