import { mockIPC } from "@tauri-apps/api/mocks";
import { providersApi, providerSourceApi } from "@/features/providers/api";
declare global {
  interface Window {
    providerResponseFixture: {
      client: typeof providersApi;
      source: typeof providerSourceApi;
      reply: unknown;
      calls: { name: string; args: unknown }[];
    };
  }
}
Object.defineProperty(globalThis, "isTauri", {
  value: true,
  configurable: true,
});
window.providerResponseFixture = {
  client: providersApi,
  source: providerSourceApi,
  reply: null,
  calls: [],
};
mockIPC((name, args) => {
  window.providerResponseFixture.calls.push({ name, args });
  return window.providerResponseFixture.reply;
});
