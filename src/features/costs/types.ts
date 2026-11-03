import type { AssociatedCosts, CostTotals } from "@/shared/types/cost";
import type { ProviderCost } from "@/features/providers/types";
import type { Usage } from "@/shared/types/usage";
export interface CostQuery {
  scope: "session" | "project";
  id: string;
  agent?: string;
  days?: number;
  provider?: string;
  model?: string;
  cursor?: string;
}
export interface CostCall {
  id: string;
  requestId: string;
  sessionId: string | null;
  providerKey: string;
  provider: string;
  model: string | null;
  time: string;
  status: string;
  usage: Usage;
  cost: ProviderCost;
  association: "response_id" | "session_id";
}
export interface CostGroup {
  providerKey: string;
  provider: string;
  model: string | null;
  attempts: number;
  cost: CostTotals;
}
export interface CostPage {
  summary: AssociatedCosts;
  filteredCost: CostTotals;
  total: number;
  providers: CostGroup[];
  models: CostGroup[];
  filteredProviders: CostGroup[];
  filteredModels: CostGroup[];
  calls: CostCall[];
  nextCursor: string | null;
}
