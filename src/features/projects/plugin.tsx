import type { UiPlugin } from "@/app/plugin-registry";
import type { ComponentType } from "react";
import type { ProjectsPageProps } from "./types";

export function createProjectsPlugin(
  ProjectsPage: ComponentType<ProjectsPageProps>,
): UiPlugin {
  return {
    id: "builtin.projects",
    version: 1,
    pages: [
      {
        id: "projects",
        label: "项目",
        icon: "folder",
        systemFeature: {
          purpose: "按工作目录查看已授权 Agent 的会话、活动和用量",
          scenes: ["projects"],
          operations: [
            "浏览项目与会话",
            "查看 Git 和磁盘信息",
            "隐藏或恢复项目",
          ],
          conditions:
            "需要启用 Sessions 并授权对应的会话来源。磁盘和 Git 读取遵循现有目录授权。",
          supportScenes: ["projects"],
        },
        render: ({ data, refresh, route }) => (
          <ProjectsPage
            data={data}
            refresh={refresh}
            initialScope={route.scope}
            initialAgent={route.agent}
            selectedId={route.objectId}
            selectedTab={route.tab}
            anchor={route.anchor}
          />
        ),
      },
    ],
  };
}
