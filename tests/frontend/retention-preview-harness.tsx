import { useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { IconProvider } from "@/lib/icon-context";
import { RetentionPreviewPanel } from "@/features/settings/components/retention-preview";
import { settingsApi } from "@/features/settings/api";
import type {
  RetentionRequest,
  GatewayImpact,
} from "@/features/settings/retention-result";
import type {
  RetentionInventory,
  SourceReport,
} from "@/features/settings/retention-inventory";
import type { Settings } from "@/features/settings/types";
import type { RetentionContentInventory } from "@/features/settings/retention-content-inventory";
import "@/styles/globals.css";

type Call = {
  request: RetentionRequest & { inventoryVersion?: 1 | 2 };
  resolve: (
    patch?: Partial<Omit<RetentionInventory, "gateway">> & {
      gateway?: GatewayImpact | SourceReport<GatewayImpact>;
      activityCaches?: RetentionContentInventory["activityCaches"];
    },
  ) => void;
  reject: (code: string) => void;
};
declare global {
  interface Window {
    retentionCalls: Call[];
    retentionCommands: string[];
    retentionClient: typeof settingsApi;
    retentionScope: (
      storeId: string | undefined,
      revision: number,
      days?: Settings["retentionDays"],
    ) => void;
  }
}
Object.defineProperty(globalThis, "isTauri", {
  value: true,
  configurable: true,
});
window.retentionCalls = [];
window.retentionCommands = [];
window.retentionClient = settingsApi;
mockIPC((name, args) => {
  window.retentionCommands.push(name);
  if (name !== "preview_retention" && name !== "preview_retention_inventory")
    throw "unexpected command";
  const request = (args as { request: Call["request"] }).request;
  return new Promise((resolve, reject) => {
    window.retentionCalls.push({
      request,
      resolve: (patch = {}) => {
        const now = Date.now();
        const gateway: GatewayImpact = {
          attempts: 10,
          logicalRequests: 8,
          expired: 2,
          withinWindow: 3,
          protected: {
            active: 1,
            unknownOutcome: 1,
            unknownTime: 1,
            unsupportedSource: 0,
          },
          unreadableAttempts: 0,
          classificationComplete: true,
        };
        const checked = <T,>(value: T): SourceReport<T> => ({
          status: "checked",
          readStartedAt: new Date(now).toISOString(),
          readFinishedAt: new Date(now).toISOString(),
          value,
        });
        const metadata = {
          version: 1,
          storeId: request.storeId,
          settingsRevision: request.expectedSettingsRevision,
          days: request.days,
          capturedAt: new Date(now - 5000).toISOString(),
          cutoff: new Date(
            now - 5000 - request.days * 86_400_000,
          ).toISOString(),
          validUntil: new Date(now - 5000 + 300_000).toISOString(),
          cleanupEnabled: false,
          extraArchiveEnabled: false,
          confirmationAvailable: false,
          aggregateRetention: "continuous",
          pendingWrites: 0,
          writerIssue: false,
        };
        if (name === "preview_retention") {
          resolve({
            ...metadata,
            scope: "committed_gateway_only",
            otherSources: "not_checked",
            gateway,
            ...patch,
          });
          return;
        }
        const supplied = patch.gateway;
        resolve({
          ...metadata,
          ...(request.inventoryVersion === 2
            ? {
                version: 2,
                scope: "product_owned_saved_sources",
                activityCaches: {
                  gateway: checked({
                    records: 0,
                    olderThanCutoff: 0,
                    withinWindow: 0,
                    unknownTime: 0,
                    fileBytes: 23,
                  }),
                  native: checked({
                    records: 0,
                    olderThanCutoff: 0,
                    withinWindow: 0,
                    unknownTime: 0,
                    fileBytes: 23,
                  }),
                },
              }
            : { scope: "product_owned_saved_metadata" }),
          remainingSources: "not_checked",
          toolHistory: "saved_only_prior_history_unknown",
          gateway: checked(gateway),
          tools: checked({
            records: 10,
            expired: 3,
            withinWindow: 1,
            protected: {
              active: 1,
              unknownOutcome: 4,
              unknownTime: 1,
              unsupportedSource: 0,
            },
            unreadableRecords: 0,
            classificationComplete: true,
            capacityLimit: 10_000,
          }),
          ...patch,
          ...(supplied
            ? { gateway: "status" in supplied ? supplied : checked(supplied) }
            : {}),
        });
      },
      reject: (code) =>
        reject(JSON.stringify({ code, message: code, outcomeUnknown: false })),
    });
  });
});
function Harness() {
  const [scope, setScope] = useState({
    storeId: "00000000-0000-4000-8000-000000000001" as string | undefined,
    revision: 1,
    days: 30 as Settings["retentionDays"],
  });
  window.retentionScope = (storeId, revision, days = 30) =>
    setScope({ storeId, revision, days });
  return (
    <RetentionPreviewPanel
      protocol={
        scope.storeId
          ? { version: 1, storeId: scope.storeId, receiptLimit: 256 }
          : undefined
      }
      revision={scope.revision}
      days={scope.days}
    />
  );
}
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <Harness />
  </IconProvider>,
);
