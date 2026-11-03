import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";

// The writer owns only generated fixtures. Its append ledger and byte hashes are
// independent of every service response, including cached and frozen pages.
export function historyWriter({ paths, files, journal, timestamp, readers }) {
  const committed = { claude: 0, codex: 0 };
  const hashes = new Map();
  const original = paths.map((file) =>
    createHash("sha256").update(readFileSync(file)).digest("hex"),
  );
  // Each run starts its own journal even when the chosen report path is reused.
  writeFileSync(journal, "");
  let sequence = 0;
  return {
    committed,
    append(batch) {
      const agent = sequence % 2 === 0 ? "claude" : "codex";
      const file = Math.floor(sequence / 2) % files;
      const target = paths[file + (agent === "codex" ? files : 0)];
      const batchTimestamp =
        typeof timestamp === "function" ? timestamp() : timestamp;
      const rows = Array.from({ length: batch }, (_, ordinal) => {
        const response = `continuous-${sequence}-${ordinal}`;
        return agent === "claude"
          ? {
              type: "assistant",
              sessionId: `soak-${file}`,
              uuid: response,
              timestamp: batchTimestamp,
              message: {
                id: response,
                model: "soak-0",
                content: [{ type: "text", text: "offline-append-sentinel" }],
                usage: { input_tokens: 20, output_tokens: 3 },
              },
            }
          : {
              type: "token_usage_record",
              timestamp: batchTimestamp,
              payload: {
                response_id: response,
                usage: { input_tokens: 20, output_tokens: 3 },
              },
            };
      });
      const bytes = rows.map((row) => JSON.stringify(row)).join("\n") + "\n";
      if (!hashes.has(target))
        hashes.set(target, createHash("sha256").update(readFileSync(target)));
      appendFileSync(target, bytes);
      hashes.get(target).update(bytes);
      committed[agent] += batch;
      const event = {
        sequence: sequence++,
        agent,
        file,
        calls: batch,
        tokens: batch * 23,
        activeReaders: readers(),
        committed: { ...committed },
        at: new Date().toISOString(),
      };
      appendFileSync(journal, JSON.stringify(event) + "\n");
      return event;
    },
    verify() {
      paths.forEach((file, index) =>
        assert.equal(
          createHash("sha256").update(readFileSync(file)).digest("hex"),
          hashes.get(file)?.copy().digest("hex") ?? original[index],
          `Unexpected fixture write: ${file}`,
        ),
      );
      return { ...committed, batches: sequence, modifiedFiles: hashes.size };
    },
  };
}

function observed(name, page) {
  if (name === "overview") return [page.analysis.samples, page.analysis.total];
  const totals = name === "providers" ? page.selected : page.summary;
  return [
    name === "providers"
      ? totals.calls
      : name === "requests"
        ? page.total
        : ["claude", "codex"].includes(name)
          ? totals.attempts
          : totals.modelCalls,
    totals.usage.totalTokens,
  ];
}

export function assertWritingPage(
  name,
  page,
  { files, calls, batch },
  committed,
) {
  if (name.startsWith("gateway")) return undefined;
  const scoped = ["claude", "codex"].includes(name);
  const base = files * calls * (scoped ? 1 : 2);
  const upper = scoped ? committed[name] : committed.claude + committed.codex;
  const [count, tokens] = observed(name, page);
  const extra = count - base;
  assert(
    Number.isSafeInteger(extra) && extra >= 0 && extra <= upper,
    `${name}: calls outside the committed append ledger: ${count}`,
  );
  assert.equal(extra % batch, 0, `${name}: partial append batch`);
  assert.equal(
    tokens,
    base * 12 + extra * 23,
    `${name}: mixed count / token generations`,
  );
  const totals = name === "providers" ? page.selected : page.summary;
  if (totals) {
    assert.equal(
      totals.usage.inputTokens,
      base * 10 + extra * 20,
      `${name}: input`,
    );
    assert.equal(
      totals.usage.outputTokens,
      base * 2 + extra * 3,
      `${name}: output`,
    );
    for (const key of [
      "cachedInputTokens",
      "cacheWriteInputTokens",
      "reasoningOutputTokens",
    ])
      assert.equal(totals.usage[key], null, `${name}: unknown ${key}`);
    if (totals.cost)
      assert.deepEqual(
        totals.cost,
        { actual: [], estimated: [], unknownCalls: count },
        `${name}: unknown fees`,
      );
    if (totals.reported)
      for (const key of ["inputTokens", "outputTokens", "totalTokens"])
        assert.equal(totals.reported[key], count, `${name}: reported ${key}`);
  }
  if (name === "providers") {
    assert(["ready", "partial"].includes(page.sourceState));
    assert.deepEqual(page.summary, page.selected, "Provider full scope");
    assert.equal(page.totalCalls, count);
    assert.equal(
      page.models.reduce((n, m) => n + m.totals.calls, 0),
      count,
    );
    assert.equal(
      page.models.reduce((n, m) => n + m.totals.usage.totalTokens, 0),
      tokens,
    );
    assert.equal(totals.unknownOutcomes, count);
    assert.equal(totals.errors, 0);
    assert.equal(totals.inProgress, 0);
    for (const key of ["latencyP95Ms", "ttftP95Ms"])
      assert.equal(totals[key], null);
    for (const key of ["latencySamples", "ttftSamples"])
      assert.equal(totals[key], 0);
  }
  if (name === "requests") {
    assert.equal(page.summary.requests, count);
    assert.equal(page.summary.usageMissing, 0);
    assert.equal(page.summary.attempts, 0);
    assert.deepEqual(page.summary.statuses, { unknown: count });
    for (const key of ["ttft", "duration", "throughput"])
      assert.deepEqual(page.summary[key], { samples: 0, p50: null, p95: null });
    assert.deepEqual(
      page.sourceReport.native,
      page.summary,
      "Request source report / page",
    );
    for (const key of [
      "matchedNative",
      "ambiguousNative",
      "usageConflicts",
      "nativeMissingTime",
    ])
      assert.equal(page.sourceReport[key], 0, `Request ${key}`);
    assert.equal(page.sourceReport.unlinkedNative, count);
  }
  if (scoped) {
    assert(
      ["ready", "partial", "scanning"].includes(page.sourceState),
      `${name}: unexpected source state ${page.sourceState}`,
    );
    assert.equal(totals.missingUsage, 0);
  }
  if (name === "sessions") assert.equal(page.total, files * 2);
  const fees =
    name === "sessions"
      ? page.associatedCosts
      : name === "projects"
        ? totals.associatedCosts
        : undefined;
  if (["sessions", "projects"].includes(name)) {
    assert(fees, `${name}: fee evidence is missing`);
    assert(["ready", "partial"].includes(fees.sourceState));
    for (const key of ["nativeResponses", "unlinkedNativeResponses"])
      assert.equal(fees[key], count, `${name}: ${key}`);
    for (const key of [
      "gatewayAttempts",
      "matchedNativeResponses",
      "conflictingNativeResponses",
      "responseLinkedAttempts",
      "sessionLinkedAttempts",
    ])
      assert.equal(fees[key], 0, `${name}: ${key}`);
    assert.deepEqual(fees.cost, { actual: [], estimated: [], unknownCalls: 0 });
  }
  const agents = {};
  const modelRows =
    name === "providers" ? page.models : scoped ? page.items : [];
  for (const item of modelRows) {
    const model = item.model;
    const agent = model === "soak-codex" ? "codex" : "claude";
    const ordinal =
      agent === "codex" ? null : Number(model?.replace(/^soak-/, ""));
    assert(
      agent === "codex" ||
        (Number.isInteger(ordinal) &&
          ordinal >= 0 &&
          ordinal < Math.min(37, calls)),
      "Unexpected native model",
    );
    const modelBase =
      files *
      (agent === "codex"
        ? calls
        : Math.floor(calls / 37) + Number(ordinal < calls % 37));
    const modelCount = item.totals.calls ?? item.totals.attempts;
    const added = modelCount - modelBase;
    assert(
      added >= 0 && added <= committed[agent] && added % batch === 0,
      "Model count outside the append ledger",
    );
    if (agent === "claude" && ordinal !== 0)
      assert.equal(added, 0, "Untouched model changed");
    assert.equal(
      item.totals.usage.totalTokens,
      modelBase * 12 + added * 23,
      "Model mixed generation",
    );
    assert.equal(item.totals.usage.inputTokens, modelBase * 10 + added * 20);
    assert.equal(item.totals.usage.outputTokens, modelBase * 2 + added * 3);
    assert.deepEqual(item.totals.cost, {
      actual: [],
      estimated: [],
      unknownCalls: modelCount,
    });
    if (agent === "codex" || ordinal === 0) agents[agent] = added;
  }
  return { calls: count, tokens, extra, agents };
}

// A cohort can span publication boundaries. Individual pages must always pass
// the ledger first; only cross-page generation differences are retryable.
export function coherentWritingPages(pages, options, committed, exact = false) {
  const native = Object.fromEntries(
    Object.entries(pages)
      .filter(([name]) => !name.startsWith("gateway"))
      .map(([name, page]) => [
        name,
        assertWritingPage(name, page, options, committed),
      ]),
  );
  const totalExtra = native.claude.extra + native.codex.extra;
  const same = Object.entries(native).every(
    ([name, value]) =>
      ["claude", "codex"].includes(name) || value.extra === totalExtra,
  );
  return {
    native,
    coherent:
      same &&
      native.providers.agents.claude === native.claude.extra &&
      native.providers.agents.codex === native.codex.extra &&
      (!exact ||
        (native.claude.extra === committed.claude &&
          native.codex.extra === committed.codex)),
  };
}
