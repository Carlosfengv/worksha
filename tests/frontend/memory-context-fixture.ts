import type { MemorySnapshot } from "@/features/agents/types";
import type { SessionContext } from "@/features/sessions/types";
import { agentProducts } from "@/shared/types/agent";

declare global {
  interface Window {
    memoryFail: boolean;
    contextMissing: boolean;
    memoryQueries: { agent: string; file?: string }[];
  }
}
window.memoryFail = false;
window.contextMissing = false;
window.memoryQueries = [];
export async function memoryFixture(
  agent: string,
  file?: string,
): Promise<MemorySnapshot> {
  window.memoryQueries.push({ agent, file });
  if (window.memoryFail) throw Error("记忆读取权限已撤销");
  const family = agentProducts.find((p) => p.id === agent)!.family;
  return {
    agentId: agent,
    sharedAgents: agentProducts
      .filter((p) => p.family === family)
      .map((p) => p.id),
    files: Array.from({ length: 25 }, (_, n) => ({
      id: `${family}-${n}`,
      name: `memory-${n}.md`,
      path: `/fixture/${family}/memory-${n}.md`,
      scope: "项目记忆",
      project: null,
      sizeBytes: 100,
      modifiedAt: new Date().toISOString(),
    })),
    selected: file
      ? {
          id: file,
          text: `${family} memory body · [已隐藏凭据]`,
          truncated: true,
        }
      : null,
    asOf: new Date().toISOString(),
    limited: true,
    warnings: ["已发现文件，实际加载未知。"],
  };
}
export function contextFixture(): SessionContext {
  if (window.contextMissing)
    return {
      inputTokens: null,
      windowTokens: null,
      usedPercentage: null,
      model: null,
      sampleAt: null,
      source: null,
      compactionCount: 0,
      loadedCount: 0,
      limited: false,
      loaded: [],
      compactions: [],
    };
  return {
    inputTokens: 0,
    windowTokens: 1000,
    usedPercentage: 0,
    model: "fixture-model",
    sampleAt: new Date().toISOString(),
    source: "Codex last_token_usage",
    sampleAgentProductId: null,
    compactionCount: 1,
    loadedCount: 1,
    limited: false,
    compactions: [
      {
        id: "compact-1",
        kind: "compaction",
        label: "上下文压缩边界",
        text: "retained summary",
        timestamp: null,
        sourceLine: 100,
        agentProductId: null,
        truncated: false,
      },
    ],
    loaded: [
      {
        id: "read-1",
        kind: "read",
        label: "读取记忆 / 规则：AGENTS.md",
        text: "actual loaded rule text",
        timestamp: null,
        sourceLine: 101,
        agentProductId: "codex-cli",
        truncated: true,
      },
    ],
  };
}
