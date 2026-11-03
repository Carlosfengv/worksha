// Generated from contracts/plugins/contributions/*.schema.json. Do not edit.
export type ContributionCatalog = { "items": Array<ContributionStatus>; "schemaVersion": number; };
export type Contributions = { "detailSlots"?: Array<DetailSlot>; "optionalDependencies"?: Array<OptionalDependency>; "schemaVersion": number; };
export type ContributionStatus = { "dependencies": Array<DependencyStatus>; "digest": string; "errorCode"?: (string | null); "pluginId": string; "slots": Array<DetailSlot>; };
export type DependencyStatus = { "available": boolean; "pluginId": string; "reason"?: (string | null); "selectedVersion"?: (string | null); "version": string; "views": Array<string>; };
export type DetailPlacement = "tab" | "card";
export type DetailSlot = { "id": string; "objectIds": Array<string>; "placement": DetailPlacement; "target": DetailTarget; "title": string; "viewId": string; };
export type DetailTarget = "agent" | "provider" | "account";
export type OptionalDependency = { "pluginId": string; "version": string; "views": Array<string>; };
