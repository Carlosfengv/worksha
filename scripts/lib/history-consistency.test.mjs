import assert from "node:assert/strict";
import { test } from "node:test";
import {
  mkdtempSync,
  mkdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { diskSample, percentile } from "./capacity-evidence.mjs";
import {
  assertHistoryConsistency,
  stableStatistics,
} from "./history-consistency.mjs";

function fixture() {
  const usage = (n) => ({ totalTokens: n * 12, reasoningTokens: null });
  const summary = (n) => ({
    attempts: n,
    modelCalls: n,
    usage: usage(n),
    cost: { unknownCalls: n, actual: [] },
  });
  return {
    overview: { analysis: { samples: 630, total: 7560 } },
    providers: { selected: { calls: 630, usage: usage(630) } },
    claude: { summary: summary(315) },
    codex: { summary: summary(315) },
    sessions: { total: 90, summary: summary(630) },
    projects: { summary: summary(630) },
    requests: { total: 630, summary: summary(630) },
    gatewayOverview: { analysis: { samples: 85, total: 1020 } },
    gatewayProviders: { selected: { calls: 85, usage: usage(85) } },
    gatewayModels: { summary: summary(85) },
    gatewayRequests: { total: 85, summary: summary(85) },
  };
}
const expected = { files: 45, calls: 7, gatewayCalls: 85 };
test("session count is not treated as model-call count", () => {
  assert.equal(
    assertHistoryConsistency(fixture(), expected).native.sessions.calls,
    630,
  );
});
for (const field of [
  "providers",
  "claude",
  "sessions",
  "projects",
  "requests",
  "gatewayModels",
]) {
  test(`reject ${field} count or usage regression`, () => {
    const pages = fixture();
    if (field === "providers") pages.providers.selected.usage.totalTokens--;
    else pages[field].summary.usage.totalTokens--;
    assert.throws(() => assertHistoryConsistency(pages, expected));
  });
}
test("lifecycle fingerprint retains unknown costs and unknown usage", () => {
  const before = stableStatistics(fixture());
  const pages = fixture();
  pages.claude.summary.cost.unknownCalls = 0;
  pages.codex.summary.usage.reasoningTokens = 0;
  assert.notDeepEqual(stableStatistics(pages), before);
});
test("pagination and runtime timestamps do not change aggregate fingerprint", () => {
  const pages = fixture(),
    before = stableStatistics(pages);
  pages.claude.nextCursor = "other-snapshot";
  pages.claude.asOf = "later";
  assert.deepEqual(stableStatistics(pages), before);
});
test("live fee progress preserves numeric comparison and rejects unhealthy states", () => {
  const pages = fixture();
  pages.sessions.associatedCosts = {
    sourceState: "ready",
    cost: { unknownCalls: 630 },
    nativeResponses: 630,
  };
  pages.projects.summary.associatedCosts = structuredClone(
    pages.sessions.associatedCosts,
  );
  const before = stableStatistics(pages);
  pages.sessions.associatedCosts.sourceState = "partial";
  pages.projects.summary.associatedCosts.sourceState = "partial";
  assert.deepEqual(stableStatistics(pages), before);
  assert.equal(pages.sessions.associatedCosts.sourceState, "partial");
  pages.projects.summary.associatedCosts.cost.unknownCalls--;
  assert.notDeepEqual(stableStatistics(pages), before);
  pages.sessions.associatedCosts.sourceState = "unavailable";
  assert.throws(() => stableStatistics(pages));
});
test("disk sampling measures nested fixtures without traversing links", () => {
  const root = mkdtempSync(path.join(tmpdir(), "capacity-sampling-"));
  try {
    mkdirSync(path.join(root, "nested"));
    writeFileSync(path.join(root, "nested", "data"), "12345");
    symlinkSync(root, path.join(root, "self"));
    const sample = diskSample(root);
    assert.equal(sample.logicalBytes, 5);
    assert(sample.allocatedBytes > 0 && sample.volumeAvailableBytes > 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
test("five-sample P95 reports the measured maximum without interpolation", () => {
  const values = [5, 1, 4, 2, 3];
  assert.equal(percentile(values, 50), 3);
  assert.equal(percentile(values, 95), 5);
  assert.deepEqual(values, [5, 1, 4, 2, 3]);
});
