import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import {
  historyWriter,
  assertWritingPage,
  coherentWritingPages,
} from "./history-writer.mjs";

const options = { files: 2, calls: 3, batch: 2 };
function page(name, extra) {
  const base = ["claude", "codex"].includes(name) ? 6 : 12;
  const count = base + extra;
  const totals = {
    calls: count,
    unknownOutcomes: count,
    errors: 0,
    inProgress: 0,
    latencyP95Ms: null,
    ttftP95Ms: null,
    latencySamples: 0,
    ttftSamples: 0,
    attempts: count,
    modelCalls: count,
    missingUsage: 0,
    usageMissing: 0,
    usage: {
      inputTokens: base * 10 + extra * 20,
      outputTokens: base * 2 + extra * 3,
      totalTokens: base * 12 + extra * 23,
      cachedInputTokens: null,
      cacheWriteInputTokens: null,
      reasoningOutputTokens: null,
    },
    cost: { actual: [], estimated: [], unknownCalls: count },
  };
  if (name === "overview")
    return { analysis: { samples: count, total: totals.usage.totalTokens } };
  if (name === "providers")
    return {
      sourceState: "ready",
      selected: totals,
      summary: totals,
      totalCalls: count,
      models: [0, 1, 2, "codex"].map((ordinal) => {
        const modelBase = ordinal === "codex" ? 6 : 2;
        const added = ordinal === 0 ? extra : 0;
        return {
          model: `soak-${ordinal}`,
          totals: {
            calls: modelBase + added,
            usage: {
              inputTokens: modelBase * 10 + added * 20,
              outputTokens: modelBase * 2 + added * 3,
              totalTokens: modelBase * 12 + added * 23,
            },
            cost: {
              actual: [],
              estimated: [],
              unknownCalls: modelBase + added,
            },
          },
        };
      }),
    };
  if (name === "requests")
    Object.assign(totals, {
      requests: count,
      attempts: 0,
      statuses: { unknown: count },
      ...Object.fromEntries(
        ["ttft", "duration", "throughput"].map((key) => [
          key,
          { samples: 0, p50: null, p95: null },
        ]),
      ),
    });
  const fees = {
    sourceState: "ready",
    nativeResponses: count,
    unlinkedNativeResponses: count,
    gatewayAttempts: 0,
    matchedNativeResponses: 0,
    conflictingNativeResponses: 0,
    responseLinkedAttempts: 0,
    sessionLinkedAttempts: 0,
    cost: { actual: [], estimated: [], unknownCalls: 0 },
  };
  if (name === "projects") totals.associatedCosts = fees;
  return {
    sourceState: "ready",
    summary: totals,
    items: [],
    associatedCosts: fees,
    sourceReport: {
      native: structuredClone(totals),
      matchedNative: 0,
      ambiguousNative: 0,
      usageConflicts: 0,
      nativeMissingTime: 0,
      unlinkedNative: count,
    },
    total: name === "sessions" ? 4 : count,
  };
}
const committed = { claude: 4, codex: 2 };
test("current-time writes capture one timestamp per complete append batch", () => {
  const root = mkdtempSync(path.join(tmpdir(), "history-writer-time-"));
  try {
    const paths = ["claude", "codex"].map((agent) => path.join(root, agent));
    for (const file of paths) writeFileSync(file, "{}\n");
    let captured = 0;
    const times = ["2026-10-24T01:00:00.001Z", "2026-10-24T01:00:01.002Z"];
    const writer = historyWriter({
      paths,
      files: 1,
      journal: path.join(root, "ledger"),
      timestamp: () => times[captured++],
      readers: () => 8,
    });
    writer.append(2);
    writer.append(2);
    assert.equal(captured, 2);
    paths.forEach((file, index) => {
      const rows = readFileSync(file, "utf8")
        .trim()
        .split("\n")
        .slice(1)
        .map(JSON.parse);
      assert.deepEqual(
        rows.map((row) => row.timestamp),
        [times[index], times[index]],
      );
    });
    assert.equal(writer.verify().batches, 2);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
test("model scan progress preserves exact generations and rejects unusable sources", () => {
  for (const name of ["claude", "codex"]) {
    const scanning = page(name, 2);
    scanning.sourceState = "scanning";
    assert.equal(
      assertWritingPage(name, scanning, options, committed).extra,
      2,
    );
    scanning.summary.usage.totalTokens--;
    assert.throws(() => assertWritingPage(name, scanning, options, committed));
    for (const sourceState of ["error", "disabled", "unavailable", "unknown"]) {
      const failed = { ...page(name, 2), sourceState };
      assert.throws(() => assertWritingPage(name, failed, options, committed));
    }
  }
  const provider = { ...page("providers", 2), sourceState: "scanning" };
  assert.throws(() =>
    assertWritingPage("providers", provider, options, committed),
  );
});
test("append oracle accepts delayed complete generations and preserves unknowns", () => {
  for (const name of [
    "overview",
    "providers",
    "claude",
    "codex",
    "sessions",
    "projects",
    "requests",
  ])
    assert.equal(
      assertWritingPage(name, page(name, 2), options, committed).extra,
      2,
    );
});
test("mixed token/count generations, lost usage and uncommitted rows fail", () => {
  const mixed = page("requests", 2);
  mixed.summary.usage.totalTokens--;
  assert.throws(() => assertWritingPage("requests", mixed, options, committed));
  const unknown = page("claude", 2);
  unknown.summary.usage.cachedInputTokens = 0;
  assert.throws(() => assertWritingPage("claude", unknown, options, committed));
  assert.throws(() =>
    assertWritingPage("codex", page("codex", 4), options, committed),
  );
  assert.throws(() =>
    assertWritingPage("requests", page("requests", 1), options, committed),
  );
});
test("only cross-page publication differences are retryable; exact catchup is required", () => {
  const pages = Object.fromEntries(
    [
      "overview",
      "providers",
      "claude",
      "codex",
      "sessions",
      "projects",
      "requests",
    ].map((name) => [name, page(name, name === "codex" ? 0 : 2)]),
  );
  assert(coherentWritingPages(pages, options, committed).coherent);
  assert(!coherentWritingPages(pages, options, committed, true).coherent);
  pages.requests = page("requests", 4);
  assert(!coherentWritingPages(pages, options, committed).coherent);
  pages.requests.summary.usage.totalTokens--;
  assert.throws(() => coherentWritingPages(pages, options, committed));
});
test("writer rotates agents and files, journals committed bytes and detects corruption", () => {
  const root = mkdtempSync(path.join(tmpdir(), "native-writer-"));
  try {
    const paths = Array.from({ length: 4 }, (_, n) =>
      path.join(root, `${n}.jsonl`),
    );
    paths.forEach((file) => writeFileSync(file, '{"prefix":true}\n'));
    const journal = path.join(root, "ledger.jsonl");
    const writer = historyWriter({
      paths,
      files: 2,
      journal,
      timestamp: "2026-10-24T00:00:00Z",
      readers: () => 8,
    });
    for (let n = 0; n < 8; n++) assert.equal(writer.append(2).activeReaders, 8);
    assert.deepEqual(writer.verify(), {
      claude: 8,
      codex: 8,
      batches: 8,
      modifiedFiles: 4,
    });
    const events = readFileSync(journal, "utf8")
      .trim()
      .split("\n")
      .map(JSON.parse);
    assert.deepEqual(
      events.map((e) => e.agent),
      [
        "claude",
        "codex",
        "claude",
        "codex",
        "claude",
        "codex",
        "claude",
        "codex",
      ],
    );
    assert.equal(events.at(-1).committed.codex, 8);
    const ids = paths
      .flatMap((file) =>
        readFileSync(file, "utf8").trim().split("\n").slice(1).map(JSON.parse),
      )
      .map((row) => row.uuid ?? row.payload.response_id);
    assert.equal(new Set(ids).size, 16);
    writeFileSync(paths[0], "unexpected overwrite");
    assert.throws(() => writer.verify());
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
