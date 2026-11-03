// Generated from contracts/plugins/extensions/*.schema.json. Do not edit.
export type Action = ({ "allowPrivateNetwork": boolean; "kind": "bind-protocol"; "providerId": string; } | { "kind": "unbind-protocol"; } | { "kind": "prepare-config"; "providerId": string; } | { "kind": "apply-config"; "planId": string; "revision": string; } | { "kind": "restore-config"; "operationId": string; } | { "kind": "discard-config"; "planId": string; } | { "kind": "start-connector"; } | { "kind": "stop-connector"; } | { "kind": "refresh-connector"; });
export type BindingValue = "endpoint" | "model" | "credential";
export type Config = { "fields": Array<ConfigField>; };
export type ConfigCandidate = { "edits": Array<JsonEdit>; };
export type ConfigField = { "pointer": string; "value": BindingValue; };
export type Connection = ({ "args": Array<string>; "command": string; "transport": "stdio"; } | { "allowPrivateNetwork": boolean; "providerId"?: (string | null); "transport": "http"; "url": string; } | { "launchToken": string; "transport": "dsh"; "url": string; "version": string; });
export type Connector = "mcp-stdio" | "mcp-http" | "dsh-sessions";
export type JsonEdit = { "pointer": string; "value": unknown; };
export type Policy = { "config"?: (Config | null); "connector"?: (Connector | null); "protocol"?: (Protocol | null); "schemaVersion": number; };
export type Protocol = { "baseProtocol": string; "path": string; };
export type ProtocolOpen = { "request": unknown; "stream": boolean; };
export type Query = { "digest": string; "pluginId": string; };
export type Request = { "action": Action; "digest": string; "pluginId": string; };
export type WireChunk = { "bytes": Array<number>; };
export type WireOutput = { "bytes": Array<number>; "complete": boolean; };
