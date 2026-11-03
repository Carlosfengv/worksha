// Generated from contracts/domain/routing-policy-recovery/*.schema.json. Do not edit.
export type PolicyRecoveryAction = ({ "kind": "list"; "storeId": string; } | { "kind": "inspect"; "scope": PolicyRecoveryScope; } | { "kind": "resolve"; "mode": PolicyRecoveryMode; "reference": PolicyRecoveryReference; });
export type PolicyRecoveryEntry = { "pendingOperationIds": Array<string>; "problem"?: (PolicyRecoveryProblem | null); "scope": PolicyRecoveryScope; "serviceExists"?: (boolean | null); "serviceName"?: (string | null); "state": PolicyRecoveryState; };
export type PolicyRecoveryInspection = { "canRestoreNative": boolean; "canRestorePending": boolean; "currentServiceVersion"?: (string | null); "entry": PolicyRecoveryEntry; "reference"?: (PolicyRecoveryReference | null); };
export type PolicyRecoveryMode = ({ "kind": "native"; } | { "kind": "pending-original"; });
export type PolicyRecoveryProblem = "missing-control" | "marker-mismatch" | "pending-publication" | "incomplete-evidence" | "unavailable" | "capacity";
export type PolicyRecoveryReference = { "referenceId": string; "scope": PolicyRecoveryScope; };
export type PolicyRecoveryReply = ({ "entries": Array<PolicyRecoveryEntry>; "kind": "listed"; "limited": boolean; "storeId": string; } | { "inspection": PolicyRecoveryInspection; "kind": "inspected"; } | { "kind": "resolved"; "mode": PolicyRecoveryMode; "reference": PolicyRecoveryReference; "revision": string; });
export type PolicyRecoveryScope = { "serviceId": string; "storeId": string; };
export type PolicyRecoveryState = "unchecked" | "consistent" | "needs-recovery" | "blocked";
