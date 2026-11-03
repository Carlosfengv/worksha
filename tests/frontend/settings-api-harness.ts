import { mockIPC } from "@tauri-apps/api/mocks";
import { settingsApi } from "@/features/settings/api";
import type { SettingsUpdate } from "@/features/settings/types";
declare global {
  interface Window {
    settingsScopedClient: typeof settingsApi;
    settingsScopedCommands: { name: string; args: unknown }[];
    releaseSettingsProbe: () => void;
    mutableSettingsRequest?: SettingsUpdate;
    pendingSettingsSave?: Promise<unknown>;
  }
}
Object.defineProperty(globalThis, "isTauri", {
  value: true,
  configurable: true,
});
window.settingsScopedClient = settingsApi;
window.settingsScopedCommands = [];
window.releaseSettingsProbe = () => {};
const mode = new URLSearchParams(location.search).get("mode");
mockIPC((name, args) => {
  window.settingsScopedCommands.push({ name, args: structuredClone(args) });
  const input = args as Record<string, unknown>;
  const fail = (code: string) =>
    JSON.stringify({ code, message: code, outcomeUnknown: false });
  if (name === "get_scoped_settings_operation") {
    if (mode === "probe-timeout") throw fail("transport_timeout");
    if (mode === "unsupported") throw fail("settings_protocol_unavailable");
    if (mode === "wrong-store") throw fail("operation_store_changed");
    if (mode === "held")
      return new Promise((_resolve, reject) => {
        window.releaseSettingsProbe = () => reject(fail("operation_not_found"));
      });
    if (mode === "existing")
      return {
        storeId: input.storeId,
        operationId: input.operationId,
        revision: 1,
        value: { captureContent: false, retentionDays: 7 },
      };
    throw fail("operation_not_found");
  }
  if (name !== "save_settings") throw fail("unexpected_command");
  if (mode === "conflict") throw fail("operation_conflict");
  const request = input.settings as SettingsUpdate;
  return {
    storeId:
      mode === "malformed"
        ? "00000000-0000-4000-8000-000000000002"
        : request.storeId,
    operationId: request.operationId,
    revision: request.expectedRevision + 1,
    value: request.value,
  };
});
