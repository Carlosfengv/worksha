import { useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { IconProvider } from "@/lib/icon-context";
import { GatewayPanel } from "@/features/settings/gateway-panel";
import { gatewayStatusResource } from "@/features/settings/queries";
import {
  readGatewayRecovery,
  rememberGatewayOperation,
  forgetGatewayOperation,
} from "@/features/settings/pending-gateway";
import type {
  GatewayOperationRequest,
  PendingGatewayOperation,
  GatewayStatus,
} from "@/features/settings/types";
import {
  gatewayFixtureOutcome,
  gatewayFixtureStatus,
  gatewayStoreA,
} from "./gateway-operation-fixture";
import "@/styles/globals.css";
type Pending = {
  args: Record<string, unknown>;
  resolve: (value: unknown) => void;
  reject: (code?: string, outcomeUnknown?: boolean) => void;
  persisted: ReturnType<typeof readGatewayRecovery>;
};
declare global {
  interface Window {
    gatewayOps: {
      writes: Pending[];
      revokes: Pending[];
      checks: Pending[];
      calls: { name: string; args: unknown }[];
      read: typeof readGatewayRecovery;
      remember: typeof rememberGatewayOperation;
      forget: typeof forgetGatewayOperation;
      outcome: typeof gatewayFixtureOutcome;
      publish: (storeId: string, root?: boolean) => Promise<void>;
      available: (value: boolean) => void;
      failStatus: boolean;
      legacy: boolean;
      rawStatus?: unknown;
      storage: (mode: "none" | "set" | "remove" | "unavailable") => void;
    };
  }
}
Object.defineProperty(globalThis, "isTauri", {
  value: true,
  configurable: true,
});
let status: GatewayStatus = gatewayFixtureStatus();
const fixture: Window["gatewayOps"] = {
  writes: [] as Pending[],
  revokes: [] as Pending[],
  checks: [] as Pending[],
  calls: [] as { name: string; args: unknown }[],
  read: readGatewayRecovery,
  remember: rememberGatewayOperation,
  forget: forgetGatewayOperation,
  outcome: gatewayFixtureOutcome,
  publish: async () => {},
  available: () => {},
  failStatus: false,
  legacy: false,
  rawStatus: undefined as unknown,
  storage: () => {},
};
let storageMode = "none";
const savedStorage = window.localStorage,
  set = Storage.prototype.setItem,
  remove = Storage.prototype.removeItem;
Object.defineProperty(window, "localStorage", {
  configurable: true,
  get: () => {
    if (storageMode === "unavailable")
      throw Error("fixture storage unavailable");
    return savedStorage;
  },
});
Storage.prototype.setItem = function (key, value) {
  if (
    key.startsWith("local-agent.gateway-operation.v1.") &&
    storageMode === "set"
  )
    throw Error("fixture quota");
  set.call(this, key, value);
};
Storage.prototype.removeItem = function (key) {
  if (
    key.startsWith("local-agent.gateway-operation.v1.") &&
    storageMode === "remove"
  )
    throw Error("fixture remove");
  remove.call(this, key);
};
fixture.storage = (mode) => {
  storageMode = mode;
  window.dispatchEvent(new Event("gateway-operations-changed"));
};
window.gatewayOps = fixture;
const defer = (args: unknown, target: Pending[]) => {
  if (
    !args ||
    typeof args !== "object" ||
    Array.isArray(args) ||
    ArrayBuffer.isView(args) ||
    args instanceof ArrayBuffer
  )
    throw Error("Expected named IPC arguments");
  return new Promise((resolve, reject) =>
    target.push({
      args: structuredClone(args) as Record<string, unknown>,
      persisted: readGatewayRecovery(),
      resolve,
      reject: (code = "transport_timeout", outcomeUnknown = true) =>
        reject({ code, outcomeUnknown, message: code }),
    }),
  );
};
mockIPC((name, args) => {
  fixture.calls.push({ name, args: structuredClone(args) });
  if (name === "get_gateway_status") {
    if (fixture.failStatus)
      return Promise.reject({
        code: "unavailable",
        outcomeUnknown: false,
        message: "fixture status unavailable",
      });
    if (fixture.rawStatus !== undefined)
      return structuredClone(fixture.rawStatus);
    const value = structuredClone(status);
    if (fixture.legacy) delete value.writeProtocol;
    return value;
  }
  if (name === "save_gateway_client_operation")
    return defer(args ?? {}, fixture.writes);
  if (name === "commit_gateway_client_operation")
    return defer(args ?? {}, fixture.revokes);
  if (name === "get_gateway_client_operation")
    return defer(args ?? {}, fixture.checks);
  throw Error("Unexpected fixture command " + name);
});
function Harness() {
  const [rootScope, setRootScope] = useState(gatewayStoreA),
    [available, setAvailable] = useState(true),
    [visible, setVisible] = useState(true);
  fixture.available = setAvailable;
  fixture.publish = async (storeId, root = true) => {
    status = gatewayFixtureStatus(storeId);
    if (root) setRootScope(storeId);
    await gatewayStatusResource.invalidate();
  };
  return (
    <IconProvider>
      <main className="p-6">
        <button onClick={() => setVisible((v) => !v)}>切换客户端挂载</button>
        {visible && (
          <GatewayPanel
            storeId={available ? rootScope : undefined}
            providers={
              available
                ? [
                    {
                      id: "fixture",
                      name: "Fixture provider",
                      endpoint: "http://fixture.invalid",
                      protocol: "messages",
                      model: "fixture-model",
                      models: ["fixture-model"],
                      configured: false,
                      revision: 0,
                      category: "custom",
                      credentialRef: "must-not-persist",
                      credentialLabel: null,
                    },
                  ]
                : []
            }
          />
        )}
      </main>
    </IconProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
export type { GatewayOperationRequest, PendingGatewayOperation };
