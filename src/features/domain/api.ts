import { command } from "@/shared/lib/desktop-command";
import { invalidateCapabilityAccess } from "@/shared/lib/capability-access";
import catalogSchema from "../../../contracts/domain/unified-catalog.schema.json";
import pageSchema from "../../../contracts/domain/unified-session-page.schema.json";
import conversationSchema from "../../../contracts/domain/unified-conversation.schema.json";
import consentReviewSchema from "../../../contracts/domain/source-consent-review.schema.json";
import capabilitiesSchema from "../../../contracts/domain/facts/capability-catalog.schema.json";
import factPageSchema from "../../../contracts/domain/facts/fact-page.schema.json";
import activityPageSchema from "../../../contracts/domain/facts/unified-activity-page.schema.json";
import requestPageSchema from "../../../contracts/domain/facts/unified-request-page.schema.json";
import requestExportSchema from "../../../contracts/domain/request-export/save-reply.schema.json";
import type {
  FactQuery,
  FactPage,
} from "../../../contracts/domain/facts/types";
import type { CapabilityCatalog } from "../../../contracts/domain/facts/types";
import { parseDomain } from "./contract";
import type {
  SourceConsent,
  SourceConsentQuery,
  SourceConsentReview,
  UnifiedCatalog,
  UnifiedConversation,
  UnifiedConversationRequest,
  UnifiedSessionPage,
  UnifiedSessionQuery,
} from "./types";

export const domainApi = {
  async exportRequests(snapshotToken: string) {
    const reply = parseDomain<
      import("../../../contracts/domain/request-export/types").SaveReply
    >(
      requestExportSchema,
      await command<unknown>("export_unified_request_csv", { snapshotToken }),
    );
    if (
      reply.status === "saved" &&
      (reply.info.snapshotToken !== snapshotToken ||
        reply.info.apiVersion !== "1.0.0")
    )
      throw new Error("导出确认与原冻结范围不一致，请核查目标文件。");
    return reply;
  },
  async requests(
    query: import("../../../contracts/domain/facts/types").UnifiedRequestQuery,
  ) {
    return parseDomain<
      import("../../../contracts/domain/facts/types").UnifiedRequestPage
    >(
      requestPageSchema,
      await command<unknown>("query_unified_requests", { query }),
    );
  },
  async activity(
    query: import("../../../contracts/domain/facts/types").UnifiedActivityQuery,
  ) {
    return parseDomain<
      import("../../../contracts/domain/facts/types").UnifiedActivityPage
    >(
      activityPageSchema,
      await command<unknown>("query_unified_activity", { query }),
    );
  },
  async factConversation(query: UnifiedConversationRequest) {
    return parseDomain<UnifiedConversation>(
      conversationSchema,
      await command<unknown>("get_fact_conversation", { query }),
    );
  },
  async projectVisibility(
    input: import("../../../contracts/domain/facts/types").ProjectVisibility,
  ) {
    invalidateCapabilityAccess();
    try {
      return await command<unknown>("set_domain_project_visibility", { input });
    } finally {
      invalidateCapabilityAccess();
    }
  },
  async facts(query: FactQuery) {
    return parseDomain<FactPage>(
      factPageSchema,
      await command<unknown>("query_unified_facts", { query }),
    );
  },
  async authorizeSource(query: SourceConsentQuery) {
    invalidateCapabilityAccess();
    try {
      return await command<unknown>("authorize_domain_source", { query });
    } finally {
      invalidateCapabilityAccess();
    }
  },
  async refreshSource(query: SourceConsentQuery) {
    return command<unknown>("refresh_domain_source", { query });
  },
  async capabilities() {
    return parseDomain<CapabilityCatalog>(
      capabilitiesSchema,
      await command<unknown>("query_domain_capabilities"),
    );
  },
  async catalog() {
    return parseDomain<UnifiedCatalog>(
      catalogSchema,
      await command<unknown>("query_domain_catalog"),
    );
  },
  async sessions(query: UnifiedSessionQuery) {
    return parseDomain<UnifiedSessionPage>(
      pageSchema,
      await command<unknown>("query_unified_sessions", { query }),
    );
  },
  async conversation(query: UnifiedConversationRequest) {
    return parseDomain<UnifiedConversation>(
      conversationSchema,
      await command<unknown>("get_unified_conversation", { query }),
    );
  },
  async consent(input: SourceConsent) {
    invalidateCapabilityAccess();
    try {
      return await command<unknown>("set_domain_source_consent", { input });
    } finally {
      invalidateCapabilityAccess();
    }
  },
  async prepareConsent(query: SourceConsentQuery) {
    return parseDomain<SourceConsentReview>(
      consentReviewSchema,
      await command<unknown>("prepare_domain_source_consent", { query }),
    );
  },
};
