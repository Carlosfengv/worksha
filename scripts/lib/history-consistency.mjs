import assert from "node:assert/strict";

// Compare model calls, not the number of sessions. Unknown usage and costs remain
// part of the full summaries, which are additionally compared across lifecycles.
export function assertHistoryConsistency(
  pages,
  { files, calls, gatewayCalls },
) {
  const expected = calls * files * 2;
  const row = (count, tokens) => ({ calls: count, tokens });
  const native = {
    overview: row(
      pages.overview.analysis.samples,
      pages.overview.analysis.total,
    ),
    providers: row(
      pages.providers.selected.calls,
      pages.providers.selected.usage.totalTokens,
    ),
    models: row(
      pages.claude.summary.attempts + pages.codex.summary.attempts,
      pages.claude.summary.usage.totalTokens +
        pages.codex.summary.usage.totalTokens,
    ),
    sessions: row(
      pages.sessions.summary.modelCalls,
      pages.sessions.summary.usage.totalTokens,
    ),
    projects: row(
      pages.projects.summary.modelCalls,
      pages.projects.summary.usage.totalTokens,
    ),
    requests: row(
      pages.requests.total,
      pages.requests.summary.usage.totalTokens,
    ),
  };
  assert.equal(
    pages.sessions.total,
    files * 2,
    "Session count is independent of model calls",
  );
  for (const [name, value] of Object.entries(native))
    assert.deepEqual(value, row(expected, expected * 12), name);
  for (const agent of ["claude", "codex"]) {
    assert.equal(pages[agent].summary.attempts, files * calls, agent);
    assert.equal(
      pages[agent].summary.usage.totalTokens,
      files * calls * 12,
      agent,
    );
  }
  const gateway = {
    overview: row(
      pages.gatewayOverview.analysis.samples,
      pages.gatewayOverview.analysis.total,
    ),
    providers: row(
      pages.gatewayProviders.selected.calls,
      pages.gatewayProviders.selected.usage.totalTokens,
    ),
    models: row(
      pages.gatewayModels.summary.attempts,
      pages.gatewayModels.summary.usage.totalTokens,
    ),
    requests: row(
      pages.gatewayRequests.total,
      pages.gatewayRequests.summary.usage.totalTokens,
    ),
  };
  for (const [name, value] of Object.entries(gateway))
    assert.deepEqual(
      value,
      row(gatewayCalls, gatewayCalls * 12),
      `gateway.${name}`,
    );
  return { native, gateway };
}

export function stableStatistics(pages) {
  const result = {};
  for (const [name, value] of Object.entries(pages)) {
    result[name] = {};
    for (const field of [
      "summary",
      "selected",
      "models",
      "providers",
      "unattributed",
      "total",
      "totalCalls",
      "modelOptions",
      "missingTime",
      "missingCounts",
      "associatedCosts",
    ])
      if (value[field] !== undefined)
        result[name][field] = structuredClone(value[field]);
    // Progress is live metadata. Fee amounts, unknowns and attribution counters
    // remain in the fingerprint; unhealthy metadata must still fail explicitly.
    const fees =
      name === "sessions"
        ? result[name].associatedCosts
        : name === "projects"
          ? result[name].summary?.associatedCosts
          : undefined;
    if (fees) {
      assert(
        ["ready", "partial"].includes(fees.sourceState),
        "Unexpected fee source state",
      );
      delete fees.sourceState;
    }
    if (name === "overview" || name === "gatewayOverview")
      result[name].analysis = {
        total: value.analysis.total,
        samples: value.analysis.samples,
      };
  }
  return result;
}
