import { pluginsApi } from "@/features/plugins/api";
import { pluginDirectoryResource } from "@/features/plugins/queries";
import type { Manifest, PluginStatus } from "@/features/plugins/types";
import raw from "../../../../sdk/plugins/examples/source-report/package/manifest.json";
import contribution from "../../../../sdk/plugins/examples/source-report/package/contributions.json";

declare global {
  interface Window {
    setNavigationFixtureEnabled: (enabled: boolean) => Promise<void>;
  }
}
export function installPluginNavigation() {
  const manifest = raw as Manifest;
  const digest = "d".repeat(64);
  let enabled = true;
  const status = (): PluginStatus => ({
    manifest,
    digest,
    enabled,
    signingKey: "e".repeat(64),
    runtimeStatus: enabled ? "active" : "installed",
    activationEpoch: "1",
    grantRevision: "1",
    sourceAuthorized: false,
    connectionAuthorized: false,
    reportAuthorized: true,
    settings: {},
  });
  pluginsApi.list = async () => ({
    apiVersion: "1.0.0",
    runtime: "fixture",
    developmentMode: false,
    items: [status()],
    contributions: {
      schemaVersion: 1,
      items: [
        {
          pluginId: manifest.id,
          digest,
          slots: [],
          dependencies: [],
          navigation: contribution.navigation,
          dataQueries: contribution.dataQueries,
          errorCode: null,
        },
      ],
    },
  });
  pluginsApi.query = async () => ({
    view: manifest.views![0],
    status: status(),
    confirmedEmpty: false,
    report: {
      requests: "0",
      attempts: "0",
      inputTokens: null,
      outputTokens: null,
      totalTokens: null,
      rangeStart: "2026-11-07T00:00:00Z",
      rangeEnd: "2026-11-07T00:00:00Z",
      sources: [
        {
          sourceId: "net.worksha.dsh-desktop:desktop",
          sourceName: "DeepSeek Harness 桌面端",
          sessions: "2",
          totalTokens: "38",
          lease: "f".repeat(64),
          coverage: "complete",
        },
      ],
    },
  });
  window.setNavigationFixtureEnabled = async (next) => {
    enabled = next;
    await pluginDirectoryResource.invalidate();
  };
}
