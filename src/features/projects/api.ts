import type {
  GitDiff,
  ProjectDetail,
  ProjectPage,
  ProjectQuery,
  HiddenProject,
} from "@/features/projects/types";
import { command } from "@/shared/lib/desktop-command";
import { invalidateCapabilityAccess } from "@/shared/lib/capability-access";
export const projectsApi = {
  async setHidden(
    projectId: string,
    hidden: boolean,
  ): Promise<{ hiddenProjects: HiddenProject[] }> {
    invalidateCapabilityAccess();
    try {
      return await command("set_project_hidden", { projectId, hidden });
    } finally {
      invalidateCapabilityAccess();
    }
  },
  selectProjectDirectory(projectId: string): Promise<boolean> {
    return command("select_project_directory", { projectId });
  },
  projects(query: ProjectQuery): Promise<ProjectPage> {
    return command("query_projects", { query });
  },
  project(projectId: string, query: ProjectQuery): Promise<ProjectDetail> {
    return command("get_project", { projectId, query });
  },
  projectDiff(projectId: string, fileId: string): Promise<GitDiff> {
    return command("get_project_diff", { projectId, fileId });
  },
};
