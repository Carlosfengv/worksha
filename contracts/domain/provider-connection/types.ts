// Generated from contracts/domain/provider-connection/*.schema.json. Do not edit.
export type Action = ({ "allowPrivateNetwork": boolean; "kind": "bind"; "target": Target; } | { "bindingId": string; "kind": "unbind"; } | { "bindingId": string; "kind": "read"; "target": Target; });
export type ConnectionRequest = { "catalog": Selection; "operation": ProviderOperationRequest; };
export type Kind = "save" | "connect";
export type ModelCatalogSource = { "credentialRef"?: (string | null); "endpoint": string; "fetchedAt": string; "protocol": string; "sourceUrl": string; };
export type Provider = { "category"?: string; "configured": boolean; "credentialLabel"?: (string | null); "credentialRef"?: (string | null); "disabledModels"?: Array<string>; "endpoint": string; "id": string; "model": string; "modelCatalog"?: (ModelCatalogSource | null); "models"?: Array<string>; "name": string; "protocol": string; "revision"?: number; "vendor"?: string; };
export type ProviderOperationRequest = { "expectedSequence": number; "kind": Kind; "operationId": string; "provider": Provider; "storeId": string; };
export type Request = { "action": Action; "digest": string; "pluginId": string; };
export type ScopedRequest = { "request": Request; "storeId": string; };
export type Selection = { "bindingId": string; "digest": string; "pluginId": string; "target": Target; };
export type Target = ({ "expectedRevision": string; "kind": "saved"; "providerId": string; } | { "credentialRef"?: (string | null); "endpoint": string; "kind": "draft"; "label": string; "protocol": string; "vendor": string; });
