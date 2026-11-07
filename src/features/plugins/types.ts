export type {
  ActionRequest,
  InstallRequest,
  Manifest,
  PackageReview,
  PublicAction,
  StateAction,
  StateRequest,
  Status as PluginStatus,
  View,
  ViewQuery,
  SessionCandidate,
  Quota,
  Conversation,
  GatewayReport,
} from "../../../contracts/plugins/types";
import type {
  Status,
  View,
  SessionCandidate,
  Quota,
  GatewayReport,
} from "../../../contracts/plugins/types";
export interface PluginList {
  contributions?:
    | import("../../../contracts/plugins/contributions/types").ContributionCatalog
    | null;
  contributionsError?: string;
  diagnostics?: import("./diagnostics").PluginDiagnostics | null;
  diagnosticsError?: string;
  items: Status[];
  developmentMode: boolean;
  apiVersion: string;
  runtime: string;
}
export type PluginUploadReview =
  | {
      kind: "plugin";
      review: import("../../../contracts/plugins/types").PackageReview;
    }
  | {
      kind: "source";
      review: import("../../../contracts/plugins/sources/types").Review;
    }
  | {
      kind: "policy";
      review: import("../../../contracts/plugins/routing/types").Review;
    };
export interface PluginView {
  report?: GatewayReport | null;
  reportExpired?: boolean;
  view: View;
  status: Status;
  snapshot?: string | null;
  items?: { sessionId: string; record: SessionCandidate }[];
  quota?: Quota | null;
  quotaExpired?: boolean;
  total?: number;
  nextCursor?: string | null;
  confirmedEmpty: boolean;
  needsRefresh?: boolean;
  summary?: {
    knownUsageRecords: string;
    inputTokens: string;
    outputTokens: string;
    totalTokens: string;
  };
}
