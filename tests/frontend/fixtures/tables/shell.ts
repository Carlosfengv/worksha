import { appApi } from "@/app/api";
import { agentProducts } from "@/shared/types/agent";
import { sessionsApi } from "@/features/sessions/api";
import type { SessionSource } from "@/features/sessions/types";
import { providersApi } from "@/features/providers/api";
import { accountsApi } from "@/features/accounts/api";
import { settingsApi } from "@/features/settings/api";
import { pluginsApi } from "@/features/plugins/api";
import { backendDiagnostics } from "@/features/settings/backend-result";
import { fixtureProvider } from "./data";
import { checkedTableSnapshot, type TableFixtureContext } from "./context";
export function installTableShell(context: TableFixtureContext) {
  const { sourceState } = context;
  const openCodeSource: SessionSource = {
    agentId: "opencode",
    enabled: true,
    locations: [],
    status: sourceState === "ready" ? "ready" : "partial",
    sessionCount: 29,
    scannedFiles: sourceState === "failed" ? 0 : 29,
    totalFiles: 29,
    skippedFiles: sourceState === "partial" ? 1 : 0,
    lastScan: "2026-10-19T00:00:00Z",
    error: sourceState === "ready" ? null : "读取范围的具体错误",
    versions: [],
    capabilities: [],
  };
  appApi.snapshot = async () =>
    checkedTableSnapshot({
      agentConnections:
        new URLSearchParams(location.search).get("mode") === "activity"
          ? {
              checkedAt: new Date().toISOString(),
              error: null,
              items: (
                ["claude", "claude-desktop", "codex", "opencode"] as const
              ).map((agentId) => ({
                agentId,
                mode: window.activityConnection,
                providerName:
                  window.activityConnection === "official_login"
                    ? "官方"
                    : "MyDeepSeek",
                model:
                  window.activityConnection === "official_login"
                    ? agentId === "codex"
                      ? "gpt-6.1-sol"
                      : "claude-sonnet-5"
                    : agentId === "claude-desktop"
                      ? "claude-sonnet-5-local-agent-1234567890"
                      : agentId === "opencode"
                        ? "local_agent_fixture/deepseek-flash"
                        : "deepseek-flash",
                endpoint: null,
                reason: null,
                activeRequests: 0,
              })),
            }
          : undefined,
      hiddenProjects: context.hiddenProjects,
      version: "fixture",
      databasePath: "/tmp/fixture",
      gatewayStatus: "not_configured",
      // Match the service's registered product directory in every feature mode.
      agents: agentProducts.map((p) => ({
        ...p,
        found: false,
        location: null,
        connected: false,
      })),
      providers:
        new URLSearchParams(location.search).get("mode") === "provider-history"
          ? [fixtureProvider(0), fixtureProvider(1)]
          : [fixtureProvider(0)],
      settings: { captureContent: false, retentionDays: 30 },
      sessionSources:
        new URLSearchParams(location.search).get("mode") === "activity"
          ? (["claude", "codex", "opencode"] as const).map((agentId) => ({
              ...openCodeSource,
              agentId,
              enabled: sourceState !== "disabled",
              status: sourceState === "disabled" ? "disabled" : "ready",
              error: null,
            }))
          : sourceState
            ? [openCodeSource]
            : [],
      observationBindings: [],
    });
  sessionsApi.sessions = async () => ({
    items: [],
    total: 0,
    nextCursor: null,
    summary: null,
    asOf: "2026-10-19T00:00:00Z",
    coverage: "not_connected",
    missingCounts: 0,
  });
  providersApi.providerStatus = async () => ({
    configs: [],
    transactions: [],
    backupProtection: "not_created",
    verifications: [],
    runtimes: [],
  });
  accountsApi.accounts = async () => ({
    accounts: [],
    asOf: "2026-10-19T00:00:00Z",
  });
  // Existing domain fixtures have no installed extensions. Keep the new read-only
  // directory boundary explicit instead of invoking a native bridge in the browser.
  pluginsApi.list = async () => ({
    items: [],
    apiVersion: "1.0.0",
    runtime: "wasmtime-36.0.19",
    developmentMode: false,
    contributions: { schemaVersion: 1, items: [] },
  });
  if (new URLSearchParams(location.search).get("mode") === "shell") {
    // Supply only the read-only diagnostics boundary; settings writes remain
    // unavailable without a database-scoped write protocol.
    const diagnostics = backendDiagnostics({
      state: "connected",
      verifiedAt: Date.UTC(2026, 9, 30),
      canReconnect: false,
      events: Array.from({ length: 32 }, (_, index) => ({
        id: `00000000-0000-4000-8000-${String(index + 1).padStart(12, "0")}`,
        operationId: "00000000-0000-4000-8000-000000000100",
        at: Date.UTC(2026, 9, 30) - index * 1000,
        phase: "startup",
        category: "verified",
        exitCode: null,
        signal: null,
      })),
    });
    settingsApi.backendDiagnostics = async () => diagnostics;
  }
}
