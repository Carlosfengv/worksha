import { createReadResourceFamily } from "@/shared/lib/read-resource";
import { projectsApi } from "./api";

export const projectDiffResource = createReadResourceFamily(
  ({ projectId, fileId }: { projectId: string; fileId: string }) =>
    projectsApi.projectDiff(projectId, fileId),
  { interval: false },
);
