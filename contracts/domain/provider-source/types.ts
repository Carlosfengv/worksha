// Generated from contracts/domain/provider-source/*.schema.json. Do not edit.
export type Choice = { "digest": string; "pluginId": string; };
export type Policy = { "baseProtocol": string; "path": string; "schemaVersion": number; };
export type Selection = { "bindingId": string; "digest": string; "pluginId": string; "target": Target; };
export type SourceAction = ({ "kind": "state"; "query": SourceQuery; } | { "kind": "read"; "query": SourceQuery; "sourceRevision"?: (string | null); } | { "catalog"?: (Selection | null); "kind": "select"; "query": SourceQuery; "sourceRevision"?: (string | null); });
export type SourceKind = "builtin" | "external" | "pending" | "changed";
export type SourceQuery = { "expectedRevision": string; "providerId": string; "storeId": string; };
export type SourceState = { "authorized": boolean; "choice"?: (Choice | null); "errorCode"?: (string | null); "expectedRevision": string; "kind": SourceKind; "policy"?: (Policy | null); "providerId": string; "selection"?: (Selection | null); "sourceRevision"?: (string | null); };
export type Target = ({ "expectedRevision": string; "kind": "saved"; "providerId": string; } | { "credentialRef"?: (string | null); "endpoint": string; "kind": "draft"; "label": string; "protocol": string; "vendor": string; });
