// Generated from contracts/plugins/contributions/*.schema.json. Do not edit.
export type ContributionCatalog = { "items": Array<ContributionStatus>; "schemaVersion": number; };
export type Contributions = { "dataQueries"?: Array<DataQuery>; "detailSlots"?: Array<DetailSlot>; "navigation"?: Array<Navigation>; "optionalDependencies"?: Array<OptionalDependency>; "schemaVersion": number; };
export type ContributionStatus = { "dataQueries"?: Array<DataQuery>; "dependencies": Array<DependencyStatus>; "digest": string; "errorCode"?: (string | null); "navigation"?: Array<Navigation>; "pluginId": string; "slots": Array<DetailSlot>; };
export type DataQuery = { "capability": string; "sourceId": string; "viewId": string; };
export type DependencyStatus = { "available": boolean; "pluginId": string; "reason"?: (string | null); "selectedVersion"?: (string | null); "version": string; "views": Array<string>; };
export type DetailPlacement = "tab" | "card";
export type DetailSlot = { "id": string; "objectIds": Array<string>; "placement": DetailPlacement; "target": DetailTarget; "title": string; "viewId": string; };
export type DetailTarget = "agent" | "provider" | "account";
export type Navigation = { "id": string; "title": string; "viewId": string; };
export type OptionalDependency = { "pluginId": string; "version": string; "views": Array<string>; };
