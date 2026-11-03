import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { ExternalRoutingPolicy } from "@/features/model-services/components/external-routing-policy";
import {
  defaultRoutingPolicy,
  type ModelService,
} from "@/features/model-services/types";
import type {
  Action,
  Reply,
  PackageStatus,
  BindingStatus,
  ScopeStatus,
  PublishStatus,
} from "../../contracts/plugins/routing/types";
import manifest from "../../sdk/plugins/examples/pure-policy/package/manifest.json";
import "@/styles/globals.css";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
import type {
  PolicyRecoveryAction,
  PolicyRecoveryEntry,
} from "../../contracts/domain/routing-policy-recovery/types";
const id = (n: number) =>
  "00000000-0000-4000-8000-" + String(n).padStart(12, "0");
const storeId = id(1);
const service: ModelService = {
  id: new URLSearchParams(window.location.search).get("serviceId") ?? id(2),
  versionId: id(3),
  revision: 1,
  name: "编程服务",
  models: [],
  defaultModelId: id(4),
  publishedAt: "2026-11-01T00:00:00Z",
  routingPolicy: { ...defaultRoutingPolicy(), enabled: true },
};
let pkg: PackageStatus = {
  manifest: manifest as PackageStatus["manifest"],
  digest: "a".repeat(64),
  signingKey: "b".repeat(64),
  enabled: false,
  active: false,
  epoch: "1",
  publisherTrusted: true,
};
let installed = false,
  binding: BindingStatus | null = null,
  scope: ScopeStatus | null = null;
const calls: Action[] = [];
const fixture = {
  calls,
  recoveryCalls: [] as PolicyRecoveryAction[],
  recoveryPending: false,
  recoveryBlocked: false,
  recoveryUnknownOnce: false,
  recoveryForeign: false,
  recoveryLegacyIds: false,
  recoveryConsistentOrphan: false,
  recoveryFoldedReference: false,
  holdRecoveryInspect: false,
  releaseRecoveryInspect: () => {},
  reconnect: notifyBackendRecovery,
  failRead: false,
  malformedRegistry: false,
  registryUnknownServiceId: null as string | null,
  wrongScope: false,
  resumeStillUnknown: false,
  cancelPicker: false,
  holdPicker: false,
  releasePicker: () => {},
  unmount: () => {},
  publishes: [] as PublishStatus[],
  seedUnknown: () => {
    fixture.publishes = [
      {
        storeId,
        operationId: id(20),
        serviceId: service.id,
        state: "unknown",
        errorCode: "result_pending",
        binding: null,
        request: {
          storeId,
          operationId: id(20),
          action: {
            kind: "save",
            draft: {
              id: service.id,
              revision: 1,
              name: "原服务配置",
              defaultModelId: id(4),
              models: [],
              routingPolicy: { ...defaultRoutingPolicy(), enabled: true },
            },
          },
        },
      },
    ];
  },
};
declare global {
  interface Window {
    routingPolicyFixture: typeof fixture;
  }
}
window.routingPolicyFixture = fixture;
(globalThis as { isTauri?: boolean }).isTauri = true;
mockIPC(async (name, args) => {
  if (name === "perform_routing_policy_recovery") {
    const input = (args as { input: PolicyRecoveryAction }).input;
    fixture.recoveryCalls.push(structuredClone(input));
    const entry = (serviceId: string): PolicyRecoveryEntry => ({
      scope: { storeId: fixture.recoveryForeign ? id(99) : storeId, serviceId },
      serviceName: fixture.recoveryConsistentOrphan
        ? null
        : serviceId === service.id
          ? service.name
          : null,
      serviceExists:
        !fixture.recoveryConsistentOrphan &&
        (serviceId === service.id || fixture.recoveryLegacyIds),
      state: "unchecked",
      problem: null,
      pendingOperationIds: [],
    });
    if (input.kind === "list")
      return {
        kind: "listed",
        storeId,
        limited: false,
        entries: (fixture.recoveryLegacyIds
          ? [
              "ABCDEFAB-CDEF-4ABC-8DEF-ABCDEFABCDEF",
              "urn:uuid:abcdefab-cdef-1abc-8def-abcdefabcdef",
            ]
          : [service.id, ...Array.from({ length: 6 }, (_, n) => id(100 + n))]
        ).map(entry),
      };
    if (input.kind === "inspect") {
      if (fixture.holdRecoveryInspect)
        await new Promise<void>((resolve) => {
          fixture.releaseRecoveryInspect = resolve;
        });
      const selected = entry(input.scope.serviceId);
      selected.state = fixture.recoveryBlocked ? "blocked" : "needs-recovery";
      selected.problem = fixture.recoveryBlocked
        ? "unavailable"
        : fixture.recoveryPending
          ? "pending-publication"
          : fixture.recoveryConsistentOrphan
            ? null
            : "missing-control";
      selected.pendingOperationIds = fixture.recoveryPending ? [id(20)] : [];
      return {
        kind: "inspected",
        inspection: {
          entry: selected,
          reference: fixture.recoveryBlocked
            ? null
            : {
                scope: {
                  ...selected.scope,
                  serviceId: fixture.recoveryFoldedReference
                    ? selected.scope.serviceId.toLowerCase()
                    : selected.scope.serviceId,
                },
                referenceId: id(200),
              },
          currentServiceVersion: service.versionId,
          canRestoreNative:
            !fixture.recoveryBlocked && !fixture.recoveryPending,
          canRestorePending:
            !fixture.recoveryBlocked && fixture.recoveryPending,
        },
      };
    }
    if (fixture.recoveryUnknownOnce) {
      fixture.recoveryUnknownOnce = false;
      throw JSON.stringify({
        code: "scope_recovery_unknown",
        outcomeUnknown: true,
        message: "恢复结果待核查，请保留原核查编号。",
      });
    }
    return {
      kind: "resolved",
      reference: input.reference,
      mode: input.mode,
      revision: id(201),
    };
  }
  if (name !== "perform_routing_policy")
    throw Error("Unexpected command: " + name);
  const input = (args as { input: Action }).input;
  calls.push(structuredClone(input));
  let reply: Reply;
  switch (input.kind) {
    case "list":
      if (fixture.failRead) throw Error("策略目录读取失败");
      return {
        kind: "inventory",
        storeId: fixture.wrongScope ? id(99) : storeId,
        packages: installed ? [pkg] : [],
        bindings: binding ? [binding] : [],
        scopes: scope ? [scope] : [],
        publishes: fixture.publishes,
        unavailableServiceIds: fixture.malformedRegistry
          ? [fixture.registryUnknownServiceId ?? service.id]
          : [],
      };
    case "inspect":
      if (fixture.holdPicker)
        await new Promise<void>((r) => {
          fixture.releasePicker = r;
        });
      if (fixture.cancelPicker) return null;
      return {
        kind: "review",
        review: {
          packageRef: id(5),
          digest: pkg.digest,
          manifest: pkg.manifest,
          signingKey: pkg.signingKey,
          trustedPublisher: false,
          compressedBytes: "100",
          expandedBytes: "200",
        },
      };
    case "install":
      installed = true;
      reply = { kind: "package", status: pkg };
      break;
    case "state":
      pkg = {
        ...pkg,
        enabled: input.action === "enable",
        active: input.action === "enable",
        epoch: String(Number(pkg.epoch) + 1),
      };
      reply = { kind: "package", status: pkg };
      break;
    case "bind":
      binding = {
        storeId,
        serviceId: input.serviceId,
        serviceVersionId: input.serviceVersionId,
        revision: crypto.randomUUID(),
        package: input.package,
        failureMode: input.failureMode,
        authorized: true,
        errorCode: null,
      };
      scope = {
        storeId,
        serviceId: input.serviceId,
        revision: binding.revision,
        pendingOperationId: null,
        errorCode: null,
      };
      reply = { kind: "binding", binding };
      break;
    case "unbind":
      binding = null;
      scope = {
        storeId,
        serviceId: input.serviceId,
        revision: crypto.randomUUID(),
        pendingOperationId: null,
        errorCode: null,
      };
      reply = {
        kind: "unbound",
        storeId,
        serviceId: input.serviceId,
        revision: scope.revision,
      };
      break;
    case "resume-publish": {
      const original = fixture.publishes.find(
        (p) => p.operationId === input.operationId,
      );
      if (!original) throw Error("Unknown publication");
      if (!fixture.resumeStillUnknown) {
        original.state = "committed";
        original.errorCode = null;
      }
      reply = {
        kind: "publish",
        status: {
          ...original,
          storeId: fixture.wrongScope ? id(99) : original.storeId,
        },
      };
      break;
    }
    default:
      throw Error("fixture action not configured");
  }
  return structuredClone(reply);
});
const root = createRoot(document.getElementById("root")!);
fixture.unmount = () => root.unmount();
root.render(
  <IconProvider>
    <div className="p-4">
      <ExternalRoutingPolicy storeId={storeId} services={[service]} />
    </div>
  </IconProvider>,
);
