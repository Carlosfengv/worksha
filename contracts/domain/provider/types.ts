// Generated from contracts/domain/provider/*.schema.json. Do not edit.
export type CatalogContext = { "basePath": string; "officialCatalogRoot": boolean; "protocol": string; "vendor": string; };
export type CatalogContinuation = { "afterId"?: (string | null); "hasMore": boolean; };
export type CatalogModel = { "capabilities": ModelCapabilities; "contextWindow"?: (number | null); "id": string; "maxInputTokens"?: (number | null); "maxOutputTokens"?: (number | null); "name": string; };
export type CatalogRequest = { "path": string; };
export type ModelCapabilities = { "jsonOutput"?: (boolean | null); "reasoning"?: (boolean | null); "structuredOutput"?: (boolean | null); "tools"?: (boolean | null); "vision"?: (boolean | null); };
export type ModelMetadata = { "capabilities": ModelCapabilities; "contextWindow"?: (number | null); "maxInputTokens"?: (number | null); "maxOutputTokens"?: (number | null); };
