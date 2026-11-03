export interface Settings {
  captureContent: boolean;
  retentionDays: 7 | 30 | 90;
}

export interface SettingsUpdate {
  storeId: string;
  value: Settings;
  expectedRevision: number;
  operationId: string;
}

export interface SettingsSaved {
  value: Settings;
  revision: number;
  operationId: string;
}

export interface ScopedSettingsSaved extends SettingsSaved {
  storeId: string;
}
export interface SettingsWriteProtocol {
  version: number;
  storeId: string;
  receiptLimit: number;
}

export interface GatewayClient {
  id: string;
  name: string;
  agent: string;
  providerId: string;
  models: string[];
  concurrency: number;
  active: number;
  enabled: boolean;
  createdAt: string;
}
export interface GatewayStatus {
  listen: string | null;
  persistent: boolean;
  concurrency: number | null;
  active: number | null;
  clients: GatewayClient[];
  writeProtocol?: {
    version: 1;
    storeId: string;
    maxClients: number;
    secretReplay: false;
  };
}
export interface GatewayClientDraft {
  name: string;
  agent: "claude" | "codex" | "opencode";
  providerId: string;
  models: string[];
  concurrency: number;
}

export type GatewayClientFileResult =
  | { status: "cancelled" }
  | {
      status: "saved";
      id: string;
      fileName: string;
      sourceInstanceId?: string;
      sourceCurrent?: boolean;
    };

export type GatewayOperationRequest = {
  storeId: string;
  operationId: string;
  action:
    | { kind: "create"; draft: GatewayClientDraft }
    | { kind: "revoke"; clientId: string };
};
export type GatewayOperationOutcome = {
  request: GatewayOperationRequest;
  receipt: {
    apiVersion: 1;
    storeId: string;
    operationId: string;
    action: "create" | "revoke";
    clientId: string;
    listen: string;
    completedAt: string;
  };
};
export type PendingGatewayOperation = {
  request: GatewayOperationRequest;
  compensationOperationId?: string;
};
export type GatewayOperationFileResult =
  | { status: "cancelled" }
  | { status: "notIssued"; reason: string }
  | ({
      sourceInstanceId: string;
      sourceCurrent: boolean;
      outcome: GatewayOperationOutcome;
    } & (
      | { status: "saved"; fileName: string }
      | { status: "issued" }
      | {
          status: "revoked";
          compensation: GatewayRevocation;
          reason: string;
        }
    ));
export type GatewayRevocation =
  | { status: "committed"; outcome: GatewayOperationOutcome }
  | { status: "alreadyRevoked"; request: GatewayOperationRequest };
