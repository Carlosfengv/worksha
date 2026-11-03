// Generated from contracts/domain/provider-draft-catalog/*.schema.json. Do not edit.
export type DraftCatalogAction = ({ "kind": "check"; "request": DraftCatalogRequest; } | { "kind": "read"; "request": DraftCatalogRequest; });
export type DraftCatalogRequest = { "draft": DraftEndpoint; "origin": DraftOrigin; "source": SourceChoice; };
export type DraftEndpoint = { "credentialRef"?: (string | null); "endpoint": string; "protocol": string; "vendor": string; };
export type DraftOrigin = ({ "kind": "saved"; "query": SourceQuery; "sourceRevision"?: (string | null); } | { "kind": "new"; "providerId": string; "storeId": string; });
export type Selection = { "bindingId": string; "digest": string; "pluginId": string; "target": Target; };
export type SourceChoice = ({ "kind": "builtin"; } | { "catalog": Selection; "kind": "external"; });
export type SourceQuery = { "expectedRevision": string; "providerId": string; "storeId": string; };
export type Target = ({ "expectedRevision": string; "kind": "saved"; "providerId": string; } | { "credentialRef"?: (string | null); "endpoint": string; "kind": "draft"; "label": string; "protocol": string; "vendor": string; });
