import { expect, test, type Page } from "@playwright/test";
import type { Snapshot } from "../../src/app/types";
import type { NativePage } from "../../src/features/sessions/types";
import type {} from "./harness";

const pageResult = (
  ids: string[],
  nextCursor: string | null = null,
): NativePage<{ id: string }> => ({
  items: ids.map((id) => ({ id })),
  total: ids.length,
  nextCursor,
  summary: null,
  asOf: "2026-10-19T00:00:00Z",
  coverage: "native_history",
  missingCounts: 0,
});
const snapshot = (version: string): Snapshot => ({
  version,
  databasePath: "/test",
  gatewayStatus: "not_configured",
  agents: [],
  providers: [],
  settings: { captureContent: false, retentionDays: 30 },
  sessionSources: [],
  observationBindings: [],
});
const count = (page: Page, expected: number) =>
  expect
    .poll(() => page.evaluate(() => window.hookReads.length))
    .toBe(expected);
async function resolve(
  page: Page,
  index: number,
  result: ReturnType<typeof pageResult> | Snapshot,
) {
  await page.evaluate(
    ({ index, result }) => window.hookReads[index].resolve(result),
    { index, result },
  );
}
test.beforeEach(async ({ page }) => {
  await page.clock.install();
});

test("slow first page completes despite multiple polling intervals", async ({
  page,
}) => {
  await page.goto("/tests/frontend/");
  await count(page, 1);
  await page.clock.fastForward(15_000);
  await count(page, 1);
  await resolve(page, 0, pageResult(["first"], "next"));
  await expect(page.getByTestId("items")).toHaveText("first");
  await expect(page.getByTestId("loading")).toHaveText("false");
});

test("pagination supersedes a poll, deduplicates rows and blocks polling until reload", async ({
  page,
}) => {
  await page.goto("/tests/frontend/");
  await count(page, 1);
  await resolve(page, 0, pageResult(["a"], "next"));
  await page.clock.fastForward(5_000);
  await count(page, 2);
  await page.getByRole("button", { name: "更多" }).click();
  await count(page, 3);
  expect(await page.evaluate(() => window.hookReads[2].cursor)).toBe("next");
  await page.getByRole("button", { name: "更多" }).click();
  await page.clock.fastForward(15_000);
  await count(page, 3);
  await resolve(page, 2, pageResult(["a", "b", "b"]));
  await resolve(page, 1, pageResult(["old poll"]));
  await expect(page.getByTestId("items")).toHaveText("a,b");
  await page.getByRole("button", { name: "重新读取" }).click();
  await count(page, 4);
  await resolve(page, 3, pageResult(["fresh"]));
  await expect(page.getByTestId("items")).toHaveText("fresh");
});

test("failed pagination retains rows and remains retryable", async ({
  page,
}) => {
  await page.goto("/tests/frontend/");
  await count(page, 1);
  await resolve(page, 0, pageResult(["a"], "next"));
  await page.getByRole("button", { name: "更多" }).click();
  await count(page, 2);
  await page.evaluate(() =>
    window.hookReads[1].reject(new Error("read failed")),
  );
  await expect(page.getByTestId("error")).toHaveText("read failed");
  await expect(page.getByTestId("items")).toHaveText("a");
  await page.clock.fastForward(10_000);
  await count(page, 2);
  await page.getByRole("button", { name: "更多" }).click();
  await count(page, 3);
  await resolve(page, 2, pageResult(["b"]));
  await expect(page.getByTestId("items")).toHaveText("a,b");
});

for (const mode of ["native", "resource"])
  test(`${mode}: stale results cannot replace a different filter and unmount clears polling`, async ({
    page,
  }) => {
    await page.goto(`/tests/frontend/?mode=${mode}`);
    await count(page, 1);
    await page.getByRole("button", { name: "切换筛选" }).click();
    await count(page, 2);
    await resolve(page, 1, pageResult(["new"]));
    await resolve(page, 0, pageResult(["stale"]));
    await expect(page.getByTestId("items")).toHaveText("new");
    await page.getByRole("button", { name: "切换挂载" }).click();
    await page.clock.fastForward(30_000);
    await count(page, 2);
  });

test("snapshot polling coalesces reads and explicit refresh waits for a new read after an older snapshot", async ({
  page,
}) => {
  await page.goto("/tests/frontend/?mode=snapshot");
  await count(page, 1);
  await page.clock.fastForward(15_000);
  await count(page, 1);
  await page.getByRole("button", { name: "刷新", exact: true }).click();
  await expect(page.getByTestId("completed")).toHaveText("false");
  await count(page, 1);
  await resolve(page, 0, snapshot("before-save"));
  await count(page, 2);
  await page.clock.runFor(100);
  await expect(page.getByTestId("snapshot")).toHaveText("empty");
  await expect(page.getByTestId("completed")).toHaveText("false");
  await resolve(page, 1, snapshot("fresh"));
  await expect(page.getByTestId("snapshot")).toHaveText("fresh");
  await expect(page.getByTestId("completed")).toHaveText("true");
});

test("StrictMode snapshot skips unstarted work and ignores the previous mount's reply", async ({
  page,
}) => {
  await page.goto("/tests/frontend/?mode=snapshot&strict");
  await count(page, 1);
  await page.getByRole("button", { name: "切换挂载" }).click();
  await page.getByRole("button", { name: "切换挂载" }).click();
  await count(page, 2);
  await resolve(page, 1, snapshot("new mount"));
  await resolve(page, 0, snapshot("old mount"));
  await expect(page.getByTestId("snapshot")).toHaveText("new mount");
  await page.clock.fastForward(5_000);
  await count(page, 3);
  await page.getByRole("button", { name: "切换挂载" }).click();
  await page.evaluate(() =>
    window.hookReads[2].reject(new Error("late error")),
  );
  await page.clock.fastForward(20_000);
  await count(page, 3);
});

test("snapshot background refresh keeps content and loading quiet; explicit refresh still reports progress", async ({
  page,
}) => {
  await page.goto("/tests/frontend/?mode=snapshot");
  await count(page, 1);
  await expect(page.getByTestId("loading")).toHaveText("true");
  await resolve(page, 0, snapshot("saved"));
  await expect(page.getByTestId("loading")).toHaveText("false");
  await page.clock.fastForward(5000);
  await count(page, 2);
  await expect(page.getByTestId("loading")).toHaveText("false");
  await expect(page.getByTestId("snapshot")).toHaveText("saved");
  await page.evaluate(() => window.hookReads[1].reject(Error("offline")));
  await expect(page.getByTestId("snapshot")).toHaveText("saved");
  await expect(page.getByTestId("error")).toHaveText("offline");
  await page.getByRole("button", { name: "刷新", exact: true }).click();
  await count(page, 3);
  await expect(page.getByTestId("loading")).toHaveText("true");
  await resolve(page, 2, snapshot("recovered"));
  await expect(page.getByTestId("loading")).toHaveText("false");
  await expect(page.getByTestId("error")).toBeEmpty();
});

for (const mode of ["native", "resource"])
  test(`${mode}: background reads keep content and errors until success, while manual reload remains visible`, async ({
    page,
  }) => {
    await page.goto(`/tests/frontend/?mode=${mode}`);
    await count(page, 1);
    await resolve(page, 0, pageResult(["saved"]));
    await expect(page.getByTestId("loading")).toHaveText("false");
    const interval = mode === "native" ? 5000 : 10000;
    await page.clock.fastForward(interval);
    await count(page, 2);
    await expect(page.getByTestId("loading")).toHaveText("false");
    await expect(page.getByTestId("items")).toHaveText("saved");
    await page.evaluate(() => window.hookReads[1].reject(Error("offline")));
    await expect(page.getByTestId("error")).toHaveText("offline");
    await page.clock.fastForward(interval);
    await count(page, 3);
    await expect(page.getByTestId("error")).toHaveText("offline");
    await expect(page.getByTestId("loading")).toHaveText("false");
    await resolve(page, 2, pageResult(["recovered"]));
    await expect(page.getByTestId("error")).toBeEmpty();
    await page.getByRole("button", { name: "重新读取", exact: true }).click();
    await count(page, 4);
    await expect(page.getByTestId("loading")).toHaveText("true");
    await expect(page.getByTestId("items")).toHaveText("recovered");
    await resolve(page, 3, pageResult(["manual"]));
    await expect(page.getByTestId("loading")).toHaveText("false");
  });

test("browser entry keeps native operations unavailable across lazy routes", async ({
  page,
}) => {
  await page.goto("/");
  await expect(page.getByRole("alert")).toHaveText(
    "请在 Local Agent 桌面应用中连接本机数据。",
  );
  for (const hash of [
    "agents",
    "providers",
    "projects",
    "mcp",
    "sessions",
    "requests",
    "settings",
  ]) {
    await page.goto(`/#/${hash}`);
    await expect(page.locator("main")).toBeVisible();
    await expect(page.getByRole("alert")).toHaveText(
      "请在 Local Agent 桌面应用中连接本机数据。",
    );
    await expect(
      page.getByRole("button", { name: "重新读取", exact: true }),
    ).toBeEnabled();
  }
});

test("resource reload and polls share one pending read without discarding its result", async ({
  page,
}) => {
  await page.goto("/tests/frontend/?mode=resource");
  await count(page, 1);
  await page
    .getByRole("button", { name: "重新读取", exact: true })
    .click({ clickCount: 3 });
  await page.clock.fastForward(30_000);
  await count(page, 1);
  await resolve(page, 0, pageResult(["shared read"]));
  await expect(page.getByTestId("items")).toHaveText("shared read");
  await expect(page.getByTestId("loading")).toHaveText("false");
});

for (const mode of ["native", "resource"])
  test(`${mode}: mutation refreshes wait for an old read and share the fresh result`, async ({
    page,
  }) => {
    await page.goto(`/tests/frontend/?mode=${mode}`);
    await count(page, 1);
    await page
      .getByRole("button", { name: "变更后刷新", exact: true })
      .click({ clickCount: 2 });
    await count(page, 1);
    await resolve(page, 0, pageResult(["before mutation"]));
    await count(page, 2);
    await page.clock.runFor(100);
    await expect(page.getByTestId("items")).toHaveText("empty");
    await expect(page.getByTestId("completed")).toHaveText("0");
    await resolve(page, 1, pageResult(["after mutation"]));
    await expect(page.getByTestId("items")).toHaveText("after mutation");
    await expect(page.getByTestId("completed")).toHaveText("2");
    await count(page, 2);
  });

for (const mode of ["native", "resource"])
  test(`a later mutation while a fresh ${mode} read is running requires another read`, async ({
    page,
  }) => {
    await page.goto(`/tests/frontend/?mode=${mode}`);
    await count(page, 1);
    await page.getByRole("button", { name: "变更后刷新", exact: true }).click();
    await resolve(page, 0, pageResult(["before mutation A"]));
    await count(page, 2);
    await page.getByRole("button", { name: "变更后刷新", exact: true }).click();
    await resolve(page, 1, pageResult(["before mutation B"]));
    await count(page, 3);
    await page.clock.runFor(100);
    await expect(page.getByTestId("items")).toHaveText("empty");
    await expect(page.getByTestId("completed")).toHaveText("1");
    await resolve(page, 2, pageResult(["after mutation B"]));
    await expect(page.getByTestId("completed")).toHaveText("2");
    await expect(page.getByTestId("items")).toHaveText("after mutation B");
  });

for (const mode of ["native", "resource", "snapshot"])
  for (const outcome of ["result", "error"])
    test(`${mode}: mutation suppresses an old ${outcome} and preserves the last confirmed data`, async ({
      page,
    }) => {
      const isSnapshot = mode === "snapshot";
      const value = (text: string) =>
        isSnapshot ? snapshot(text) : pageResult([text]);
      const field = isSnapshot ? "snapshot" : "items";
      await page.goto(`/tests/frontend/?mode=${mode}`);
      await count(page, 1);
      await resolve(page, 0, value("last confirmed"));
      await expect(page.getByTestId(field)).toHaveText("last confirmed");
      await page.clock.fastForward(mode === "resource" ? 10000 : 5000);
      await count(page, 2);
      await page
        .getByRole("button", {
          name: isSnapshot ? "刷新" : "变更后刷新",
          exact: true,
        })
        .click();
      await expect(page.getByTestId("loading")).toHaveText("true");
      if (outcome === "result") await resolve(page, 1, value("obsolete"));
      else
        await page.evaluate(() =>
          window.hookReads[1].reject(Error("obsolete error")),
        );
      await count(page, 3);
      await page.clock.runFor(100);
      await expect(page.getByTestId(field)).toHaveText("last confirmed");
      await expect(page.getByTestId("error")).toBeEmpty();
      await expect(page.getByTestId("loading")).toHaveText("true");
      await resolve(page, 2, value("after mutation"));
      await expect(page.getByTestId(field)).toHaveText("after mutation");
      await expect(page.getByTestId("loading")).toHaveText("false");
    });

test("snapshot invalidation during its replacement read requires a later snapshot", async ({
  page,
}) => {
  await page.goto("/tests/frontend/?mode=snapshot");
  await count(page, 1);
  await resolve(page, 0, snapshot("confirmed"));
  await expect(page.getByTestId("snapshot")).toHaveText("confirmed");
  await page.getByRole("button", { name: "刷新", exact: true }).click();
  await count(page, 2);
  await page.getByRole("button", { name: "刷新", exact: true }).click();
  await resolve(page, 1, snapshot("before second mutation"));
  await count(page, 3);
  await page.clock.runFor(100);
  await expect(page.getByTestId("snapshot")).toHaveText("confirmed");
  await expect(page.getByTestId("loading")).toHaveText("true");
  await resolve(page, 2, snapshot("after second mutation"));
  await expect(page.getByTestId("snapshot")).toHaveText(
    "after second mutation",
  );
});

test("native invalidation blocks stale pagination until the new first page completes", async ({
  page,
}) => {
  await page.goto("/tests/frontend/?mode=native");
  await count(page, 1);
  await resolve(page, 0, pageResult(["old first"], "old cursor"));
  await expect(page.getByTestId("items")).toHaveText("old first");
  await page.getByRole("button", { name: "更多", exact: true }).click();
  await count(page, 2);
  await page.getByRole("button", { name: "变更后刷新", exact: true }).click();
  await page.getByRole("button", { name: "更多", exact: true }).click();
  await count(page, 2);
  await resolve(page, 1, pageResult(["obsolete second"], "obsolete cursor"));
  await count(page, 3);
  await page.getByRole("button", { name: "更多", exact: true }).click();
  await count(page, 3);
  await page.clock.runFor(100);
  await expect(page.getByTestId("items")).toHaveText("old first");
  await resolve(page, 2, pageResult(["new first"], "new cursor"));
  await expect(page.getByTestId("items")).toHaveText("new first");
  await page.getByRole("button", { name: "更多", exact: true }).click();
  await count(page, 4);
  expect(await page.evaluate(() => window.hookReads[3].cursor)).toBe(
    "new cursor",
  );
  await resolve(page, 3, pageResult(["new second"]));
  await expect(page.getByTestId("items")).toHaveText("new first,new second");
});

for (const mode of ["native", "resource"])
  test(`${mode}: a mutation callback captured before a filter change cannot restart its old scope`, async ({
    page,
  }) => {
    await page.goto(`/tests/frontend/?mode=${mode}`);
    await count(page, 1);
    await resolve(page, 0, pageResult(["old scope"]));
    await page
      .getByRole("button", { name: "保留变更回调", exact: true })
      .click();
    await page.getByRole("button", { name: "切换筛选", exact: true }).click();
    await count(page, 2);
    await resolve(page, 1, pageResult(["new scope"]));
    await expect(page.getByTestId("items")).toHaveText("new scope");
    await page.evaluate(() => window.hookAfterMutation?.());
    await page.clock.runFor(100);
    await count(page, 2);
    await expect(page.getByTestId("items")).toHaveText("new scope");
  });

for (const mode of ["native", "snapshot"])
  test(`${mode}: a synchronous loader error releases its pending slot and can be retried`, async ({
    page,
  }) => {
    const isSnapshot = mode === "snapshot";
    const value = (text: string) =>
      isSnapshot ? snapshot(text) : pageResult([text]);
    const field = isSnapshot ? "snapshot" : "items";
    const button = isSnapshot ? "刷新" : "重新读取";
    await page.goto(`/tests/frontend/?mode=${mode}`);
    await count(page, 1);
    await resolve(page, 0, value("saved"));
    await expect(page.getByTestId(field)).toHaveText("saved");
    await page.evaluate(() => {
      window.hookThrows = true;
    });
    await page.getByRole("button", { name: button, exact: true }).click();
    await expect(page.getByTestId("error")).toHaveText(
      "synchronous loader failure",
    );
    await expect(page.getByTestId("loading")).toHaveText("false");
    await expect(page.getByTestId(field)).toHaveText("saved");
    await page.evaluate(() => {
      window.hookThrows = false;
    });
    await page.getByRole("button", { name: button, exact: true }).click();
    await count(page, 2);
    await resolve(page, 1, value("retried"));
    await expect(page.getByTestId(field)).toHaveText("retried");
  });

for (const mode of ["native", "resource"])
  test(`${mode}: an invalidation waiting on an old key cannot read or publish to the new lifecycle`, async ({
    page,
  }) => {
    await page.goto(`/tests/frontend/?mode=${mode}`);
    await count(page, 1);
    await page.getByRole("button", { name: "变更后刷新", exact: true }).click();
    await page.getByRole("button", { name: "切换筛选", exact: true }).click();
    await count(page, 2);
    await resolve(page, 0, pageResult(["old key"]));
    await resolve(page, 1, pageResult(["new key"]));
    await expect(page.getByTestId("items")).toHaveText("new key");
    await expect(page.getByTestId("completed")).toHaveText("1");
    await count(page, 2);
  });

test("a synchronous resource loader error releases the read and remains retryable", async ({
  page,
}) => {
  await page.goto("/tests/frontend/?mode=resource");
  await count(page, 1);
  await resolve(page, 0, pageResult(["saved"]));
  await page.evaluate(() => {
    window.hookThrows = true;
  });
  await page.getByRole("button", { name: "重新读取", exact: true }).click();
  await expect(page.getByTestId("error")).toHaveText(
    "synchronous loader failure",
  );
  await expect(page.getByTestId("items")).toHaveText("saved");
  await page.evaluate(() => {
    window.hookThrows = false;
  });
  await page.getByRole("button", { name: "重新读取", exact: true }).click();
  await count(page, 2);
  await resolve(page, 1, pageResult(["recovered"]));
  await expect(page.getByTestId("items")).toHaveText("recovered");
});

test("StrictMode resource cleanup skips unstarted work and ignores late replies after unmount", async ({
  page,
}) => {
  await page.goto("/tests/frontend/?mode=resource&strict");
  await count(page, 1);
  await page.getByRole("button", { name: "重新读取", exact: true }).click();
  await count(page, 1);
  await page.getByRole("button", { name: "切换挂载", exact: true }).click();
  await page.evaluate(() =>
    window.hookReads[0].reject(new Error("late error")),
  );
  await page.clock.fastForward(30_000);
  await count(page, 1);
  await expect(page.getByTestId("items")).toHaveCount(0);
});

test("bounded native pages replace bodies and revisit an opaque earlier cursor", async ({
  page,
}) => {
  await page.goto("/tests/frontend/?mode=native&retention=page");
  await resolve(page, 0, {
    ...pageResult(["a"], "opaque-[]:next"),
    total: 10000,
  });
  await page.getByRole("button", { name: "更多", exact: true }).click();
  await count(page, 2);
  expect(await page.evaluate(() => window.hookReads[1].cursor)).toBe(
    "opaque-[]:next",
  );
  await resolve(page, 1, {
    ...pageResult(["b", "b"], "next again"),
    total: 10000,
  });
  await expect(page.getByTestId("items")).toHaveText("b");
  await expect(page.getByTestId("trail")).toHaveText("1");
  await page.getByRole("button", { name: "上一页", exact: true }).click();
  await count(page, 3);
  expect(await page.evaluate(() => window.hookReads[2].cursor)).toBeUndefined();
  await resolve(page, 2, pageResult(["a"], "opaque-[]:next"));
  await expect(page.getByTestId("items")).toHaveText("a");
  await expect(page.getByTestId("trail")).toHaveText("0");
});

test("bounded failed forward and backward reads keep the committed page and retry the same destination", async ({
  page,
}) => {
  await page.goto("/tests/frontend/?mode=native&retention=page");
  await resolve(page, 0, pageResult(["a"], "next"));
  await page.getByRole("button", { name: "更多", exact: true }).click();
  await count(page, 2);
  await page.evaluate(() =>
    window.hookReads[1].reject(Error("disk unavailable")),
  );
  await expect(page.getByTestId("error")).toHaveText("disk unavailable");
  await expect(page.getByTestId("items")).toHaveText("a");
  await expect(page.getByTestId("trail")).toHaveText("0");
  await page.getByRole("button", { name: "更多", exact: true }).click();
  await count(page, 3);
  await resolve(page, 2, pageResult(["b"]));
  await expect(page.getByTestId("trail")).toHaveText("1");
  await page.getByRole("button", { name: "上一页", exact: true }).click();
  await count(page, 4);
  await page.evaluate(() =>
    window.hookReads[3].reject(Error("disk unavailable")),
  );
  await expect(page.getByTestId("loading")).toHaveText("false");
  await expect(page.getByTestId("items")).toHaveText("b");
  await expect(page.getByTestId("trail")).toHaveText("1");
  await page.getByRole("button", { name: "上一页", exact: true }).click();
  await count(page, 5);
  await resolve(page, 4, pageResult(["a"], "next"));
  await expect(page.getByTestId("items")).toHaveText("a");
  await expect(page.getByTestId("trail")).toHaveText("0");
});

test("bounded pagination supersedes a slow first-page poll and polling resumes only after returning", async ({
  page,
}) => {
  await page.goto("/tests/frontend/?mode=native&retention=page");
  await resolve(page, 0, pageResult(["a"], "next"));
  await page.clock.fastForward(5000);
  await count(page, 2);
  await page.getByRole("button", { name: "更多", exact: true }).click();
  await count(page, 3);
  await resolve(page, 2, pageResult(["b"]));
  await resolve(page, 1, pageResult(["old poll"], "old cursor"));
  await expect(page.getByTestId("items")).toHaveText("b");
  await expect(page.getByTestId("trail")).toHaveText("1");
  await page.clock.fastForward(15000);
  await count(page, 3);
  await page.getByRole("button", { name: "上一页", exact: true }).click();
  await count(page, 4);
  await resolve(page, 3, pageResult(["fresh first"], "next"));
  await page.clock.fastForward(5000);
  await count(page, 5);
});

test("a recovery hides old bounded titles before its still-live read completes", async ({
  page,
}) => {
  await page.goto("/tests/frontend/?mode=native&retention=page");
  await resolve(page, 0, pageResult(["old first"], "old next"));
  await page.getByRole("button", { name: "更多", exact: true }).click();
  await count(page, 2);
  await page.evaluate(() => window.hookRecover());
  await expect(page.getByTestId("items")).toHaveText("empty");
  await count(page, 2);
  await resolve(page, 1, pageResult(["late old next"], "old again"));
  await count(page, 3);
  await resolve(page, 2, pageResult(["new first"], "new next"));
  await expect(page.getByTestId("items")).toHaveText("new first");
  await expect(page.getByTestId("trail")).toHaveText("0");
});

test("a callback captured before changing native retention mode cannot restart the old scope", async ({
  page,
}) => {
  await page.goto("/tests/frontend/?mode=native&retention=page");
  await resolve(page, 0, pageResult(["page mode"]));
  await page.getByRole("button", { name: "保留变更回调", exact: true }).click();
  await page.getByRole("button", { name: "切换保留方式", exact: true }).click();
  await count(page, 2);
  await page.evaluate(() => window.hookAfterMutation?.());
  await count(page, 2);
  await resolve(page, 1, pageResult(["append mode"]));
  await expect(page.getByTestId("items")).toHaveText("append mode");
});

test("bounded title history retains at most 100 cursors without retaining earlier rows", async ({
  page,
}) => {
  test.setTimeout(90000);
  await page.goto("/tests/frontend/?mode=native&retention=page");
  await resolve(page, 0, pageResult(["page-0"], "cursor-1"));
  for (let n = 1; n <= 102; n++) {
    await page.getByRole("button", { name: "更多", exact: true }).click();
    await count(page, n + 1);
    await resolve(page, n, pageResult([`page-${n}`], `cursor-${n + 1}`));
    await expect(page.getByTestId("items")).toHaveText(`page-${n}`);
    await expect(page.getByTestId("retained")).toHaveText("1");
    await expect(page.getByTestId("trail")).toHaveText(
      String(Math.min(n, 100)),
    );
  }
  await page.getByRole("button", { name: "重新读取", exact: true }).click();
  await count(page, 104);
  await resolve(page, 103, pageResult(["first again"]));
  await expect(page.getByTestId("trail")).toHaveText("0");
});

test("native page actions captured before recovery cannot send old cursors or reset the new source", async ({
  page,
}) => {
  await page.goto("/tests/frontend/?mode=native&retention=page");
  await resolve(page, 0, pageResult(["old first"], "old next"));
  await page.getByRole("button", { name: "更多", exact: true }).click();
  await count(page, 2);
  await resolve(page, 1, pageResult(["old second"], "old third"));
  await expect(page.getByTestId("items")).toHaveText("old second");
  await page.getByRole("button", { name: "保留本页动作", exact: true }).click();
  await page.evaluate(() => window.hookRecover());
  await count(page, 3);
  await resolve(page, 2, pageResult(["new first"], "new next"));
  await expect(page.getByTestId("items")).toHaveText("new first");
  await page.evaluate(() => {
    const actions = window.hookOldActions!;
    void actions.next();
    void actions.previous();
    void actions.reload();
    void actions.invalidate();
  });
  await page.clock.runFor(100);
  await count(page, 3);
  await expect(page.getByTestId("items")).toHaveText("new first");
  await expect(page.getByTestId("trail")).toHaveText("0");
});
