import { command } from "@/shared/lib/desktop-command";
import type { CostPage, CostQuery } from "./types";
export const costsApi = {
  details(query: CostQuery): Promise<CostPage> {
    return command("query_associated_costs", { query });
  },
};
