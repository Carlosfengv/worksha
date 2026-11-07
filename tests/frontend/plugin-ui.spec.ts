import { test, expect } from "@playwright/test";

test("registered pages retain all detail and return parameters", async ({
  page,
}) => {
  await page.goto("/tests/frontend/plugin-ui.html");
  const contexts = JSON.parse(
    (await page.getByTestId("contexts").textContent()) ?? "[]",
  );
  expect(contexts.map((row: { page: string }) => row.page)).toEqual([
    "overview",
    "agents",
    "providers",
    "model-services",
    "mcp",
    "sessions",
    "requests",
    "plugins",
    "settings",
  ]);
  for (const { route } of contexts) {
    expect(route).toMatchObject({
      objectId: "你/id",
      agent: "codex-desktop",
      anchor: "tool-1",
      from: "claude-code-cli",
      tab: "memory",
      days: "7",
      model: "a/b",
      project: "p",
      returnTo: "/projects/p",
      scope: "all",
      view: "context",
    });
  }
  const props = Object.fromEntries(
    contexts.map((row: { page: string; props: unknown }) => [
      row.page,
      row.props,
    ]),
  );
  expect(props.agents).toMatchObject({
    selectedId: "你/id",
    selectedTab: "memory",
  });
  expect(props.providers).toMatchObject({ selectedId: "你/id" });
  expect(props.sessions).toMatchObject({
    selectedId: "你/id",
    initialAgent: "codex-desktop",
    anchor: "tool-1",
    returnAgent: "claude-code-cli",
    returnTab: "memory",
    returnTo: "/projects/p",
    initialView: "context",
  });
  expect(props.mcp).toMatchObject({
    selectedId: "你/id",
    initialAgent: "codex-desktop",
    initialDays: "7",
    initialModel: "a/b",
    initialProject: "p",
    refreshToken: "2:[]",
  });
  expect(props.requests.routeSearch).toContain("returnTo=%2Fprojects%2Fp");
});
test("registry rejects conflicts and keeps navigation and scroll ownership", async ({
  page,
}) => {
  await page.goto("/tests/frontend/plugin-ui.html");
  expect(
    JSON.parse((await page.getByTestId("checks").textContent()) ?? "{}"),
  ).toEqual({
    duplicate: true,
    missing: true,
    conflictingPage: true,
    badVersion: true,
    unknown: "overview",
    malformedId: null,
    extension: {
      page: "plugins",
      extensionPlugin: "dev.aurora",
      extensionView: "history",
    },
    deferredTask: { page: "plugins", deferredPlugin: "tasks" },
    deferredProject: { page: "plugins", deferredPlugin: "projects" },
  });
  const navigation = JSON.parse(
    (await page.getByTestId("navigation").textContent()) ?? "[]",
  );
  expect(
    navigation
      .filter((item: { footer: boolean }) => item.footer)
      .map((item: { id: string }) => item.id),
  ).toEqual(["settings"]);
  expect(
    navigation
      .filter((item: { ownsBody: boolean }) => item.ownsBody)
      .map((item: { id: string }) => item.id),
  ).toEqual(["sessions"]);
  expect(
    navigation
      .filter(
        (item: { resetScrollOnObject: boolean }) => item.resetScrollOnObject,
      )
      .map((item: { id: string }) => item.id),
  ).toEqual(["providers"]);
});
