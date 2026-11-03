import { useState } from "react";
import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { ProvidersPage } from "@/features/providers/page";
import { providersApi } from "@/features/providers/api";
import type {
  Provider,
  ProviderVerification,
} from "@/features/providers/types";
import type { Snapshot } from "@/app/types";
import "@/styles/globals.css";

const failRefresh = new URLSearchParams(location.search).has("fail-refresh");
let provider: Provider = {
  id: "notice-provider",
  name: "Owned notice fixture",
  vendor: "custom",
  endpoint: "http://127.0.0.1:9/v1",
  protocol: "responses",
  model: "model-a",
  models: ["model-a", "model-b"],
  configured: true,
  credentialRef: null,
  credentialLabel: null,
  revision: 1,
  category: "local",
};
let sequence = 0;
const verifications: ProviderVerification[] = [];
const protocol = () => ({
  version: 1 as const,
  storeId: "6d6f20a2-8b15-48c9-a792-894fc50e34e5",
  sequence,
  receiptLimit: 256,
  receiptBytes: 2097152,
  pendingLimit: 4,
  pendingOperationIds: [],
});
providersApi.writeProtocol = async () => protocol();
providersApi.providerStatus = async () => ({
  configs: [],
  transactions: [],
  verifications: [...verifications],
  runtimes: [],
  backupProtection: "owned fixture",
});
providersApi.verifyProvider = async (_id, revision) => {
  if (revision !== provider.revision) throw Error("Owned revision mismatch");
  const result: ProviderVerification = {
    providerId: provider.id,
    model: provider.model,
    fingerprint: `owned-${revision}-${provider.model}`,
    state: "verified",
    checkedAt: new Date().toISOString(),
    message: `验证成功：${provider.model}`,
  };
  verifications.unshift(result);
  return result;
};
providersApi.commitOperation = async (request) => {
  provider = { ...request.provider, revision: provider.revision + 1 };
  sequence++;
  return {
    apiVersion: 1,
    sequence,
    status: "committed",
    request,
    provider,
    verification: null,
    error: null,
  };
};
function Harness() {
  const [data, setData] = useState<Snapshot>({
    version: "fixture",
    databasePath: "temporary",
    gatewayStatus: "ready",
    agents: [],
    providers: [provider],
    providerWriteProtocol: protocol(),
    settings: { captureContent: false, retentionDays: 30 },
    sessionSources: [],
    observationBindings: [],
  });
  return (
    <IconProvider>
      <main className="min-w-0 p-4">
        <ProvidersPage
          data={data}
          selectedId={provider.id}
          refresh={async () => {
            if (failRefresh) throw Error("Owned snapshot read unavailable");
            setData((old) => ({
              ...old,
              providers: [provider],
              providerWriteProtocol: protocol(),
            }));
          }}
        />
      </main>
    </IconProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
