// Generated from contracts/domain/routing-policy/*.schema.json. Do not edit.
export type Candidate = { "capabilities": CandidateCapabilities; "health": CandidateHealth; "modelId": string; };
export type CandidateCapabilities = { "contextWindow": number; "inputModalities": Array<string>; "maxOutputTokens": number; "thinking": boolean; "tools": boolean; };
export type CandidateHealth = "unknown" | "available" | "cooling";
export type RankReason = "builtin-priority" | "extension-order";
export type RankRequest = { "candidates": Array<Candidate>; "configuredOrder": Array<string>; "defaultModelId"?: (string | null); "preferredModelId"?: (string | null); "serviceId": string; "serviceVersionId": string; };
export type RankResult = { "order": Array<string>; "reason": RankReason; "requestIdentity": string; };
