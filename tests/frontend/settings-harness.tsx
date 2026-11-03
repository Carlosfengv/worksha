import { useState } from "react";
import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { SettingsPreferences } from "@/features/settings/components/settings-preferences";
import { settingsApi, SettingsPreflightError } from "@/features/settings/api";
import type {
  Settings,
  SettingsUpdate,
  ScopedSettingsSaved,
  SettingsWriteProtocol,
} from "@/features/settings/types";
import { DesktopError } from "@/shared/lib/desktop-command";
import { scopedSettingsReceipt } from "@/features/settings/save-result";
import {
  readSettingsRecovery,
  rememberSettingsOperation,
  forgetSettingsOperation,
  requireSettingsRecovery,
} from "@/features/settings/pending-settings";
import "@/styles/globals.css";

type Pending = {
  request: SettingsUpdate;
  resolve: (result: ScopedSettingsSaved) => void;
  reject: (code: string, outcomeUnknown?: boolean, preflight?: boolean) => void;
};
declare global {
  interface Window {
    settingsWrites: Pending[];
    settingsChecks: Pending[];
    publishSettings: (
      value: Settings,
      revision: number,
      storeId?: string,
    ) => void;
    setSettingsAvailable: (available: boolean) => void;
    settingsRecovery: {
      read: typeof readSettingsRecovery;
      remember: typeof rememberSettingsOperation;
      forget: typeof forgetSettingsOperation;
      require: typeof requireSettingsRecovery;
    };
  }
}
window.settingsWrites = [];
window.settingsChecks = [];
window.settingsRecovery = {
  read: readSettingsRecovery,
  remember: rememberSettingsOperation,
  forget: forgetSettingsOperation,
  require: requireSettingsRecovery,
};
const pending = (request: SettingsUpdate, target: Pending[]) =>
  new Promise<ScopedSettingsSaved>((resolve, reject) =>
    target.push({
      request,
      resolve,
      reject: (code, outcomeUnknown, preflight) => {
        const error = new DesktopError({ code, message: code, outcomeUnknown });
        reject(preflight ? new SettingsPreflightError(error) : error);
      },
    }),
  ).then((value) => scopedSettingsReceipt(value, request));
settingsApi.saveSettings = (request) => pending(request, window.settingsWrites);
settingsApi.scopedSettingsOperation = (request) =>
  pending(request, window.settingsChecks);

function Harness() {
  const [state, setState] = useState({
    value: { captureContent: false, retentionDays: 30 } as Settings,
    revision: 0,
    protocol: {
      version: 1,
      storeId: "00000000-0000-4000-8000-000000000001",
      receiptLimit: 256,
    } as SettingsWriteProtocol,
  });
  const [visible, setVisible] = useState(true);
  const [available, setAvailable] = useState(true);
  window.setSettingsAvailable = setAvailable;
  const [refreshes, setRefreshes] = useState(0);
  window.publishSettings = (
    value,
    revision,
    storeId = "00000000-0000-4000-8000-000000000001",
  ) =>
    setState({
      value,
      revision,
      protocol: { version: 1, storeId, receiptLimit: 256 },
    });
  return (
    <IconProvider>
      <main className="p-6">
        <button onClick={() => setVisible((v) => !v)}>切换挂载</button>
        <output data-testid="refreshes">{refreshes}</output>
        {visible && (
          <SettingsPreferences
            value={available ? state.value : undefined}
            revision={
              new URLSearchParams(location.search).has("legacy")
                ? undefined
                : state.revision
            }
            protocol={
              location.search.includes("legacy") ? undefined : state.protocol
            }
            refresh={async () => {
              setRefreshes((n) => n + 1);
              throw new Error("snapshot unavailable");
            }}
          />
        )}
      </main>
    </IconProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
