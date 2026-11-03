// Generated from contracts/domain/object-summary/*.schema.json. Do not edit.
export type Coverage = "complete" | "partial" | "unavailable";
export type Decimal = string;
export type Freshness = "unread" | "current" | "stale" | "failed";
export type MissingReason = "unread" | "not-recorded" | "unavailable" | "partial";
export type ObjectOrigin = ({ "kind": "builtin"; } | { "kind": "unified"; "scope": QueryScope; "snapshotToken": string; "sourceId": string; });
export type ObjectSelector = ({ "id": string; "target": "agent"; } | { "id": string; "target": "provider"; } | { "id": string; "target": "account"; } | { "id": string; "origin": ObjectOrigin; "target": "project"; } | { "id": string; "origin": ObjectOrigin; "target": "session"; } | { "id": string; "origin": ObjectOrigin; "target": "request"; "unit": RequestUnit; });
export type ObjectSummary = { "apiVersion": string; "capturedAt": string; "coverage": Coverage; "freshness": Freshness; "metrics": Array<SummaryMetric>; "target": ObjectTarget; };
export type ObjectTarget = "agent" | "provider" | "account" | "project" | "session" | "request";
export type QueryScope = { "kind": ScopeKind; "sourceIds": Array<string>; };
export type RequestUnit = "model-response" | "logical-request";
export type ScopeKind = "builtin" | "external" | "combined";
export type SummaryField = "session-count" | "model-count" | "configuration" | "quota-window-count" | "input-tokens" | "output-tokens" | "total-tokens" | "attempt-count" | "duration-ms" | "request-outcome";
export type SummaryMetric = { "field": SummaryField; "missingReason"?: (MissingReason | null); "value"?: (SummaryValue | null); };
export type SummaryOutcome = "succeeded" | "failed" | "cancelled" | "in-progress" | "unknown";
export type SummaryValue = ({ "kind": "count"; "value": Decimal; } | { "kind": "tokens"; "value": Decimal; } | { "configured": boolean; "kind": "configuration"; } | { "kind": "milliseconds"; "value": number; } | { "kind": "outcome"; "value": SummaryOutcome; });
