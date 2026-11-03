export const DISCOVERY_FIXTURE_ID = "364a8375-9ec2-4c92-9b76-ed6e42294ab6";
export function discoveryFixturePage(
  request: Record<string, unknown>,
  mode: string | null | undefined,
  read = 1,
): Record<string, unknown> {
  const snapshot = (
    mode === "discovery-changed" && read > 1 ? "b" : "a"
  ).repeat(64);
  const kind =
    mode === "discovery-legacy"
      ? "legacy"
      : mode === "discovery-v2"
        ? "scoped_v2"
        : "scoped_v3";
  const count =
    mode === "discovery-many"
      ? 70
      : mode === "discovery-empty" || mode === "discovery-unrecognized"
        ? 0
        : mode === "discovery-bad-row"
          ? 2
          : 1;
  const items = Array.from({ length: count }, (_, index) => ({
    kind,
    operationId:
      mode === "discovery-many"
        ? "00000000-0000-4000-8000-" + index.toString(16).padStart(12, "0")
        : index === 0
          ? DISCOVERY_FIXTURE_ID
          : "464a8375-9ec2-4c92-9b76-ed6e42294ab6",
    digest: "c".repeat(64) as string | null,
    label:
      mode === "discovery-label-changed" && read > 1
        ? "Changed fixture"
        : mode === "discovery-legacy" ||
            mode === "discovery-prepared" ||
            mode === "discovery-committed"
          ? "Legacy fixture"
          : "Current fixture",
    phase: "ready" as string | null,
    legacyStage: (mode === "discovery-legacy"
      ? "unclaimed"
      : mode === "discovery-prepared"
        ? "prepared"
        : mode === "discovery-committed"
          ? "committed"
          : null) as string | null,
    issue: null as string | null,
  }));
  if (mode === "discovery-bad-row")
    Object.assign(items[0], {
      digest: null,
      label: null,
      phase: null,
      legacyStage: null,
      issue: "invalid",
    });
  const domain = kind === "legacy" ? "1" : kind === "scoped_v2" ? "2" : "3";
  const cursor = request.cursor as string | null;
  const start = cursor
    ? items.findIndex(
        (item) => domain + ":" + item.operationId === cursor.slice(65),
      ) + 1
    : 0;
  const page = items.slice(start, start + 32);
  if (
    ["discovery-legacy", "discovery-prepared", "discovery-committed"].includes(
      mode ?? "",
    )
  )
    for (const item of items) item.digest = "a".repeat(64);
  const result: Record<string, unknown> = {
    version: 1,
    storeId: request.storeId,
    namespaceId: request.namespaceId,
    snapshot,
    pageSize: 32,
    totalCandidates: count,
    scanned: page.length,
    merged: 0,
    unrecognized: mode === "discovery-unrecognized" ? 1 : 0,
    items: page,
    nextCursor:
      start + 32 < count
        ? snapshot + ":" + domain + ":" + page.at(-1)!.operationId
        : null,
  };
  if (mode === "discovery-api-foreign")
    result.storeId = "6087597b-0625-42ea-8fd0-e51f6ad16d01";
  if (mode === "discovery-api-future") result.version = 2;
  if (mode === "discovery-api-extra")
    result.secret = "must-not-accept-extra-private-data";
  if (mode === "discovery-api-duplicates") {
    result.items = [items[0], items[0]];
    result.scanned = 2;
    result.totalCandidates = 2;
  }
  if (mode === "discovery-api-counts") result.scanned = 33;
  if (mode === "discovery-api-phase") items[0].phase = "future";
  if (mode === "discovery-api-digest") items[0].digest = "not-a-digest";
  if (mode === "discovery-api-cursor") result.nextCursor = "bad";
  return result;
}
