import { expect, test, type Page } from "@playwright/test";
import type {} from "./provider-editor-source-harness";
const fetched = (page: Page) => page.getByText(/已获取 1 个模型/);
const read = (page: Page) =>
  page.getByRole("button", { name: "获取模型", exact: true });
async function open(page: Page) {
  await page.goto("/tests/frontend/provider-editor-source.html?mode=saved");
  await page
    .getByRole("checkbox", {
      name: "允许所选目录访问本机或私有网络地址",
      exact: true,
    })
    .check();
  await page.getByRole("checkbox", { name: /同意为本次连接授权/ }).check();
  await page.getByRole("button", { name: "高级设置", exact: true }).click();
  await expect(read(page)).toBeEnabled();
}
async function populated(page: Page) {
  await open(page);
  await read(page).click();
  await expect(fetched(page)).toBeVisible();
}
const result = (page: Page) =>
  page.evaluate(() => ({
    requests: window.editorSourceFixture.draftRequests,
    checks: window.editorSourceFixture.draftChecks,
    reads: window.editorSourceFixture.reads,
    native: window.editorSourceFixture.nativeReads,
    legacy: window.editorSourceFixture.legacyReads,
    commands: window.editorSourceFixture.operations?.commands,
  }));
test("save cannot merge fetched IDs retired while its initial source check was pending", async ({
  page,
}) => {
  await populated(page);
  await page.evaluate(() => {
    window.editorSourceFixture.retireOnProtocol = true;
  });
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const saved = await page.evaluate(() => window.editorSourceFixture.saves[0]);
  expect(saved.models).toEqual(["owned-model"]);
  expect(saved.modelCatalog).toBeNull();
});
test("unsubmitted Key text masks freshness without discarding an authorized old scope, and remote retirement prevents restoration", async ({
  page,
}) => {
  await populated(page);
  const key = page.getByRole("textbox", { name: "API Key", exact: true });
  await key.fill("owned-unsubmitted-key");
  await expect(fetched(page)).toHaveCount(0);
  await key.fill("");
  await expect(fetched(page)).toBeVisible();
  await key.fill("owned-unsubmitted-key");
  await page.evaluate(() => {
    window.editorSourceFixture.token = "c".repeat(64);
  });
  await expect(page.getByText(/原目录来源已变化/)).toBeVisible();
  await key.fill("");
  await expect(fetched(page)).toHaveCount(0);
  expect((await result(page)).requests).toHaveLength(1);
});
test("actual editor and API carry the original Saved token with edited Draft endpoint and exact grant; own bind does not cancel its read", async ({
  page,
}) => {
  await open(page);
  await page
    .getByRole("textbox", { name: "Endpoint", exact: true })
    .fill("http://127.0.0.1:11435/edited/v1");
  await read(page).click();
  await expect(fetched(page)).toBeVisible();
  const r = await result(page);
  expect(r.requests).toHaveLength(1);
  expect(r.requests[0]).toMatchObject({
    origin: {
      kind: "saved",
      sourceRevision: "b".repeat(64),
      query: { expectedRevision: "1" },
    },
    draft: { endpoint: "http://127.0.0.1:11435/edited/v1" },
    source: {
      kind: "external",
      catalog: {
        target: { kind: "draft", endpoint: "http://127.0.0.1:11435/edited/v1" },
      },
    },
  });
  expect(r.legacy).toBe(0);
  expect(r.native).toBe(0);
  expect(
    r.commands?.some((c) => c.name === "perform_provider_draft_catalog"),
  ).toBe(true);
});
test("remote origin changes before reading refuse the captured source with no legacy native fallback", async ({
  page,
}) => {
  await open(page);
  await page.evaluate(() => {
    window.editorSourceFixture.token = "c".repeat(64);
  });
  await read(page).click();
  await expect(page.getByText(/原目录来源已变化/)).toBeVisible();
  await expect(fetched(page)).toHaveCount(0);
  const r = await result(page);
  expect(r.reads).toBe(0);
  expect(r.native).toBe(0);
  expect(r.legacy).toBe(0);
});
for (const phase of ["read", "check"] as const)
  test(`${phase}: a remote source change before a held reply cannot publish its directory`, async ({
    page,
  }) => {
    await open(page);
    await page.evaluate((phase) => {
      const f = window.editorSourceFixture;
      if (phase === "read") f.holdRead = true;
      else f.holdCheck = true;
    }, phase);
    await read(page).click();
    await expect
      .poll(() => page.evaluate(() => !!window.editorSourceFixture.release))
      .toBe(true);
    await page.evaluate(() => {
      const f = window.editorSourceFixture;
      f.token = "c".repeat(64);
      f.release?.();
    });
    await expect(read(page)).toBeEnabled();
    await expect(fetched(page)).toHaveCount(0);
    expect((await result(page)).legacy).toBe(0);
  });
for (const change of ["remote", "local", "scope"] as const)
  test(`${change}: displayed data retires while registered models remain and no directory refresh occurs`, async ({
    page,
  }) => {
    await populated(page);
    if (change === "scope") {
      const input = page.getByRole("textbox", {
        name: "Endpoint",
        exact: true,
      });
      const before = await input.inputValue();
      await input.fill("http://127.0.0.1:11435/new");
      await expect(fetched(page)).toHaveCount(0);
      await input.fill(before);
    } else
      await page.evaluate((change) => {
        const f = window.editorSourceFixture;
        if (change === "remote") f.token = "c".repeat(64);
        else f.invalidate();
      }, change);
    await expect(fetched(page)).toHaveCount(0);
    await expect(
      page.getByRole("combobox", { name: "默认模型", exact: true }),
    ).toHaveValue("owned-model");
    expect((await result(page)).requests).toHaveLength(1);
  });
test("a stalled metadata check expires on the monotonic clock; a late reply cannot restore the cache", async ({
  page,
}) => {
  await page.clock.install();
  await populated(page);
  await page.evaluate(() => {
    window.editorSourceFixture.holdCheck = true;
  });
  await page.clock.fastForward(1100);
  await expect
    .poll(() => page.evaluate(() => !!window.editorSourceFixture.release))
    .toBe(true);
  await page.clock.setSystemTime(new Date("2000-01-01T00:00:00Z"));
  await page.clock.fastForward(5500);
  await expect(fetched(page)).toHaveCount(0);
  await page.evaluate(() => {
    const f = window.editorSourceFixture;
    f.holdCheck = false;
    f.release?.();
  });
  await page.clock.fastForward(1100);
  await expect(fetched(page)).toHaveCount(0);
  expect((await result(page)).requests).toHaveLength(1);
});
test("a hide-and-return cycle fences an already admitted read", async ({
  page,
}) => {
  await open(page);
  await page.evaluate(() => {
    window.editorSourceFixture.holdRead = true;
  });
  await read(page).click();
  await expect
    .poll(() => page.evaluate(() => !!window.editorSourceFixture.release))
    .toBe(true);
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "hidden",
    });
    document.dispatchEvent(new Event("visibilitychange"));
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "visible",
    });
    document.dispatchEvent(new Event("visibilitychange"));
    const f = window.editorSourceFixture;
    f.holdRead = false;
    f.release?.();
  });
  await expect(read(page)).toBeEnabled();
  await expect(fetched(page)).toHaveCount(0);
  await read(page).click();
  await expect(fetched(page)).toBeVisible();
});
test("a malformed metadata reply is not authority and never causes a fallback", async ({
  page,
}) => {
  await open(page);
  await page.evaluate(() => {
    window.editorSourceFixture.badCheckReply = true;
  });
  await read(page).click();
  await expect(read(page)).toBeEnabled();
  await expect(fetched(page)).toHaveCount(0);
  expect((await result(page)).legacy).toBe(0);
});
