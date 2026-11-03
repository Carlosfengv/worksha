import type {
  SettingsUpdate,
  GatewayClientDraft,
  GatewayOperationRequest,
  PendingGatewayOperation,
} from "@/features/settings/types";
import { command, DesktopError } from "@/shared/lib/desktop-command";
import {
  settingsReceipt,
  scopedSettingsReceipt,
  freezeSettingsRequest,
} from "./save-result";
import { backendDiagnostics } from "./backend-result";
import { gatewayFileResult } from "./gateway-result";
import { gatewayStatus } from "./gateway-status";
import {
  freezeGatewayRequest,
  freezeGatewayPending,
  gatewayOutcome,
  gatewayOperationFileResult,
  gatewayRevocation,
} from "./gateway-operation";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
import {
  freezeRetentionRequest,
  retentionPreview,
  type RetentionRequest,
} from "./retention-result";
import { retentionInventory } from "./retention-inventory";
import { retentionContentInventory } from "./retention-content-inventory";
const readRetentionInventory = (
  request: RetentionRequest & { inventoryVersion?: 2 },
) => command("preview_retention_inventory", { request });
export class SettingsPreflightError extends DesktopError {
  readonly phase = "preflight";
  constructor(reason: unknown) {
    super({
      code:
        reason instanceof DesktopError
          ? reason.code
          : "settings_preflight_failed",
      outcomeUnknown: false,
      message: `本次未发送新的设置保存请求。${reason instanceof Error ? reason.message : String(reason)}`,
    });
  }
}
export const settingsApi = {
  previewRetentionContentInventory: async (input: RetentionRequest) => {
    const request = freezeRetentionRequest(input);
    return retentionContentInventory(
      await readRetentionInventory({ ...request, inventoryVersion: 2 }),
      request,
    );
  },
  previewRetentionInventory: async (input: RetentionRequest) => {
    const request = freezeRetentionRequest(input);
    return retentionInventory(await readRetentionInventory(request), request);
  },
  previewRetention: async (input: RetentionRequest) => {
    const request = freezeRetentionRequest(input);
    return retentionPreview(
      await command("preview_retention", { request }),
      request,
    );
  },
  backendDiagnostics: async () =>
    backendDiagnostics(await command("get_backend_diagnostics")),
  reconnectBackend: async () => {
    const result = backendDiagnostics(await command("reconnect_backend"));
    if (result.state !== "connected")
      throw new Error("后台连接尚未确认，请检查连接状态。");
    notifyBackendRecovery();
    return result;
  },
  gatewayStatus: async () => gatewayStatus(await command("get_gateway_status")),
  createGatewayClient: async (input: GatewayClientDraft) =>
    gatewayFileResult(
      await command("create_gateway_client", {
        input,
      }),
    ),
  revokeGatewayClient: (id: string) => command("revoke_gateway_client", { id }),
  gatewayClientOperation: async (request: GatewayOperationRequest) => {
    request = freezeGatewayRequest(request);
    return gatewayOutcome(
      await command("get_gateway_client_operation", {
        operationId: request.operationId,
        storeId: request.storeId,
      }),
      request,
    );
  },
  commitGatewayClientOperation: async (request: GatewayOperationRequest) => {
    request = freezeGatewayRequest(request);
    if (request.action.kind !== "revoke")
      throw Error("此入口仅用于撤销原客户端。");
    return gatewayRevocation(
      await command("commit_gateway_client_operation", { request }),
      request,
    );
  },
  saveGatewayClientOperation: async (input: PendingGatewayOperation) => {
    input = freezeGatewayPending(input);
    if (input.request.action.kind !== "create")
      throw Error("此入口仅用于保存原客户端连接文件。");
    return gatewayOperationFileResult(
      await command("save_gateway_client_operation", { input }),
      input,
    );
  },
  saveSettings: async (settings: SettingsUpdate) => {
    settings = freezeSettingsRequest(settings);
    try {
      await settingsApi.scopedSettingsOperation(settings);
    } catch (e) {
      if (!(
        e instanceof DesktopError &&
        e.code === "operation_not_found" &&
        !e.outcomeUnknown
      ))
        throw new SettingsPreflightError(e);
    }
    return scopedSettingsReceipt(
      await command("save_settings", { settings }),
      settings,
    );
  },
  scopedSettingsOperation: async (settings: SettingsUpdate) => {
    settings = freezeSettingsRequest(settings);
    return scopedSettingsReceipt(
      await command("get_scoped_settings_operation", {
        operationId: settings.operationId,
        storeId: settings.storeId,
      }),
      settings,
    );
  },
  settingsOperation: async (operationId: string) =>
    settingsReceipt(
      await command("get_settings_operation", { operationId }),
      operationId,
    ),
};
