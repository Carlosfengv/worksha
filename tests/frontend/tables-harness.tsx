import { useState, useSyncExternalStore } from "react";
import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import App from "@/app/App";
import { ProviderTable } from "@/features/providers/components/provider-table";
import { ProviderDetail } from "@/features/providers/components/provider-detail";
import { ProviderEditor } from "@/features/providers/components/provider-editor";
import { RequestsTable } from "@/features/observation/components/requests-table";
import { AgentCallsTable } from "@/features/agents/components/agent-calls-table";
import { McpCallsTable } from "@/features/mcp/components/mcp-calls-table";
import { PROVIDER_FIXTURE_STORE_ID } from "./provider-operation-fixture";
import { requestRecord } from "./request-fixture";
import {
  providers,
  requests,
  mcpCalls,
  mcpGroup,
} from "./fixtures/tables/data";
import {
  installTableFixtures,
  applicationTableModes,
} from "./fixtures/tables/install";
import "@/styles/globals.css";
const fixture = installTableFixtures();
function Harness() {
  const [connectionRows, setConnectionRows] = useState(providers);
  const provider = useSyncExternalStore(
    fixture.provider.subscribe,
    fixture.provider.getProvider,
  );
  const [editing, setEditing] = useState(false);
  const mode = fixture.mode;
  if (applicationTableModes.has(mode ?? "")) return <App />;
  return (
    <IconProvider>
      <main className="min-w-0 p-4">
        {mode === "providers" ? (
          <ProviderTable
            providers={connectionRows}
            status={null}
            onEdit={() => setEditing(true)}
            onVerify={() => undefined}
          />
        ) : mode === "requests" ? (
          <RequestsTable rows={requests.map(requestRecord)} />
        ) : mode === "loading" ? (
          <AgentCallsTable items={[]} agentId="claude" loading />
        ) : mode === "mcp" ? (
          <McpCallsTable
            items={mcpCalls}
            selected={mcpGroup}
            scope="临时夹具"
            asOf="2026-10-19T00:00:00Z"
          />
        ) : (
          <ProviderDetail
            provider={provider}
            storeId={PROVIDER_FIXTURE_STORE_ID}
            status={null}
            onEdit={() => setEditing(true)}
            onVerify={() => undefined}
            onUpdated={async () => undefined}
          />
        )}
        {mode === "providers" && (
          <button onClick={() => setConnectionRows(providers.slice(0, 3))}>
            模拟目录减少
          </button>
        )}
        {editing && (
          <ProviderEditor
            provider={provider}
            storeId={PROVIDER_FIXTURE_STORE_ID}
            onClose={() => setEditing(false)}
            onSaved={async () => setEditing(false)}
          />
        )}
      </main>
    </IconProvider>
  );
}
const root = document.getElementById("root");
if (!root) throw Error("Missing fixture root");
createRoot(root).render(<Harness />);
