// Generated from contracts/domain/provider-draft-grant/*.schema.json. Do not edit.
export type DraftEndpoint = { "credentialRef"?: (string | null); "endpoint": string; "protocol": string; "vendor": string; };
export type DraftGrantAction = ({ "kind": "open"; "scope": DraftGrantScope; "storeId": string; } | { "kind": "acquire"; "owner": DraftGrantOwner; } | { "kind": "attach"; "owner": DraftGrantOwner; "request": SourceRequest; } | { "kind": "release"; "owner": DraftGrantOwner; });
export type DraftGrantOwner = { "ownerId": string; "storeId": string; };
export type DraftGrantReceipt = { "owner": DraftGrantOwner; "selection"?: (Selection | null); "state": DraftGrantState; };
export type DraftGrantScope = { "allowPrivateNetwork": boolean; "catalog": Query; "draft": DraftEndpoint; "label": string; };
export type DraftGrantState = "prepared" | "acquired" | "borrowed" | "pending" | "released";
export type Kind = "save" | "connect";
export type ModelCatalogSource = { "credentialRef"?: (string | null); "endpoint": string; "fetchedAt": string; "protocol": string; "sourceUrl": string; };
export type Provider = { "category"?: string; "configured": boolean; "credentialLabel"?: (string | null); "credentialRef"?: (string | null); "disabledModels"?: Array<string>; "endpoint": string; "id": string; "model": string; "modelCatalog"?: (ModelCatalogSource | null); "models"?: Array<string>; "name": string; "protocol": string; "revision"?: number; "vendor"?: string; };
export type ProviderOperationRequest = { "expectedSequence": number; "kind": Kind; "operationId": string; "provider": Provider; "storeId": string; };
export type Query = { "digest": string; "pluginId": string; };
export type Selection = { "bindingId": string; "digest": string; "pluginId": string; "target": Target; };
export type SourceChoice = ({ "kind": "builtin"; } | { "catalog": Selection; "kind": "external"; });
export type SourceRequest = { "expectedSourceRevision"?: (string | null); "operation": ProviderOperationRequest; "source": SourceChoice; };
export type Target = ({ "expectedRevision": string; "kind": "saved"; "providerId": string; } | { "credentialRef"?: (string | null); "endpoint": string; "kind": "draft"; "label": string; "protocol": string; "vendor": string; });
