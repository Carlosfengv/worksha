import { createRoot } from "react-dom/client";
import { isValidElement } from "react";
import { builtinPlugins, pageRegistry } from "@/app/builtin-plugins";
import { createUiRegistry } from "@/app/plugin-registry";
import type { Page } from "@/app/types";

// Inspect the actual registered React elements; never install a browser desktop bridge.
const pages: Page[] = [
  "overview",
  "agents",
  "tasks",
  "projects",
  "providers",
  "model-services",
  "mcp",
  "sessions",
  "requests",
  "plugins",
  "settings",
];
const contexts = pages.map((page) => {
  const hash = `#/${page}/%E4%BD%A0%2Fid?agent=codex-desktop&anchor=tool-1&from=claude-code-cli&tab=memory&days=7&model=a%2Fb&project=p&returnTo=%2Fprojects%2Fp&scope=all&view=context`;
  const route = pageRegistry.readRoute(hash);
  const element = pageRegistry.resolve(page).render({
    route,
    routeSearch: hash.split("?")[1],
    data: null,
    refresh: async () => {},
    navigate: () => {},
    pageRefresh: 2,
  });
  if (!isValidElement(element)) throw new Error("Invalid registered page");
  return { page, route, props: element.props };
});
const rejects = (factory: () => unknown) => {
  try {
    factory();
    return false;
  } catch {
    return true;
  }
};
const checks = {
  duplicate: rejects(() =>
    createUiRegistry([...builtinPlugins, builtinPlugins[0]]),
  ),
  missing: rejects(() => createUiRegistry(builtinPlugins.slice(1))),
  conflictingPage: rejects(() =>
    createUiRegistry([
      ...builtinPlugins,
      { id: "collision", version: 1, pages: builtinPlugins[0].pages },
    ]),
  ),
  badVersion: rejects(() =>
    createUiRegistry([
      {
        ...builtinPlugins[0],
        version: 2,
      } as unknown as (typeof builtinPlugins)[number],
    ]),
  ),
  unknown: pageRegistry.readRoute("#/future/bad").page,
  malformedId: pageRegistry.readRoute("#/sessions/%E0%A4%A").objectId ?? null,
  extension: pageRegistry.readRoute("#/extensions/dev.aurora/history"),
};
createRoot(document.getElementById("root")!).render(
  <>
    <output data-testid="contexts">{JSON.stringify(contexts)}</output>
    <output data-testid="checks">{JSON.stringify(checks)}</output>
    <output data-testid="navigation">
      {JSON.stringify(pageRegistry.navigation)}
    </output>
  </>,
);
