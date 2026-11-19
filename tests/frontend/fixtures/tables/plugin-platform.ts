import { appApi } from "@/app/api";
import { sessionsApi } from "@/features/sessions/api";
import { platformPluginsApi, pluginsApi } from "@/features/plugins/api";
import { invalidateBackendReads } from "@/shared/lib/backend-recovery";
import type {
  PlatformContribution,
  PlatformDefinition,
  PlatformRecord,
} from "../../../../contracts/plugins/platform/types";
import type { PluginStatus } from "@/features/plugins/types";
import { activityConversation, activitySessions } from "../../activity-fixture";

declare global {
  interface Window {
    platformReads: Record<string, number>;
    platformPatchCard: (patch: Record<string, unknown>) => void;
  }
}

export function installPlatformFixture() {
  const session = activitySessions()[0];
  const project = "fixture-project";
  const id = "org.example.platform-fixture";
  const digest = "d".repeat(64);
  let enabled = true;
  const epoch = "1";
  const grantRevision = "1.1";
  const definition: PlatformDefinition = {
    schemaVersion: 1,
    collections: {
      cards: {
        groupBy: "state",
        fields: {
          title: { kind: "text", label: "标题", required: true },
          state: {
            kind: "enum",
            label: "状态",
            required: true,
            options: ["待处理", "进行中", "完成"],
          },
          project: { kind: "ref:project", label: "项目" },
          session: { kind: "ref:session", label: "会话" },
        },
      },
    },
    views: {
      board: {
        title: "测试任务看板",
        kind: "kanban",
        collection: "cards",
        primaryField: "title",
        actions: ["records.create", "records.move", "records.update"],
      },
      "project-board": {
        title: "项目看板",
        kind: "kanban",
        collection: "cards",
        primaryField: "title",
        actions: ["records.move"],
      },
      "session-cards": {
        title: "会话关联卡片",
        kind: "detail-list",
        collection: "cards",
        primaryField: "title",
        actions: ["records.move"],
      },
      archive: { title: "测试归档", kind: "detail-list" },
      "archive-button": { title: "查看归档", kind: "action-bar" },
      overview: { title: "看板概况", kind: "stat-row", collection: "cards" },
    },
    slots: [
      {
        id: "board-nav",
        point: "nav.home-sidebar",
        viewId: "board",
        group: "observation",
      },
      {
        id: "archive-nav",
        point: "nav.home-sidebar",
        viewId: "archive",
        group: "supply",
      },
      {
        id: "request-toolbar",
        point: "requests.toolbar.actions",
        viewId: "archive-button",
      },
      {
        id: "session-toolbar",
        point: "sessions.toolbar.actions",
        viewId: "archive-button",
      },
      {
        id: "project-slot",
        point: "project.detail.tab",
        viewId: "project-board",
        filterByContext: "project",
      },
      {
        id: "session-slot",
        point: "session.inspector.card",
        viewId: "session-cards",
        filterByContext: "session",
      },
      { id: "overview-slot", point: "overview.section", viewId: "overview" },
    ],
  };
  const rows: PlatformRecord[] = [
    {
      recordId: "fixture-card",
      values: {
        title: "跨页卡片",
        state: "待处理",
        project,
        session: session.id,
      },
      _hydrated: {
        session: {
          authorization: "authorized",
          freshness: "current",
          activityState: "生成中",
          title: session.title,
          totalTokens: 42,
        },
      },
    },
    {
      recordId: "directory-card",
      values: {
        title: "目录来源卡片",
        state: "待处理",
        session: "org.example.reader:imported",
      },
      _hydrated: {
        session: {
          authorization: "authorized",
          freshness: "current",
          title: "目录会话",
          sourceId: "org.example.reader:source",
          model: "fixture-cursor-model",
        },
      },
    },
  ];
  window.platformReads = {};
  window.platformPatchCard = (patch) => {
    rows[0].values = { ...rows[0].values, ...patch };
  };
  const listing = pluginsApi.list;
  const status = (): PluginStatus => ({
    digest,
    enabled,
    runtimeStatus: enabled ? "active" : "disabled",
    errorCode: null,
    activationEpoch: epoch,
    grantRevision,
    sourceAuthorized: false,
    connectionAuthorized: false,
    reportAuthorized: false,
    signingKey: "f".repeat(64),
    settings: {},
    manifest: {
      id,
      manifestVersion: 1,
      version: "1.0.0",
      displayName: "测试插件",
      publisher: "org.example",
      hostApi: "^1.0.0",
      runtime: { kind: "declarative" },
      capabilities: ["ui.platform@1", "storage.plugin-data@1"],
    },
  });
  pluginsApi.list = async () => ({ ...(await listing()), items: [status()] });
  pluginsApi.change = async (request) => {
    if (
      request.pluginId !== id ||
      request.digest !== digest ||
      !["enable", "disable"].includes(request.action)
    )
      throw Error("未声明的插件变更");
    enabled = request.action === "enable";
    invalidateBackendReads();
    return status();
  };
  platformPluginsApi.list = async () => ({
    items: [
      {
        pluginId: id,
        digest,
        epoch,
        grantRevision,
        enabled,
        definition,
        entityGrants: ["session", "project"],
        directoryGrants: [],
        usedRows: rows.length,
        usedBytes: 256,
      } satisfies PlatformContribution,
    ],
  });
  platformPluginsApi.query = async (request) => {
    if (!enabled) throw Error("插件已停用");
    const { slotOrViewId, context } = request;
    const slot = definition.slots!.find((slot) => slot.id === slotOrViewId);
    const view = definition.views![slot?.viewId ?? slotOrViewId];
    if (!view) throw Error("未声明的视图");
    window.platformReads[slotOrViewId] =
      (window.platformReads[slotOrViewId] ?? 0) + 1;
    return {
      authorization: "authorized",
      freshness: "current",
      coverage: "complete",
      epoch,
      grantRevision,
      blocks: [view],
      records: !view.collection
        ? []
        : structuredClone(
            rows.filter(
              (row) =>
                !slot?.filterByContext ||
                row.values[slot.filterByContext] === context?.objectId,
            ),
          ),
    };
  };
  platformPluginsApi.invoke = async (request) => {
    if (
      !enabled ||
      request.expectedEpoch !== epoch ||
      request.expectedGrantRevision !== grantRevision
    )
      throw Error("声明已变化");
    const row = rows.find(({ recordId }) => recordId === request.recordId);
    if (
      row &&
      (request.actionId === "records.move" ||
        request.actionId === "records.update")
    ) {
      if (
        Object.entries(request.expectedValues ?? {}).some(
          ([key, value]) => (row.values[key] ?? null) !== value,
        )
      )
        throw Error("你修改的字段已在别处更新。请重新打开记录，核对后再保存。");
      row.values = { ...row.values, ...request.values };
    } else if (request.actionId === "records.create") {
      rows.push({
        recordId: "new-card",
        values: request.values ?? {},
        _hydrated: {},
      });
    } else {
      throw Error("未声明的操作");
    }
    return null;
  };

  const snapshot = appApi.snapshot;
  appApi.snapshot = async () => {
    const data = await snapshot();
    return {
      ...data,
      sessionSources: [
        {
          agentId: session.agentId,
          enabled: true,
          locations: [],
          status: "ready",
          sessionCount: 1,
          scannedFiles: 1,
          totalFiles: 1,
          skippedFiles: 0,
          lastScan: session.updatedAt,
          error: null,
          versions: [],
          capabilities: [],
        },
      ],
    };
  };
  sessionsApi.sessions = async () => ({
    items: [
      {
        ...session,
        projectId: project,
        projectName: "Fixture Project",
        projectPath: "/tmp/fixture",
      },
    ],
    total: 1,
    nextCursor: null,
    summary: null,
    asOf: "2026-10-19T00:00:00Z",
    coverage: "native_history",
    missingCounts: 0,
  });
  sessionsApi.conversation = async (id) => activityConversation(id);
}
