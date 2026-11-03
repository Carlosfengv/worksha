// Generated from contracts/domain/provider-presets/*.schema.json. Do not edit.
export type PresetAction = ({ "kind": "list"; "storeId": string; } | { "kind": "resolve"; "origin": PresetOrigin; "storeId": string; });
export type PresetAdvice = { "documents"?: (string | null); "endpoints": { [key: string]: string; }; "model": string; "name": string; "origin": PresetOrigin; "protocol": string; "sourceName": string; };
export type PresetIssue = { "digest"?: (string | null); "errorCode": string; "pluginId"?: (string | null); };
export type PresetOrigin = ({ "kind": "builtin"; "presetId": string; } | { "digest": string; "kind": "external"; "pluginId": string; "presetId": string; });
export type PresetReply = ({ "issues": Array<PresetIssue>; "items": Array<PresetAdvice>; "kind": "listed"; "storeId": string; } | { "advice": PresetAdvice; "kind": "resolved"; "storeId": string; });
