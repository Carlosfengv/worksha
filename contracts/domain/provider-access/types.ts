// Generated from contracts/domain/provider-access/*.schema.json. Do not edit.
export type Action = ({ "allowPrivateNetwork": boolean; "kind": "bind"; "target": Target; } | { "bindingId": string; "kind": "unbind"; } | { "bindingId": string; "kind": "read"; "target": Target; });
export type Binding = { "allowPrivateNetwork": boolean; "authorized": boolean; "bindingId": string; "digest": string; "endpoint": string; "errorCode"?: (string | null); "expectedRevision"?: (string | null); "label": string; "protocol": string; "providerId"?: (string | null); };
export type CatalogContinuation = { "afterId"?: (string | null); "hasMore": boolean; };
export type CatalogModel = { "capabilities": ModelCapabilities; "contextWindow"?: (number | null); "id": string; "maxInputTokens"?: (number | null); "maxOutputTokens"?: (number | null); "name": string; };
export type ModelCapabilities = { "jsonOutput"?: (boolean | null); "reasoning"?: (boolean | null); "structuredOutput"?: (boolean | null); "tools"?: (boolean | null); "vision"?: (boolean | null); };
export type Page = { "continuation": CatalogContinuation; "models": Array<CatalogModel>; "snapshot": string; };
export type ParseRequest = { "body": unknown; "snapshot": string; };
export type Policy = { "baseProtocol": string; "path": string; "schemaVersion": number; };
export type Query = { "digest": string; "pluginId": string; };
export type Request = { "action": Action; "digest": string; "pluginId": string; };
export type State = { "bindings": Array<Binding>; "canBind": boolean; "errorCode"?: (string | null); "policy"?: (Policy | null); };
export type Target = ({ "expectedRevision": string; "kind": "saved"; "providerId": string; } | { "credentialRef"?: (string | null); "endpoint": string; "kind": "draft"; "label": string; "protocol": string; "vendor": string; });
