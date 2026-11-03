import type {
  GatewayActivity,
  NativeActivity,
  Overview,
  OverviewQuery,
} from "@/features/observation/types";
import { recordProduct, type AgentProductId } from "@/shared/types/agent";
import type { Conversation, NativeSession } from "@/features/sessions/types";

declare global {
  interface Window {
    activityConnection: "gateway" | "official_login";
    activityReverse: boolean;
    overviewDelay: number;
    overviewHeldAgent?: string;
    overviewRelease?: () => void;
    overviewHeldDone: boolean;
    activityRequestAt?: string;
    activityMessageBase?: number;
    overviewReads: OverviewQuery[];
    conversationReads: string[];
    conversationRefreshes: number;
    activityCase:
      | "parallel"
      | "completed"
      | "stale"
      | "error"
      | "many"
      | "stale-native"
      | "unknown-live"
      | "unknown-confirmed"
      | "dual-clients"
      | "tui-clients"
      | "client-session-links"
      | "opencode-tool"
      | "empty"
      | "recent-history"
      | "recent-history-completed"
      | "recent-history-reordered"
      | "recent-history-window"
      | "recent-history-expired"
      | "table-metrics"
      | "table-metrics-updated"
      | "table-metrics-stale"
      | "table-metrics-reasoning"
      | "table-metrics-tools"
      | "table-metrics-gap"
      | "table-metrics-recovered"
      | "table-metrics-output-reasoning"
      | "table-metrics-output-tools"
      | "table-metrics-output-waiting"
      | "table-metrics-output-new"
      | "table-nonstream-metrics"
      | "table-native-metrics"
      | "native-desktop-live"
      | "native-desktop-updated"
      | "native-desktop-thinking"
      | "native-desktop-reconnecting"
      | "native-desktop-completed"
      | "native-desktop-waiting"
      | "native-desktop-first-token"
      | "native-desktop-late"
      | "native-desktop-restored"
      | "native-desktop-restored-live"
      | "native-desktop-restored-completed"
      | "native-desktop-restored-completed-offline"
      | "native-desktop-output-thinking"
      | "native-desktop-output-thinking-silent"
      | "native-desktop-output-thinking-unknown"
      | "native-desktop-output-streaming-silent"
      | "native-desktop-output-tools"
      | "native-desktop-output-tool-idle"
      | "native-desktop-output-returned"
      | "native-desktop-output-reconnecting"
      | "native-desktop-output-completed"
      | "native-desktop-output-next"
      | "native-desktop-input-missing"
      | "native-desktop-input-recovered"
      | "native-desktop-input-next"
      | "native-desktop-input-restarted"
      | "native-desktop-performance-live"
      | "native-desktop-performance-idle"
      | "native-desktop-performance-tools"
      | "native-desktop-performance-restarted"
      | "native-desktop-performance-next"
      | "native-desktop-performance-completed"
      | "native-desktop-performance-parallel"
      | "native-desktop-performance-gateway"
      | "table-history-metrics"
      | "table-tool-metrics"
      | "table-invalid-metrics"
      | "activity-details"
      | "activity-details-updated"
      | "activity-details-completed"
      | "activity-details-stale"
      | "activity-details-scope"
      | "activity-details-long"
      | "message-order"
      | "message-order-tools"
      | "message-order-completed"
      | "message-order-new"
      | "message-order-new-tools"
      | "message-order-page"
      | "message-order-page-tools"
      | "message-row-thinking"
      | "message-row-tools"
      | "message-row-returned"
      | "message-row-streaming"
      | "message-row-completed"
      | "message-row-next"
      | "message-row-stale"
      | "message-row-legacy"
      | "long";
    activityContextState:
      "ready" | "pending" | "partial" | "missing" | "shared";
  }
}
const requestedActivity = new URLSearchParams(location.search).get("activity");
window.activityCase =
  requestedActivity === "completed" ||
  requestedActivity === "error" ||
  requestedActivity === "recent-history" ||
  requestedActivity === "recent-history-window" ||
  requestedActivity === "recent-history-expired"
    ? requestedActivity
    : "parallel";
window.activityContextState = "ready";
window.activityConnection =
  new URLSearchParams(location.search).get("connection") === "official"
    ? "official_login"
    : "gateway";
window.activityReverse = false;
window.overviewDelay = 0;
window.overviewHeldDone = false;
window.overviewReads = [];
window.conversationReads = [];
window.conversationRefreshes = 0;
export const activityQuery: OverviewQuery = {
  range: 24,
  metric: "calls",
  dimension: "agent",
  source: "gateway",
};
export function activitySessions(): NativeSession[] {
  return activityOverview(activityQuery).nativeActivities.map((a) => ({
    id: a.sessionId,
    nativeId: a.sessionId.split(":")[1],
    agentId: a.agentId,
    agentProductId: a.agentProductId ?? null,
    agentProducts: a.agentProductId ? [a.agentProductId] : [],
    hasUnknownClient: false,
    agentVersion: null,
    title: a.title,
    titleSource: "native_title",
    projectPath: null,
    projectName: a.project,
    projectId: null,
    projectRoot: null,
    gitRemote: null,
    createdAt: a.eventAt,
    updatedAt: a.eventAt,
    status: "imported",
    messageCount: 0,
    turnCount: null,
    modelCalls: 0,
    modelErrorCount: 0,
    latestFailureId: null,
    toolCalls: 0,
    mcpCalls: 0,
    skillCalls: 0,
    usage: {
      inputTokens: null,
      outputTokens: null,
      totalTokens: null,
      cachedInputTokens: null,
      cacheWriteInputTokens: null,
      reasoningOutputTokens: null,
    },
    usageQuality: "unknown",
    diskBytes: 0,
    diskAllocatedBytes: null,
    diskScope: "native_transcript_file",
    sourcePath: `/tmp/fixture/${a.sessionId}`,
    partial: false,
  }));
}
export function activityConversation(id: string): Conversation {
  const session = activitySessions().find((s) => s.id === id);
  if (!session) throw Error("找不到会话");
  return {
    session,
    items: [],
    total: 0,
    nextCursor: null,
    asOf: new Date().toISOString(),
    coverage: "native_history",
    warnings: [],
  };
}
export function activityOverview(query: OverviewQuery): Overview {
  if (window.activityCase === "error") throw Error("活动采集暂不可用");
  const now = Date.now();
  const gateway = (
    id: string,
    product: AgentProductId | null,
    speed: number | null,
    ttft: number | null,
    session: string | null,
  ): GatewayActivity => ({
    request: {
      id,
      agentId: product?.startsWith("claude")
        ? "claude"
        : product === "opencode"
          ? "opencode"
          : "codex",
      agentProductId: product,
      sessionId: session,
      providerId: "fixture",
      provider: "MyDeepSeek",
      model: id.endsWith("b") ? "deepseek-v4-pro" : "deepseek-flash",
      endpoint: "http://127.0.0.1:9",
      bindingRevision: 1,
      startedAt: new Date(now - 5000).toISOString(),
      endedAt: null,
      status: "in_progress",
      httpStatus: 200,
      ttftMs: ttft,
      firstTextMs: 2000,
      durationMs: null,
      usage: {
        inputTokens: null,
        outputTokens: null,
        totalTokens: null,
        cachedInputTokens: null,
        cacheWriteInputTokens: null,
        reasoningOutputTokens: null,
      },
      logicalId: null,
      protocol: "chat-completions",
      timingIssue: null,
    },
    phase: "streaming",
    stale: false,
    textTokensPerSecond: speed,
    throughput: null,
    elapsedMs: 5000,
  });
  const native = (
    id: string,
    product: AgentProductId,
    title: string,
  ): NativeActivity => ({
    id,
    sessionId: id,
    agentId:
      product === "opencode"
        ? "opencode"
        : product.startsWith("claude")
          ? "claude"
          : "codex",
    agentProductId: product,
    title,
    project: id.endsWith("a") ? "项目 A" : "项目 B",
    model: "deepseek-flash",
    phase: "processing",
    step: "本轮任务已开始",
    eventAt: new Date(now - 1000).toISOString(),
    turnTtftMs: null,
    stale: false,
  });
  const gateways = [
    gateway("claude-a", "claude-code-cli", 20, 1000, "claude:a"),
    gateway("claude-b", "claude-code-cli", 40, 3000, "claude:b"),
    gateway(
      "claude-desktop",
      "claude-code-desktop",
      90,
      4000,
      "claude:desktop",
    ),
    gateway("codex-cli", "codex-cli", 10, 500, null),
    gateway("unknown", null, 999, 9999, null),
    gateway("open-a", "opencode", 0, 0, "opencode:a"),
    gateway("open-b", "opencode", null, null, "opencode:b"),
    gateway("open-c", "opencode", 900, 9000, "opencode:c"),
  ];
  gateways[7].request.timingIssue = "时序无法确认";
  gateways[7].request.ttftMs = null;
  gateways[7].textTokensPerSecond = null;
  const natives = [
    native("claude:a", "claude-code-cli", "修复登录"),
    native("claude:b", "claude-code-cli", "整理组件"),
    native("claude:desktop", "claude-code-desktop", "桌面编程"),
    native("codex:a", "codex-cli", "检查服务"),
    native("codex:b", "codex-cli", "更新测试"),
    native("codex:desktop-a", "codex-desktop", "排查构建"),
    native("codex:desktop-b", "codex-desktop", "核对文档"),
    native("opencode:a", "opencode", "检查工具"),
    native("opencode:b", "opencode", "检查 MCP"),
    native("opencode:c", "opencode", "检查 Skill"),
  ];
  for (const activity of gateways) {
    const linked = natives.find(
      (a) => a.sessionId === activity.request.sessionId,
    );
    if (!linked) continue;
    linked.messageId = `input:${linked.sessionId}`;
    linked.messageStartedAt = activity.request.startedAt;
    activity.request.activityMessage = {
      id: linked.messageId,
      startedAt: activity.request.startedAt,
    };
  }
  if (["unknown-live", "unknown-confirmed"].includes(window.activityCase)) {
    const a = native("codex:unknown-a", "codex-desktop", "持续检查真实进度");
    const b = native("codex:unknown-b", "codex-desktop", "继续处理项目");
    a.agentProductId =
      window.activityCase === "unknown-confirmed" ? "codex-desktop" : null;
    a.agentAttribution =
      window.activityCase === "unknown-confirmed"
        ? "desktop_runtime_execution"
        : null;
    b.agentProductId = null;
    natives.push(a, b);
  }
  if (["dual-clients", "tui-clients"].includes(window.activityCase)) {
    gateways.length = 0;
    natives.length = 0;
    const cli = native("codex:live-cli-a", "codex-cli", "CLI 只读验证");
    cli.agentAttribution =
      window.activityCase === "tui-clients"
        ? "cli_runtime_execution"
        : "native_exec_turn";
    const desktop = native(
      "codex:live-desktop-b",
      "codex-desktop",
      "桌面并行处理",
    );
    desktop.agentAttribution = "desktop_runtime_execution";
    const unknown = native("codex:unconfirmed", "codex-cli", "未确认客户端");
    unknown.agentProductId = null;
    natives.push(cli, desktop, unknown);
  }
  if (["client-session-links", "opencode-tool"].includes(window.activityCase)) {
    gateways.length = 0;
    natives.length = 0;
    const cli = native(
      "claude:verified-a",
      "claude-code-cli",
      "交互式 Claude 任务",
    );
    cli.agentAttribution = "native_record";
    const sdk = native("claude:sdk-b", "claude-code-cli", "SDK 宿主未确认");
    sdk.agentProductId = null;
    sdk.agentAttribution = "sdk_client";
    const open = native(
      "opencode:ses_verified",
      "opencode",
      "OpenCode 已关联会话",
    );
    open.phase = "waiting_input";
    natives.push(cli, sdk, open);
    gateways.push(gateway("open-linked", "opencode", 50, 1200, open.sessionId));
    open.messageId = "verified-user";
    open.messageStartedAt = gateways[0].request.startedAt;
    if (window.activityCase === "opencode-tool") {
      open.phase = "tool_requested";
      open.step = "正在执行工具：bash";
      gateways[0].request.status = "success";
      gateways[0].phase = "ended";
    }
  }
  if (window.activityCase === "long") {
    natives[0].title = "检查长标题" + "很长的会话标题".repeat(30);
    natives[0].project = "/Users/fixture/" + "long-project-path-".repeat(30);
  }
  if (window.activityCase === "many") {
    gateways.length = 0;
    natives.length = 0;
    for (let n = 0; n < 50; n++) {
      gateways.push(
        gateway(`many-${n}`, "claude-code-cli", n, 1000, `claude:task-${n}`),
      );
      natives.push(
        native(`claude:task-${n}`, "claude-code-cli", `任务 ${n + 1}`),
      );
      natives[n].messageId = `user-${n}`;
      natives[n].messageStartedAt = gateways[n].request.startedAt;
    }
  }
  if (
    window.activityCase === "completed" ||
    window.activityCase === "stale-native"
  ) {
    gateways.forEach((a) => {
      a.request.status = "success";
      a.request.endedAt = new Date(now).toISOString();
      a.phase = "ended";
      a.averageTextTokensPerSecond = 35;
      a.throughput = 8;
    });
    natives.forEach((a) => {
      a.phase =
        window.activityCase === "stale-native" ? "processing" : "waiting_input";
      a.stale = window.activityCase === "stale-native";
    });
  }
  if (window.activityContextState === "shared") {
    gateways.push(gateway("shared", "claude-code-cli", 30, 2000, "claude:a"));
  }
  if (window.activityReverse) {
    gateways.forEach((a, i) => {
      a.request.startedAt = new Date(now - 5000 + i).toISOString();
    });
    natives.forEach((a, i) => {
      a.eventAt = new Date(now - 1000 + i).toISOString();
    });
  }
  if (window.activityCase.startsWith("recent-history")) {
    gateways.length = 0;
    natives.length = 0;
    const running = gateway(
      "recent-running",
      "claude-code-cli",
      20,
      1000,
      "claude:running",
    );
    running.request.startedAt = new Date(now - 30000).toISOString();
    running.request.lastTextMs = 25000;
    const runningNative = native(
      "claude:running",
      "claude-code-cli",
      "持续任务",
    );
    runningNative.eventAt = new Date(now - 10000).toISOString();
    runningNative.messageStartedAt = running.request.startedAt;
    if (window.activityCase === "recent-history-completed") {
      running.request.status = "success";
      running.request.endedAt = new Date(now).toISOString();
      running.phase = "ended";
      running.averageTextTokensPerSecond = 36;
      runningNative.phase = "waiting_input";
      runningNative.eventAt = new Date(now - 1000).toISOString();
    }
    const previous = gateway(
      "recent-previous",
      "claude-code-cli",
      null,
      600,
      "claude:previous",
    );
    previous.request.startedAt = new Date(now - 120000).toISOString();
    previous.request.endedAt = new Date(now - 60000).toISOString();
    previous.request.status = "success";
    previous.phase = "ended";
    previous.averageTextTokensPerSecond = 35;
    const previousNative = native(
      "claude:previous",
      "claude-code-cli",
      "较早网关活动",
    );
    previousNative.eventAt = new Date(now - 65000).toISOString();
    previousNative.messageStartedAt = previous.request.startedAt;
    previousNative.phase = "waiting_input";
    const failed = gateway("recent-failed", "opencode", null, null, null);
    failed.request.status = "failed";
    failed.request.startedAt = new Date(now - 9000).toISOString();
    failed.request.endedAt = new Date(now - 3000).toISOString();
    failed.request.timingIssue = "响应中断";
    failed.phase = "ended";
    const latest = native(
      "codex:recent-done",
      "codex-desktop",
      "最近原生已结束",
    );
    latest.phase = "waiting_input";
    latest.messageStartedAt = new Date(now - 2000).toISOString();
    const older = native("codex:older", "codex-cli", "较早原生活动");
    older.agentProductId = null;
    older.eventAt = new Date(now - 180000).toISOString();
    older.messageStartedAt = older.eventAt;
    older.stale = true;
    if (window.activityCase === "recent-history-reordered") {
      older.eventAt = new Date(now).toISOString();
      older.phase = "waiting_input";
      older.stale = false;
    }
    gateways.push(running, previous, failed);
    natives.push(runningNative, previousNative, latest, older);
    if (window.activityCase === "recent-history-window") {
      const windowMs = 3 * 60 * 60 * 1000;
      running.request.startedAt = new Date(
        now - 4 * 60 * 60 * 1000,
      ).toISOString();
      running.request.lastTextMs = 4 * 60 * 60 * 1000 - 5000;
      previous.request.startedAt = new Date(
        now - windowMs - 60000,
      ).toISOString();
      previous.request.endedAt = new Date(now - windowMs - 1).toISOString();
      previousNative.eventAt = previous.request.endedAt;
      latest.eventAt = new Date(now - windowMs - 1).toISOString();
      failed.request.startedAt = new Date(now - windowMs - 1000).toISOString();
      failed.request.endedAt = new Date(now - windowMs).toISOString();
      older.eventAt = failed.request.endedAt;
      runningNative.messageStartedAt = running.request.startedAt;
      previousNative.messageStartedAt = previous.request.startedAt;
      latest.messageStartedAt = latest.eventAt;
      older.messageStartedAt = older.eventAt;
    }
    if (window.activityCase === "recent-history-expired") {
      const old = new Date(now - 4 * 60 * 60 * 1000).toISOString();
      gateways.forEach((a) => {
        a.request.startedAt = old;
        a.request.endedAt = old;
        a.request.status = "success";
        a.phase = "ended";
      });
      natives.forEach((a) => {
        a.eventAt = old;
        a.phase = "waiting_input";
      });
    }
  }
  if (window.activityCase.startsWith("native-desktop-")) {
    gateways.length = 0;
    natives.length = 0;
    const a = native(
      "codex:desktop-native",
      "codex-desktop",
      "原生采集同一条消息",
    );
    a.messageId = "native-message-one";
    a.messageStartedAt = new Date(now - 5000).toISOString();
    a.model = "gpt-native";
    a.phase = "streaming";
    a.turnTtftMs = null;
    a.realtime = {
      state: "connected",
      firstTokenMs: 1050,
      firstTokenMissed: false,
      firstTextMs: 1200,
      firstOutputMs: 900,
      textTokensPerSecond:
        window.activityCase === "native-desktop-updated" ? 42 : 20,
      averageTextTokensPerSecond: 32,
    };
    if (window.activityCase.startsWith("native-desktop-restored")) {
      const live = window.activityCase === "native-desktop-restored-live";
      a.phase = live ? "streaming" : "processing";
      a.realtime.state = live ? "connected" : "connecting";
      a.realtime.firstTextMs = null;
      a.realtime.textTokensPerSecond = live ? 42 : null;
      a.realtime.averageTextTokensPerSecond = null;
    }
    if (window.activityCase.startsWith("native-desktop-restored-completed")) {
      a.phase = "waiting_input";
      a.realtime.state = window.activityCase.endsWith("offline")
        ? "reconnecting"
        : "connecting";
      a.realtime.averageTextTokensPerSecond = 32;
    }
    if (
      [
        "native-desktop-waiting",
        "native-desktop-first-token",
        "native-desktop-late",
      ].includes(window.activityCase)
    ) {
      a.phase = "thinking";
      a.realtime.firstTextMs = null;
      a.realtime.firstTokenMs =
        window.activityCase === "native-desktop-first-token" ? 1050 : null;
      a.realtime.firstTokenMissed =
        window.activityCase === "native-desktop-late";
      a.realtime.textTokensPerSecond = null;
    }
    if (window.activityCase === "native-desktop-thinking") {
      a.phase = "thinking";
      a.realtime.textTokensPerSecond = null;
    }
    if (window.activityCase === "native-desktop-reconnecting") {
      a.realtime.state = "reconnecting";
      a.realtime.textTokensPerSecond = null;
    }
    if (window.activityCase === "native-desktop-completed") {
      a.phase = "waiting_input";
      a.turnTtftMs = 1000;
      a.realtime.textTokensPerSecond = null;
    }
    if (window.activityCase.startsWith("native-desktop-output-")) {
      a.phase = "thinking";
      a.realtime.firstTextMs = null;
      a.realtime.textTokensPerSecond = null;
      a.realtime.outputTokensPerSecond = 37;
      if (window.activityCase === "native-desktop-output-thinking-unknown")
        a.realtime.outputTokensPerSecond = null;
      if (
        [
          "native-desktop-output-thinking-silent",
          "native-desktop-output-streaming-silent",
        ].includes(window.activityCase)
      )
        a.realtime.outputTokensPerSecond = 0;
      if (window.activityCase === "native-desktop-output-streaming-silent") {
        a.phase = "streaming";
        a.realtime.textTokensPerSecond = 0;
      }
      if (
        [
          "native-desktop-output-tools",
          "native-desktop-output-tool-idle",
        ].includes(window.activityCase)
      ) {
        a.phase = "tool_requested";
        a.step = "Read 工具执行中";
        a.toolNames = ["Read"];
        a.realtime.outputTokensPerSecond = window.activityCase.endsWith("idle")
          ? 0
          : 12.5;
      }
      if (window.activityCase === "native-desktop-output-returned") {
        a.phase = "tool_returned";
        a.step = "Read 已返回，等待模型继续";
        a.realtime.outputTokensPerSecond = 0;
      }
      if (window.activityCase === "native-desktop-output-reconnecting")
        a.realtime.state = "reconnecting";
      if (window.activityCase === "native-desktop-output-completed") {
        a.phase = "waiting_input";
        a.realtime.outputTokensPerSecond = null;
      }
      if (window.activityCase === "native-desktop-output-next") {
        a.messageId = "native-next-message";
        a.messageStartedAt = new Date(now).toISOString();
        a.phase = "processing";
        a.realtime.firstTokenMs = null;
        a.realtime.outputTokensPerSecond = null;
        a.realtime.averageTextTokensPerSecond = null;
      }
    }
    if (window.activityCase.startsWith("native-desktop-input-")) {
      a.title = "固定的会话标题";
      a.phase = "waiting_input";
      a.turnTtftMs = 1000;
      a.realtime.textTokensPerSecond = null;
      if (window.activityCase !== "native-desktop-input-missing") {
        const next = [
          "native-desktop-input-next",
          "native-desktop-input-restarted",
        ].includes(window.activityCase);
        a.messageId = next ? "native-message-two" : "native-message-one";
        a.messageStartedAt = new Date(now - (next ? 2000 : 5000)).toISOString();
        a.userInput = {
          text: next ? "第二条原生消息内容" : "第一条原生消息内容",
          at: a.messageStartedAt,
        };
        if (window.activityCase === "native-desktop-input-restarted")
          a.realtime.state = "connecting";
      }
    }
    if (window.activityCase.startsWith("native-desktop-performance-")) {
      const base = (window.activityMessageBase ??= now - 60000);
      const recent = {
        at: base + 45000,
        windowMs: 5000,
        tokensPerSecond: 68.2,
      };
      a.messageStartedAt = new Date(base).toISOString();
      a.title = "性能采集会话";
      a.userInput = { text: "原生采集同一条消息", at: a.messageStartedAt };
      a.phase = "thinking";
      a.messageUsage = {
        outputTokens: 1200,
        reasoningTokens: 800,
        at: new Date(base + 45000).toISOString(),
      };
      a.realtime.outputTokensPerSecond = 0;
      a.realtime.sampling = {
        at: now,
        windowMs: 5000,
        tokensPerSecond: null,
        recent,
        reported: a.messageUsage,
      };
      if (
        window.activityCase.endsWith("live") ||
        window.activityCase.endsWith("parallel")
      ) {
        a.realtime.sampling.tokensPerSecond = 24;
        a.realtime.sampling.recent = { ...recent, at: now - 2000 };
      }
      if (window.activityCase.endsWith("tools")) {
        a.phase = "tool_requested";
        a.step = "Read 工具执行中";
        a.toolNames = ["Read"];
      }
      if (window.activityCase.endsWith("restarted"))
        a.realtime.state = "connecting";
      if (window.activityCase.endsWith("next")) {
        a.messageId = "native-message-two";
        a.messageStartedAt = new Date(now).toISOString();
        a.realtime.averageTextTokensPerSecond = null;
        a.realtime.firstTokenMs = null;
        a.realtime.firstTextMs = null;
        a.realtime.firstOutputMs = null;
      }
      if (window.activityCase.endsWith("completed")) {
        a.phase = "waiting_input";
        a.turnTtftMs = 1000;
      }
      if (window.activityCase.endsWith("parallel")) {
        const b = structuredClone(a);
        b.sessionId = "codex:parallel-two";
        b.id = b.sessionId;
        b.realtime!.sampling!.at = now - 3000;
        b.realtime!.sampling!.recent!.at = now - 3100;
        b.realtime!.sampling!.tokensPerSecond = 100;
        natives.push(b);
      }
      if (window.activityCase.endsWith("gateway")) {
        a.sessionId = "claude:smooth-gateway";
        a.id = a.sessionId;
        a.agentId = "claude";
        a.agentProductId = "claude-code-cli";
        const g = gateway(
          "smooth-gateway",
          "claude-code-cli",
          0,
          1100,
          a.sessionId,
        );
        g.request.startedAt = new Date(base).toISOString();
        a.model = g.request.model;
        g.request.activityMessage = {
          id: "one",
          startedAt: g.request.startedAt,
        };
        g.sampling = {
          at: now,
          windowMs: 5000,
          tokensPerSecond: null,
          recent,
          reported: null,
        };
        g.request.usage.outputTokens = 900;
        g.request.usage.reasoningOutputTokens = 500;
        gateways.push(g);
        a.messageUsage = null;
        a.realtime = null;
      }
    }
    const old = {
      ...a,
      id: "old-native-message",
      messageId: "native-message-old",
      title: "同会话上一条消息",
      phase: "waiting_input",
      realtime: null,
      turnTtftMs: 800,
      messageStartedAt: new Date(
        window.activityCase.startsWith("native-desktop-performance-")
          ? Date.parse(a.messageStartedAt) - 60000
          : now - 60000,
      ).toISOString(),
    };
    if (window.activityCase.startsWith("native-desktop-input-"))
      old.userInput = {
        text: "更早一条消息内容",
        at: new Date(now - 20000).toISOString(),
      };
    natives.push(a, old);
  }
  if (window.activityCase === "empty") {
    gateways.length = 0;
    natives.length = 0;
  }
  if (window.activityCase.startsWith("table-")) {
    gateways.length = 0;
    natives.length = 0;
    if (window.activityCase.startsWith("table-metrics")) {
      const live = gateway(
        "table-live",
        "claude-code-cli",
        window.activityCase === "table-metrics-updated" ? 42 : 20,
        1200,
        "claude:table-live",
      );
      live.request.lastTextMs = 4000;
      live.request.startedAt =
        window.activityRequestAt ?? live.request.startedAt;
      live.stale = window.activityCase === "table-metrics-stale";
      if (window.activityCase === "table-metrics-reasoning")
        live.phase = "public_reasoning";
      if (window.activityCase === "table-metrics-tools")
        live.phase = "tool_arguments";
      if (
        ["table-metrics-gap", "table-metrics-recovered"].includes(
          window.activityCase,
        )
      ) {
        live.request.timingIssue = "部分流事件无法解析；未补算未知时序或用量";
        live.textTokensPerSecond =
          window.activityCase === "table-metrics-gap" ? null : 42;
      }
      gateways.push(live, gateway("table-zero", "opencode", 0, 0, null));
      const progress = native(
        "claude:table-live",
        "claude-code-cli",
        "实时请求指标",
      );
      progress.messageId = "table-input";
      progress.messageStartedAt = live.request.startedAt;
      live.request.activityMessage = {
        id: progress.messageId,
        startedAt: live.request.startedAt,
      };
      natives.push(progress);
      if (window.activityCase.startsWith("table-metrics-output-")) {
        const first = gateway(
          "table-first",
          "claude-code-cli",
          null,
          900,
          live.request.sessionId,
        );
        first.request.startedAt = new Date(
          Date.parse(live.request.startedAt) - 10000,
        ).toISOString();
        first.request.status = "success";
        first.request.endedAt = new Date(
          Date.parse(live.request.startedAt) - 5000,
        ).toISOString();
        first.request.activityMessage = {
          id: "table-input",
          startedAt: first.request.startedAt,
        };
        first.request.firstTextMs = null;
        live.request.activityMessage = { ...first.request.activityMessage };
        progress.messageStartedAt = first.request.startedAt;
        live.request.ttftMs = null;
        live.textTokensPerSecond = null;
        live.phase = window.activityCase.endsWith("reasoning")
          ? "public_reasoning"
          : window.activityCase.endsWith("tools")
            ? "tool_arguments"
            : "waiting_first";
        live.outputTokensPerSecond = window.activityCase.endsWith("reasoning")
          ? 37
          : window.activityCase.endsWith("tools")
            ? 64
            : null;
        gateways.push(first);
        if (window.activityCase.endsWith("new")) {
          progress.messageId = "table-next-input";
          progress.messageStartedAt = live.request.startedAt;
          live.request.activityMessage = {
            id: progress.messageId,
            startedAt: live.request.startedAt,
          };
        }
      }
    } else if (window.activityCase === "table-nonstream-metrics") {
      const finished = gateway(
        "nonstream",
        "claude-code-cli",
        null,
        null,
        null,
      );
      finished.request.status = "success";
      finished.request.endedAt = new Date(now - 2000).toISOString();
      finished.request.timingIssue =
        "非流式响应不测 TTFT；仅保留端到端耗时与已报告用量";
      finished.throughput = 8;
      gateways.push(finished);
    } else if (window.activityCase === "table-native-metrics") {
      const reported = native("codex:reported", "codex-cli", "原生首出已上报");
      reported.turnTtftMs = 1234;
      reported.phase = "waiting_input";
      const zero = native("codex:zero", "codex-cli", "原生零首出");
      zero.turnTtftMs = 0;
      zero.phase = "waiting_input";
      natives.push(
        reported,
        zero,
        native("codex:unknown", "codex-cli", "原生时序未知"),
      );
    } else if (
      window.activityCase === "table-history-metrics" ||
      window.activityCase === "table-tool-metrics"
    ) {
      const last = gateway(
        "table-last",
        "claude-code-cli",
        null,
        900,
        "claude:history",
      );
      last.request.status = "success";
      last.request.endedAt = new Date(now - 2000).toISOString();
      last.phase = "ended";
      last.request.activityMessage = {
        id: "history-current",
        startedAt: last.request.startedAt,
      };
      last.averageTextTokensPerSecond = 35;
      last.throughput = 8;
      const progress = native(
        "claude:history",
        "claude-code-cli",
        "请求后的原生状态",
      );
      progress.messageId = "history-current";
      progress.messageStartedAt = last.request.startedAt;
      progress.phase =
        window.activityCase === "table-tool-metrics"
          ? "tool_requested"
          : "waiting_input";
      gateways.push(last);
      natives.push(progress);
      if (window.activityCase === "table-history-metrics") {
        const older = structuredClone(last);
        older.request.id = "table-older";
        older.request.startedAt = new Date(now - 60000).toISOString();
        older.request.endedAt = new Date(now - 30000).toISOString();
        older.request.activityMessage = {
          id: "history-previous",
          startedAt: older.request.startedAt,
        };
        older.averageTextTokensPerSecond = null;
        older.throughput = 8;
        gateways.push(older);
        natives.push(
          native("claude:other", "claude-code-cli", "另一会话没有指标"),
        );
      }
    } else if (window.activityCase === "table-invalid-metrics") {
      const negative = gateway("negative", "claude-code-cli", -20, -1000, null);
      const invalid = gateway("invalid", "codex-cli", Infinity, NaN, null);
      const issue = gateway("timing-issue", "opencode", 99, 500, null);
      issue.request.timingIssue = "时序无法确认";
      issue.request.ttftMs = null;
      issue.textTokensPerSecond = null;
      gateways.push(negative, invalid, issue);
    }
  }
  if (window.activityCase.startsWith("activity-details")) {
    gateways.length = 0;
    natives.length = 0;
    const thinking = native(
      "claude:details",
      "claude-code-cli",
      "登录修复会话",
    );
    thinking.phase = "thinking";
    thinking.step = "收到原生思考进度";
    thinking.userInput = {
      text: "请检查最新登录失败，再补充回归测试。",
      at: new Date(now - 7000).toISOString(),
    };
    const tool = native("opencode:details", "opencode", "检查工具链");
    tool.phase = "tool_requested";
    tool.step = "已提出调用：Read、Grep";
    tool.toolNames = ["Read", "Grep"];
    tool.userInput = {
      text: "先读取项目说明，再查找配置。",
      at: new Date(now - 6000).toISOString(),
    };
    const request = gateway("details-request", "codex-cli", null, 700, null);
    request.phase = "tool_arguments";
    request.toolNames = ["functions.exec_command", "mcp__files__read"];
    request.userInput = {
      text: "核对请求参数，不要修改配置。",
      at: request.request.startedAt,
    };
    if (window.activityCase === "activity-details-updated") {
      thinking.phase = "tool_requested";
      thinking.step = "已提出调用：Edit、Bash";
      thinking.toolNames = ["Edit", "Bash"];
      thinking.userInput = {
        text: "现在修改登录按钮颜色。",
        at: new Date(now - 2000).toISOString(),
      };
      tool.phase = "tool_returned";
      tool.step = "Read 已返回；仍待返回：Grep";
      tool.toolNames = ["Read"];
      request.phase = "public_reasoning";
      request.toolNames = [];
    } else if (window.activityCase === "activity-details-completed") {
      thinking.phase = "waiting_input";
      thinking.step = "本轮已完成，等待输入";
      tool.phase = "waiting_input";
      tool.step = "本轮已完成，等待输入";
      tool.toolNames = [];
      request.request.status = "success";
      request.request.endedAt = new Date(now).toISOString();
      request.phase = "ended";
    } else if (window.activityCase === "activity-details-long") {
      thinking.userInput.text =
        "检查包含中文和🙂的长输入，".repeat(30).slice(0, 240) + "…";
      tool.toolNames = ["mcp__" + "very_long_tool_name_".repeat(5), "Read"];
      tool.step = "已提出调用：" + tool.toolNames.join("、");
    } else if (window.activityCase === "activity-details-scope") {
      const future = gateway(
        "details-future",
        "claude-code-cli",
        20,
        1000,
        "claude:details",
      );
      thinking.phase = "processing";
      thinking.step = "收到新的用户消息";
      thinking.userInput = {
        text: "后续轮次的输入不得套给旧请求。",
        at: new Date(now - 1000).toISOString(),
      };
      const older = structuredClone(future);
      future.request.activityMessage = {
        id: "input-current-request",
        startedAt: future.request.startedAt,
      };
      older.request.id = "details-old";
      older.request.startedAt = new Date(now - 15000).toISOString();
      older.request.endedAt = new Date(now - 4000).toISOString();
      older.request.status = "success";
      older.request.activityMessage = {
        id: "input-older-request",
        startedAt: older.request.startedAt,
      };
      older.userInput = {
        text: "旧请求自身的输入摘要。",
        at: older.request.startedAt,
      };
      request.userInput.text = "<img src=x onerror=alert(1)> 只是普通输入文本";
      gateways.push(future, older, request);
      natives.push(thinking);
    }
    if (window.activityCase !== "activity-details-scope") {
      gateways.push(request);
      natives.push(thinking, tool);
    }
  }
  if (window.activityCase.startsWith("message-order")) {
    const base = (window.activityMessageBase ??= now);
    const at = (seconds: number) =>
      new Date(base - seconds * 1000).toISOString();
    gateways.length = 0;
    natives.length = 0;
    const newer = native("codex:order-newer", "codex-cli", "较新消息");
    newer.messageStartedAt = at(20);
    newer.userInput = { text: "检查新的请求", at: at(20) };
    newer.eventAt = at(10);
    newer.phase = "thinking";
    const older = native("claude:order-older", "claude-code-cli", "较早消息");
    older.messageStartedAt = at(40);
    older.userInput = { text: "检查较早的请求", at: at(40) };
    older.eventAt = at(1);
    older.phase = "tool_requested";
    older.step = "已提出调用：Read";
    const linked = native(
      "claude:order-linked",
      "claude-code-desktop",
      "网关会话",
    );
    linked.messageStartedAt = at(30);
    linked.eventAt = at(9);
    const request = gateway(
      "order-linked",
      "claude-code-desktop",
      20,
      700,
      linked.sessionId,
    );
    request.request.startedAt = at(25);
    request.request.lastTextMs = 24000;
    const direct = gateway("order-direct", "opencode", null, 900, null);
    direct.request.startedAt = at(60);
    direct.request.endedAt = at(2);
    direct.request.status = "success";
    direct.phase = "ended";
    const unknown = native(
      "codex:order-unknown",
      "codex-desktop",
      "消息时间未知",
    );
    unknown.messageStartedAt = null;
    unknown.userInput = { text: "可读摘要，消息开始时间未知", at: at(3) };
    unknown.eventAt = at(0);
    if (
      window.activityCase === "message-order-new" ||
      window.activityCase === "message-order-new-tools"
    ) {
      older.messageStartedAt = at(0);
      older.userInput = { text: "开始下一条消息", at: at(0) };
      older.phase = window.activityCase.endsWith("tools")
        ? "tool_requested"
        : "processing";
      older.step = window.activityCase.endsWith("tools")
        ? "已提出调用：Edit"
        : "收到新的用户消息";
      older.eventAt = at(0);
    } else if (window.activityCase.endsWith("tools")) {
      older.eventAt = at(0);
      older.step = "已提出调用：Read、Grep";
      request.request.lastTextMs = 25000;
      unknown.eventAt = at(0);
      unknown.step = "已提出调用：Bash";
    } else if (window.activityCase === "message-order-completed") {
      older.eventAt = at(0);
      older.phase = "waiting_input";
      older.step = "本轮已完成，等待输入";
      request.request.status = "success";
      request.request.endedAt = at(2);
      request.phase = "ended";
      linked.phase = "waiting_input";
      linked.eventAt = at(1);
    }
    gateways.push(request, direct);
    natives.push(unknown, older, linked, newer);
    if (window.activityCase.includes("page")) {
      for (const seconds of [70, 80]) {
        const row = native(
          `codex:order-${seconds}`,
          "codex-cli",
          `更早消息 ${seconds}`,
        );
        row.messageStartedAt = at(seconds);
        row.eventAt = at(window.activityCase.endsWith("tools") ? 0 : 5);
        natives.push(row);
      }
    }
  }
  if (window.activityCase.startsWith("message-row")) {
    const base = (window.activityMessageBase ??= now);
    const at = (seconds: number) =>
      new Date(base - seconds * 1000).toISOString();
    natives.length = 0;
    gateways.length = 0;
    const first = native(
      "opencode:message-session",
      "opencode",
      "消息归并会话",
    );
    first.messageId = "user-1";
    first.messageStartedAt = at(20);
    first.userInput = { text: "检查登录", at: at(20) };
    first.phase = "thinking";
    first.step = "收到原生思考进度";
    first.eventAt = at(8);
    const request = gateway(
      "message-request-1",
      "opencode",
      null,
      700,
      first.sessionId,
    );
    request.request.activityMessage = {
      id: "fingerprint-first",
      startedAt: at(19),
    };
    request.request.startedAt = at(19);
    request.phase = "public_reasoning";
    if (
      [
        "message-row-tools",
        "message-row-returned",
        "message-row-streaming",
        "message-row-completed",
        "message-row-next",
        "message-row-stale",
      ].includes(window.activityCase)
    ) {
      request.request.status = "success";
      request.request.endedAt = at(9);
      request.phase = "ended";
      request.averageTextTokensPerSecond = 30;
      request.toolNames = ["Read", "Grep"];
      first.phase =
        window.activityCase === "message-row-tools"
          ? "tool_requested"
          : "tool_returned";
      first.step =
        window.activityCase === "message-row-tools"
          ? "已提出调用：Read、Grep"
          : "Read 已返回；仍待返回：Grep";
      first.toolNames = ["Read", "Grep"];
      first.eventAt = at(6);
    }
    gateways.push(request);
    if (
      [
        "message-row-streaming",
        "message-row-completed",
        "message-row-next",
        "message-row-stale",
      ].includes(window.activityCase)
    ) {
      for (let i = 2; i <= 7; i++) {
        const next = gateway(
          `message-request-${i}`,
          "opencode",
          i === 7 ? 42 : null,
          900,
          first.sessionId,
        );
        next.request.activityMessage = { ...request.request.activityMessage! };
        next.request.startedAt = at(12 - i);
        next.request.status = i === 7 ? "in_progress" : "success";
        next.request.endedAt = i === 7 ? null : at(11 - i);
        next.request.lastTextMs = i === 7 ? 4000 : 0;
        gateways.push(next);
      }
    }
    if (
      ["message-row-completed", "message-row-next"].includes(
        window.activityCase,
      )
    ) {
      first.phase = "waiting_input";
      first.step = "本轮已完成，等待输入";
      first.eventAt = at(0);
      for (const a of gateways) {
        a.request.status = "success";
        a.request.endedAt = at(1);
        a.phase = "ended";
        a.averageTextTokensPerSecond = 36;
      }
    }
    natives.push(first);
    if (window.activityCase === "message-row-next") {
      const second = {
        ...first,
        messageId: "user-2",
        messageStartedAt: at(0),
        userInput: { text: "检查登录", at: at(0) },
        phase: "processing",
        step: "收到新的用户消息",
        eventAt: at(0),
      };
      natives.push(second);
    }
    if (window.activityCase === "message-row-legacy") {
      natives.length = 0;
      for (const [id, seconds, text] of [
        ["legacy-user-1", 60, "检查登录"],
        ["legacy-user-2", 40, "核对设置"],
        ["legacy-user-3", 20, "检查登录"],
      ] as const) {
        natives.push({
          ...first,
          messageId: id,
          messageStartedAt: at(seconds),
          userInput: { text, at: at(seconds) },
          phase: "waiting_input",
          step: "本条消息已完成，等待输入",
          eventAt: at(seconds - 5),
        });
      }
      for (let i = 2; i <= 8; i++) {
        const previous = gateway(
          `legacy-${i}`,
          "opencode",
          null,
          600,
          first.sessionId,
        );
        previous.request.status = "success";
        previous.request.startedAt = at(
          i <= 3 ? 55 - i : i <= 5 ? 35 - i : 15 - i,
        );
        previous.request.endedAt = at(i);
        gateways.push(previous);
      }
      delete request.request.activityMessage;
      request.request.status = "success";
      request.request.endedAt = at(5);
    }
    if (window.activityCase === "message-row-stale") {
      for (const a of natives) a.stale = true;
      for (const a of gateways) a.stale = true;
    }
  }
  const sessionContexts = [
    ...new Set([
      ...natives.map((a) => a.sessionId),
      ...gateways.flatMap((a) =>
        a.request.sessionId ? [a.request.sessionId] : [],
      ),
    ]),
  ].map((sessionId) => {
    const input =
      sessionId === "codex:a" ? 0 : sessionId.endsWith("b") ? 200 : 100;
    const missing = window.activityContextState === "missing";
    const windowTokens = sessionId.startsWith("opencode") ? null : 1000;
    return {
      sessionId,
      context: missing
        ? null
        : {
            inputTokens: input,
            windowTokens,
            usedPercentage: windowTokens ? (input / windowTokens) * 100 : null,
            sampleAt: new Date(now - 5000).toISOString(),
            model: "fixture-model",
            source: "原生上下文样本",
            sampleAgentProductId: null,
          },
      checkedAt: new Date(now).toISOString(),
      state:
        window.activityContextState === "pending"
          ? ("pending" as const)
          : window.activityContextState === "partial"
            ? ("partial" as const)
            : ("ready" as const),
    };
  });
  return {
    sessionContexts,
    asOf: new Date(
      window.activityCase === "stale" ||
        window.activityCase === "activity-details-stale" ||
        window.activityCase === "table-metrics-stale"
        ? now - 10000
        : now,
    ).toISOString(),
    nativeActivities: natives.filter(
      (a) =>
        !query.agent ||
        (query.agent === "unknown"
          ? recordProduct(a) == null
          : recordProduct(a) === query.agent),
    ),
    gatewayActivities: gateways.filter(
      (a) =>
        !query.agent ||
        (query.agent === "unknown"
          ? recordProduct(a.request) == null
          : recordProduct(a.request) === query.agent),
    ),
    bindings: [],
    collector: {
      checkedAt: new Date(now).toISOString(),
      error: null,
      persistenceError: null,
    },
    timings: {
      success: 0,
      ttftSamples: 0,
      ttftP50Ms: null,
      ttftP95Ms: null,
      throughputSamples: 0,
      throughputP50: null,
      retryGroupsUnmeasurable: 0,
    },
    analysis: {
      asOf: new Date(now).toISOString(),
      windowStart: new Date(now - 86400000).toISOString(),
      query: { ...query, agent: query.agent ?? null },
      total: 0,
      samples: 0,
      reported: 0,
      missing: 0,
      missingTime: 0,
      provisional: 0,
      agents: [],
      models: [],
      buckets: [],
      coverage: "partial",
      unit: "calls",
      ...(query.metric === "cost"
        ? {
            total: query.source === "gateway" ? 0.004 : null,
            samples: 2,
            missing: 1,
            unit: "USD",
            cost: {
              actualUsd: query.source === "gateway" ? 0 : null,
              estimatedUsd: query.source === "gateway" ? 0.000000004 : null,
              unknownAttempts: query.source === "gateway" ? 1 : 2,
            },
            agents: [
              {
                id: query.agent ?? "opencode",
                value: query.source === "gateway" ? 0.004 : null,
                missing: 1,
                samples: 2,
              },
            ],
            models: [
              {
                id: "deepseek-flash",
                value: query.source === "gateway" ? 0.004 : null,
                missing: 1,
                samples: 2,
              },
            ],
          }
        : {}),
    },
  };
}
