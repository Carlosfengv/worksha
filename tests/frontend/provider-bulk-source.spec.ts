import { expect, test, type Page } from "@playwright/test";
import type {} from "./provider-bulk-source-harness";
const external = (page: Page) =>
  page.getByText("external-a-candidate", { exact: true });
const read = (page: Page) =>
  page.getByRole("button", { name: "读取全部模型目录", exact: true });
async function open(page: Page) {
  await page.goto("/tests/frontend/provider-bulk-source.html");
  await expect(read(page)).toBeEnabled();
}
async function populated(page: Page) {
  await open(page);
  await read(page).click();
  await expect(external(page)).toBeVisible();
  await expect(
    page.getByText("native-b-candidate", { exact: true }),
  ).toBeVisible();
}
const state = (page: Page) =>
  page.evaluate(() => ({
    reads: window.bulkSourceFixture.reads,
    states: window.bulkSourceFixture.states,
    legacy: window.bulkSourceFixture.legacyReads,
    calls: window.bulkSourceFixture.calls,
  }));
test("actual inventory reads remembered external and builtin sources only through the scoped source IPC", async ({
  page,
}) => {
  await populated(page);
  const result = await state(page);
  expect(result.reads.map((r) => r.kind)).toEqual(["external", "builtin"]);
  expect(result.legacy).toBe(0);
  expect(
    result.calls.every((c) =>
      [
        "get_provider_write_protocol",
        "perform_provider_catalog_source",
      ].includes(c.name),
    ),
  ).toBe(true);
  expect(result.reads[0].token).toBe("b".repeat(64));
});
for (const kind of [
  "source",
  "catalog",
  "scoped",
  "write",
  "native-write",
  "plugin",
  "full-plugin",
])
  test(`${kind}: an uncertain mutation retires disclosed results before its reply and never silently restores them`, async ({
    page,
  }) => {
    await populated(page);
    await page.evaluate((kind) => {
      void window.bulkSourceFixture.mutate(kind);
    }, kind);
    await expect
      .poll(() => page.evaluate(() => window.bulkSourceFixture.mutationStarted))
      .toBe(true);
    await expect(external(page)).toHaveCount(0);
    await expect(page.getByText("registered-a", { exact: true })).toBeVisible();
    await page.evaluate(() => window.bulkSourceFixture.releaseMutation?.());
    await expect
      .poll(() => page.evaluate(() => window.bulkSourceFixture.error))
      .not.toBe("");
    await expect(external(page)).toHaveCount(0);
    expect((await state(page)).reads).toHaveLength(2);
  });
for (const change of ["token", "binding", "revoked", "unreadable"] as const)
  test(`${change}: metadata monitoring retires cached remote data without another directory request`, async ({
    page,
  }) => {
    await populated(page);
    await page.evaluate((change) => {
      const f = window.bulkSourceFixture;
      if (change === "token") f.state.sourceRevision = "c".repeat(64);
      else if (change === "binding")
        f.state.selection!.bindingId = "replacement-grant";
      else if (change === "revoked") {
        f.state.authorized = false;
        f.state.selection = null;
        f.state.errorCode = "grant_revoked";
      } else f.failState = true;
    }, change);
    await expect(external(page)).toHaveCount(0);
    await expect(page.getByText("registered-a", { exact: true })).toBeVisible();
    expect((await state(page)).reads).toHaveLength(2);
    expect((await state(page)).legacy).toBe(0);
  });
test("a stalled metadata check expires disclosure and its late reply cannot resurrect the old snapshot", async ({
  page,
}) => {
  await page.clock.install();
  await populated(page);
  await page.evaluate(() => {
    window.bulkSourceFixture.holdState = true;
  });
  await page.clock.fastForward(1100);
  await expect
    .poll(() => page.evaluate(() => !!window.bulkSourceFixture.releaseState))
    .toBe(true);
  await page.clock.fastForward(5500);
  await expect(external(page)).toHaveCount(0);
  await page.evaluate(() => {
    window.bulkSourceFixture.holdState = false;
    window.bulkSourceFixture.releaseState?.();
  });
  await page.clock.fastForward(1100);
  await expect(external(page)).toHaveCount(0);
  expect((await state(page)).reads).toHaveLength(2);
});
test("source change while a directory reply is pending is checked before publication; the other provider still reads independently", async ({
  page,
}) => {
  await open(page);
  await page.evaluate(() => {
    window.bulkSourceFixture.holdRead = true;
  });
  await read(page).click();
  await expect
    .poll(() => page.evaluate(() => !!window.bulkSourceFixture.releaseRead))
    .toBe(true);
  await page.evaluate(() => {
    window.bulkSourceFixture.state.sourceRevision = "c".repeat(64);
    window.bulkSourceFixture.releaseRead?.();
  });
  await expect(read(page)).toBeEnabled();
  await expect(external(page)).toHaveCount(0);
  await expect(
    page.getByText("native-b-candidate", { exact: true }),
  ).toBeVisible();
  await read(page).click();
  await expect(
    page.getByText("external-new-candidate", { exact: true }),
  ).toBeVisible();
  expect((await state(page)).legacy).toBe(0);
});
test("unavailable or unauthorized source metadata never calls a native directory fallback", async ({
  page,
}) => {
  await open(page);
  await page.evaluate(() => {
    window.bulkSourceFixture.failState = true;
  });
  await read(page).click();
  await expect(page.getByText(/目录来源尚不可核查/)).toBeVisible();
  expect((await state(page)).reads.map((r) => r.kind)).toEqual(["builtin"]);
  await page.evaluate(() => {
    const f = window.bulkSourceFixture;
    f.failState = false;
    f.state.authorized = false;
    f.state.selection = null;
    f.state.errorCode = "grant_revoked";
  });
  await read(page).click();
  await expect(page.getByText(/请在连接配置中核对并授权/)).toBeVisible();
  expect((await state(page)).reads).toHaveLength(2);
  expect((await state(page)).legacy).toBe(0);
});
test("an ordinary directory failure preserves a prior snapshot only while the same source remains authorized", async ({
  page,
}) => {
  await populated(page);
  await page.evaluate(() => {
    window.bulkSourceFixture.failRead = true;
  });
  await read(page).click();
  await expect(page.getByText(/目录上游暂时不可用/)).toBeVisible();
  await expect(external(page)).toBeVisible();
  await page.evaluate(() => {
    const f = window.bulkSourceFixture;
    f.state.authorized = false;
    f.state.selection = null;
    f.state.errorCode = "grant_revoked";
  });
  await read(page).click();
  await expect(external(page)).toHaveCount(0);
  await expect(page.getByText("registered-a", { exact: true })).toBeVisible();
});
for (const change of ["hidden", "inactive", "authority"] as const)
  test(`${change}: a late batch cannot continue or publish after its disclosure scope was retired`, async ({
    page,
  }) => {
    await open(page);
    await page.evaluate(() => {
      window.bulkSourceFixture.holdRead = true;
    });
    await read(page).click();
    await expect
      .poll(() => page.evaluate(() => !!window.bulkSourceFixture.releaseRead))
      .toBe(true);
    await page.evaluate((change) => {
      const f = window.bulkSourceFixture;
      if (change === "hidden") {
        f.hide = true;
        document.dispatchEvent(new Event("visibilitychange"));
      } else if (change === "inactive") f.active(false);
      else f.invalidate();
    }, change);
    if (change === "inactive") await expect(read(page)).toBeDisabled();
    await page.evaluate((change) => {
      const f = window.bulkSourceFixture;
      if (change === "hidden") {
        f.hide = false;
        document.dispatchEvent(new Event("visibilitychange"));
      } else if (change === "inactive") f.active(true);
      f.releaseRead?.();
    }, change);
    await expect(read(page)).toBeEnabled();
    await expect(external(page)).toHaveCount(0);
    expect((await state(page)).reads).toHaveLength(1);
    await read(page).click();
    await expect(external(page)).toBeVisible();
  });

test("changing the wall clock cannot extend a stalled disclosure lease", async ({
  page,
}) => {
  await page.clock.install();
  await populated(page);
  await page.evaluate(() => {
    window.bulkSourceFixture.holdState = true;
  });
  await page.clock.setSystemTime(new Date("2000-01-01T00:00:00Z"));
  await page.clock.fastForward(6100);
  await expect(external(page)).toHaveCount(0);
  expect((await state(page)).reads).toHaveLength(2);
});
