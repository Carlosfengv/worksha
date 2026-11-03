// Generated from contracts/domain/provider-draft-recovery/*.schema.json. Do not edit.
export type DraftGrantOwner = { "ownerId": string; "storeId": string; };
export type DraftRecoveryAction = ({ "kind": "list"; "storeId": string; } | { "kind": "inspect"; "owner": DraftGrantOwner; } | { "kind": "finish"; "owner": DraftGrantOwner; });
export type DraftRecoveryEntry = { "canFinish": boolean; "digest"?: (string | null); "endpointOrigin"?: (string | null); "label": string; "operationId"?: (string | null); "owner": DraftGrantOwner; "ownership": DraftRecoveryOwnership; "pluginId"?: (string | null); "state": DraftRecoveryState; };
export type DraftRecoveryInspection = { "entry": DraftRecoveryEntry; "request"?: (ProviderOperationRequest | null); };
export type DraftRecoveryOwnership = "prepared" | "managed" | "shared" | "missing";
export type DraftRecoveryReply = ({ "entries": Array<DraftRecoveryEntry>; "kind": "listed"; "storeId": string; } | { "inspection": DraftRecoveryInspection; "kind": "inspected"; } | { "kind": "finished"; "owner": DraftGrantOwner; "released": boolean; });
export type DraftRecoveryState = "unchecked" | "unattached" | "pending" | "committed" | "failed" | "cancelled" | "not-found" | "unavailable";
export type Kind = "save" | "connect";
export type ModelCatalogSource = { "credentialRef"?: (string | null); "endpoint": string; "fetchedAt": string; "protocol": string; "sourceUrl": string; };
export type Provider = { "category"?: string; "configured": boolean; "credentialLabel"?: (string | null); "credentialRef"?: (string | null); "disabledModels"?: Array<string>; "endpoint": string; "id": string; "model": string; "modelCatalog"?: (ModelCatalogSource | null); "models"?: Array<string>; "name": string; "protocol": string; "revision"?: number; "vendor"?: string; };
export type ProviderOperationRequest = { "expectedSequence": number; "kind": Kind; "operationId": string; "provider": Provider; "storeId": string; };
