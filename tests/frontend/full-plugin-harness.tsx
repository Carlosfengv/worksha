import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { FullPluginManager } from "@/features/plugins/full-manager";
import { fullPluginsApi } from "@/features/plugins/api";
import { domainApi } from "@/features/domain/api";
import manifestJson from "../../sdk/plugins/examples/full-history/package/manifest.json";
import type { Manifest, Status } from "../../contracts/plugins/sources/types";
import "@/styles/globals.css";

const manifest = manifestJson as Manifest;
let status: Status = {
  manifest,
  digest: "a".repeat(64),
  signingKey: "b".repeat(64),
  enabled: false,
  runtimeActive: false,
  publisherTrusted: true,
  activationEpoch: "1",
  grantRevision: "1",
  sourceAuthorized: false,
  sourceLabel: null,
  unifiedAuthorized: false,
  persistMetadata: false,
  freshness: "unread",
  factSummary: null,
  problem: null,
};
let installed = false;
const calls: { operation: string; input?: unknown }[] = [];
fullPluginsApi.list = async () => {
  calls.push({ operation: "list" });
  return {
    apiVersion: "1.0.0",
    items: installed ? [structuredClone(status)] : [],
  };
};
fullPluginsApi.inspect = async () => {
  calls.push({ operation: "inspect" });
  return {
    packageRef: "review-one",
    digest: status.digest,
    manifest,
    signingKey: status.signingKey,
    trustedPublisher: false,
    compressedBytes: "100",
    expandedBytes: "200",
  };
};
fullPluginsApi.install = async (input) => {
  calls.push({ operation: "install", input });
  installed = true;
  return structuredClone(status);
};
fullPluginsApi.change = async (input) => {
  calls.push({ operation: "state", input });
  status = {
    ...status,
    grantRevision: String(BigInt(status.grantRevision) + 1n),
  };
  if (input.action === "enable") status.enabled = status.runtimeActive = true;
  if (input.action === "disable") {
    status.enabled = status.runtimeActive = false;
    status.sourceAuthorized = status.unifiedAuthorized = false;
  }
  if (input.action === "revoke-grants")
    status.sourceAuthorized = status.unifiedAuthorized = false;
  return structuredClone(status);
};
domainApi.authorizeSource = async (query) => {
  calls.push({ operation: "authorize", input: query });
  status.sourceAuthorized = true;
  status.sourceLabel = "/managed/fixture.jsonl";
  status.grantRevision = String(BigInt(status.grantRevision) + 1n);
  return { updated: true };
};
domainApi.refreshSource = async (query) => {
  calls.push({ operation: "refresh", input: query });
  status.freshness = "current";
  return { updated: true };
};
domainApi.prepareConsent = async (query) => ({
  ...query,
  reviewRef: "consent-current",
  resourceLabel: status.sourceLabel,
  sourceAuthorized: status.sourceAuthorized,
  unifiedAuthorized: status.unifiedAuthorized,
  persistMetadata: status.persistMetadata,
});
domainApi.consent = async (input) => {
  calls.push({ operation: "consent", input });
  status.unifiedAuthorized = input.enabled;
  status.persistMetadata = input.persistMetadata;
  return { updated: true };
};
declare global {
  interface Window {
    fullPluginFixture: { calls: typeof calls };
  }
}
window.fullPluginFixture = { calls };
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <main className="h-dvh overflow-y-auto p-4">
      <FullPluginManager />
    </main>
  </IconProvider>,
);
