// Generated from contracts/domain/config/*.schema.json. Do not edit.
export type ConfigIntent = ({ "agentId": string; "binding"?: (NativeBinding | null); "kind": "native-route"; } | { "edits": Array<FieldEdit>; "kind": "json-fields"; });
export type ConfigRequest = { "intent": ConfigIntent; "targetRevision": string; };
export type FieldEdit = { "pointer": string; "value": string; };
export type FieldsCandidate = { "edits": Array<FieldEdit>; };
export type NativeBinding = { "endpoint": string; "identity": string; "model": string; };
