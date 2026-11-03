import type { Snapshot } from "@/app/types";
import { command } from "@/shared/lib/desktop-command";
export const appApi = {
  async snapshot(): Promise<Snapshot> {
    return command("get_snapshot");
  },
};
