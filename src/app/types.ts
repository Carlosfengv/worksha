import type { ObservationBinding } from "@/features/observation/types";
import type { Provider } from "@/features/providers/types";
import type { SessionSource } from "@/features/sessions/types";
import type {
  Settings,
  SettingsWriteProtocol,
} from "@/features/settings/types";
import type { HiddenProject } from "@/features/projects/types";
import type { AgentConnections, AgentDiscovery } from "@/shared/types/agent";

export interface Snapshot {
  hiddenProjects?: HiddenProject[];
  agentConnections?: AgentConnections;
  version: string;
  databasePath: string;
  gatewayStatus: "not_configured" | "ready" | "paused";
  builtinComposition?: {
    profile: "desktop" | "background" | "recovery";
    problem: string | null;
    nativeActivityActive?: boolean;
  };
  agents: AgentDiscovery[];
  agentDirectoryError?: string | null;
  providers: Provider[];
  providerWriteProtocol?: import("@/features/providers/operation-receipt").ProviderWriteProtocol;
  settings: Settings;
  // Older services omit this field; settings writes must remain unavailable.
  settingsRevision?: number;
  settingsWriteProtocol?: SettingsWriteProtocol;
  sessionSources: SessionSource[];
  observationBindings: ObservationBinding[];
}

export type Page =
  | "overview"
  | "agents"
  | "projects"
  | "tasks"
  | "providers"
  | "model-services"
  | "mcp"
  | "sessions"
  | "requests"
  | "iam"
  | "plugins"
  | "settings";
