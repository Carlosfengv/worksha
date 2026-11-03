import type {
  CallKind,
  Conversation,
  NativeCall,
  NativePage,
  NativeSession,
  SessionQuery,
} from "@/features/sessions/types";
import { command } from "@/shared/lib/desktop-command";
import { invalidateCapabilityAccess } from "@/shared/lib/capability-access";
export const sessionsApi = {
  sessions(query: SessionQuery): Promise<NativePage<NativeSession>> {
    return command("query_sessions", { query });
  },
  calls(query: {
    agent?: string;
    kind?: CallKind;
    project?: string;
    days?: number;
    cursor?: string;
  }): Promise<NativePage<NativeCall>> {
    return command("query_calls", { query });
  },
  conversation(
    sessionId: string,
    cursor?: string,
    anchor?: string,
  ): Promise<Conversation> {
    return command("get_conversation", {
      sessionId,
      cursor: cursor ?? null,
      anchor: anchor ?? null,
    });
  },
  async setSessionSource(agentId: string, enabled: boolean): Promise<void> {
    invalidateCapabilityAccess();
    try {
      return await command("set_session_source", { agentId, enabled });
    } finally {
      invalidateCapabilityAccess();
    }
  },
  refreshSessions(): Promise<void> {
    return command("refresh_sessions");
  },
};
