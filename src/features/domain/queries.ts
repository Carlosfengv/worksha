import {
  createReadResource,
  createReadResourceFamily,
} from "@/shared/lib/read-resource";
import { isDesktopRuntime } from "@/shared/lib/desktop-command";
import { domainApi } from "./api";
import type { FactQuery } from "../../../contracts/domain/facts/types";
export const unifiedFactsResource = createReadResourceFamily(
  (query: FactQuery) => domainApi.facts(query),
  { interval: false },
);
export const unifiedActivityResource = createReadResourceFamily(
  (
    query: import("../../../contracts/domain/facts/types").UnifiedActivityQuery,
  ) => domainApi.activity(query),
  { interval: 5000 },
);
export const overviewActivityResource = createReadResourceFamily(
  domainApi.activity,
  { interval: 1500, pollWhileHidden: isDesktopRuntime },
);
export const unifiedRequestsResource = createReadResourceFamily(
  (
    query: import("../../../contracts/domain/facts/types").UnifiedRequestQuery,
  ) => domainApi.requests(query),
  { interval: false },
);
import type {
  UnifiedSessionQuery,
  UnifiedConversationRequest,
  SourceConsentQuery,
} from "./types";
export const domainDirectoryResource = createReadResource(() =>
  domainApi.catalog(),
);
export const unifiedSessionsResource = createReadResourceFamily(
  (query: UnifiedSessionQuery) => domainApi.sessions(query),
  { interval: false },
);
export const unifiedConversationResource = createReadResourceFamily(
  (query: UnifiedConversationRequest) => domainApi.conversation(query),
  { interval: false },
);
export const factConversationResource = createReadResourceFamily(
  (query: UnifiedConversationRequest) => domainApi.factConversation(query),
  { interval: false },
);
export const sourceConsentReviewResource = createReadResourceFamily(
  (query: SourceConsentQuery) => domainApi.prepareConsent(query),
  { interval: false },
);
