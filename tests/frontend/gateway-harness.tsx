import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { GatewayPanel } from "@/features/settings/gateway-panel";
import { settingsApi } from "@/features/settings/api";
import type { GatewayStatus } from "@/features/settings/types";
import {
  gatewayStoreA,
  gatewayFixtureOutcome,
} from "./gateway-operation-fixture";
import "@/styles/globals.css";

const mode = new URLSearchParams(location.search).get("mode");
let reads = 0;
let status: GatewayStatus = {
  listen: mode === "disabled" ? null : "192.168.1.10:43210",
  persistent: true,
  concurrency: 16,
  active: 1,
  writeProtocol: {
    version: 1,
    storeId: gatewayStoreA,
    maxClients: 128,
    secretReplay: false,
  },
  clients: [
    {
      id: "11111111-1111-4111-8111-111111111111",
      name: "Laptop A",
      agent: "claude",
      providerId: "fixture",
      models: ["model-a"],
      concurrency: 2,
      active: 1,
      enabled: true,
      createdAt: "2026-10-22T00:00:00Z",
    },
  ],
};
settingsApi.gatewayStatus = async () => {
  reads++;
  if (mode === "error" || (mode === "stale" && reads > 1))
    throw Error("读取失败");
  return structuredClone(status);
};
settingsApi.commitGatewayClientOperation = async (request) => {
  if (request.action.kind !== "revoke") throw Error("fixture expects revoke");
  const id = request.action.clientId;
  status = {
    ...status,
    clients: status.clients.map((c) =>
      c.id === id ? { ...c, enabled: false } : c,
    ),
  };
  return { status: "committed", outcome: gatewayFixtureOutcome(request) };
};
settingsApi.createGatewayClient = async () => ({ status: "cancelled" });
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <main className="p-6">
      <GatewayPanel
        storeId={gatewayStoreA}
        providers={[
          {
            id: "fixture",
            name: "Fixture provider",
            endpoint: "http://fixture.invalid",
            protocol: "messages",
            model: "model-a",
            models: ["model-a"],
            configured: false,
            revision: 0,
            category: "custom",
            credentialRef: null,
            credentialLabel: null,
          },
        ]}
      />
    </main>
  </IconProvider>,
);
