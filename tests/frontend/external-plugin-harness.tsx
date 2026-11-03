import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { PluginManager } from "@/features/plugins/manager";
import { ExtensionsPage } from "@/features/plugins/page";
import { pluginsApi } from "@/features/plugins/api";
import type {
  Manifest,
  PluginStatus,
  PluginView,
} from "@/features/plugins/types";
import rawManifest from "../../sdk/plugins/examples/aurora-agent/package/manifest.json";
import { parseContract } from "@/features/plugins/contract";
import "@/styles/globals.css";

// Business API stubs only: this harness never provides a browser-to-native bridge.
const mode = new URLSearchParams(location.search).get("mode") ?? "manager";
const managerMode = mode === "manager" || mode.startsWith("picker-");
const manifest = parseContract<Manifest>("manifest", rawManifest);
let current: PluginStatus = {
  manifest,
  digest: "d".repeat(64),
  signingKey: "a".repeat(64),
  enabled: !managerMode,
  runtimeStatus: managerMode ? "installed" : "active",
  errorCode: null,
  activationEpoch: "1",
  grantRevision: "1",
  sourceAuthorized: !managerMode,
  connectionAuthorized: false,
  reportAuthorized: false,
  sourceLabel: mode.startsWith("source-") ? "existing.jsonl" : null,
  connectionLabel: null,
  previousDigest: null,
  settings: { "title-style": "original" },
};
let items: PluginStatus[] = managerMode || mode === "removed" ? [] : [current];
let refreshed = false;
const calls: string[] = [];
const record = (name: string) => {
  calls.push(name);
  document.getElementById("calls")!.textContent = JSON.stringify(calls);
};
pluginsApi.list = async () => {
  if (mode === "error") throw Error("目录读取失败");
  return {
    items: structuredClone(items),
    apiVersion: "1.0.0",
    developmentMode: false,
    runtime: "wasmtime-36.0.19",
  };
};
let packageSelections = 0;
pluginsApi.inspect = async () => {
  packageSelections += 1;
  if (mode === "picker-cancel") {
    record("package-picker-cancel");
    return null;
  }
  if (mode === "picker-error-retry" && packageSelections === 1) {
    record("package-picker-error");
    throw Error("原生文件选择未完成");
  }
  return {
    packageRef: "package-review",
    digest: current.digest,
    manifest,
    signingKey: current.signingKey,
    trustedPublisher: false,
    development: false,
    compressedBytes: 1000,
    expandedBytes: 2000,
  };
};
pluginsApi.install = async (input) => {
  if (!input.approvePublisher) throw Error("未信任");
  record("install");
  items = [current];
  return current;
};
pluginsApi.change = async (input) => {
  record(input.action);
  current = {
    ...current,
    enabled:
      input.action === "enable"
        ? true
        : input.action === "disable"
          ? false
          : current.enabled,
    sourceAuthorized: false,
    grantRevision: String(Number(current.grantRevision) + 1),
  };
  items = input.action === "remove" ? [] : [current];
  return current;
};
let sourceSelections = 0;
pluginsApi.authorizeSource = async () => {
  sourceSelections += 1;
  if (mode === "source-cancel") {
    record("source-picker-cancel");
    return null;
  }
  if (mode === "source-error-retry" && sourceSelections === 1) {
    record("source-picker-error");
    throw Error("无法读取所选文件");
  }
  record("authorize");
  current = {
    ...current,
    sourceAuthorized: true,
    sourceLabel: "fixture.jsonl",
    grantRevision: String(Number(current.grantRevision) + 1),
  };
  items = [current];
  return current;
};
pluginsApi.query = async () => {
  const view: PluginView = {
    view: manifest.views![0],
    status: current,
    confirmedEmpty: false,
    needsRefresh: !refreshed,
    snapshot: refreshed ? "snapshot-1" : null,
    items: refreshed
      ? [
          {
            sessionId: manifest.id + ":1",
            record: {
              nativeId: "1",
              title: "离线扩展会话",
              model: null,
              createdAt: null,
              usage: {
                inputTokens: "9007199254740993",
                outputTokens: "7",
                cacheReadTokens: null,
                reasoningTokens: null,
              },
            },
          },
        ]
      : [],
    total: refreshed ? 1 : 0,
  };
  if (refreshed)
    view.summary = {
      knownUsageRecords: "1",
      inputTokens: "9007199254740993",
      outputTokens: "7",
      totalTokens: "9007199254741000",
    };
  return structuredClone(view);
};
pluginsApi.action = async (input) => {
  record(input.action.kind);
  if (input.action.kind === "refresh") {
    refreshed = true;
    return { refreshed: true };
  }
  if (input.action.kind === "read-session") {
    await new Promise((resolve) =>
      setTimeout(resolve, mode === "race" ? 400 : 10),
    );
    return {
      nativeId: "1",
      messages: [
        {
          role: "assistant",
          text: "<img src=x onerror=alert(1)> 正文按纯文本显示",
        },
      ],
    };
  }
  return {};
};
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <main className="p-6">
      <output id="calls" className="sr-only">
        []
      </output>
      {managerMode ? (
        <PluginManager providers={[]} />
      ) : (
        <ExtensionsPage
          pluginId={manifest.id}
          viewId="history"
          providers={[]}
          refreshSnapshot={async () => {}}
        />
      )}
    </main>
  </IconProvider>,
);
