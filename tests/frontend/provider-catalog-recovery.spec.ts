import { expect, test } from "@playwright/test";
import type {} from "./provider-catalog-recovery-harness";
import {
  storedProviderSubmission,
  selectedProviderCatalog,
} from "../../src/features/providers/catalog-operation";
import { storedProviderRequest } from "../../src/features/providers/operation-receipt";
import { responseProvider } from "./provider-response-fixture";

test("external envelopes stay incompatible with the legacy five-field parser while retaining exact scope", () => {
  const request = {
    storeId: "c9e027fe-4536-4d08-8a55-74e6cbaf6313",
    operationId: "69c2a053-bffc-4d5e-b5eb-c22c7bc2c2d9",
    expectedSequence: 0,
    kind: "save" as const,
    provider: responseProvider,
  };
  const catalog = {
    pluginId: "org.example.catalog",
    digest: "a".repeat(64),
    bindingId: "owned-binding",
    target: {
      kind: "draft" as const,
      label: "owned",
      endpoint: responseProvider.endpoint,
      protocol: responseProvider.protocol,
      vendor: responseProvider.vendor,
      credentialRef: responseProvider.credentialRef,
    },
  };
  const envelope = { version: 2, request, catalog };
  expect(storedProviderRequest(envelope)).toBeNull();
  expect(storedProviderSubmission(envelope)).toMatchObject({
    request,
    catalog,
  });
  for (const bad of [
    { ...envelope, version: 3 },
    { ...envelope, future: true },
    {
      ...envelope,
      catalog: {
        ...catalog,
        target: { ...catalog.target, endpoint: "https://other.invalid" },
      },
    },
    { ...envelope, catalog: { ...catalog, secret: "not-allowed" } },
  ])
    expect(storedProviderSubmission(bad)).toBeNull();
  expect(storedProviderSubmission(request)?.catalog).toBeNull();
});
test("directory recovery supports valid Unicode labels by scalar count", () => {
  const request = {
    storeId: "c9e027fe-4536-4d08-8a55-74e6cbaf6313",
    operationId: "69c2a053-bffc-4d5e-b5eb-c22c7bc2c2d9",
    expectedSequence: 0,
    kind: "save" as const,
    provider: responseProvider,
  };
  const selection = {
    pluginId: "org.example.catalog",
    digest: "a".repeat(64),
    bindingId: "owned-binding",
    target: {
      kind: "draft" as const,
      label: "😀".repeat(100),
      endpoint: responseProvider.endpoint,
      protocol: responseProvider.protocol,
      vendor: responseProvider.vendor,
      credentialRef: responseProvider.credentialRef,
    },
  };
  expect(selectedProviderCatalog(selection, request)).not.toBeNull();
  expect(
    selectedProviderCatalog(
      {
        ...selection,
        target: { ...selection.target, label: "😀".repeat(121) },
      },
      request,
    ),
  ).toBeNull();
});

for (const mode of ["lost", "save-lost"])
  test(`${mode}: actual hook preserves catalog envelope and recovers after reload without native replay`, async ({
    page,
  }) => {
    await page.goto(
      `/tests/frontend/provider-catalog-recovery.html?mode=${mode}`,
    );
    await page
      .getByRole("button", { name: "提交目录原请求", exact: true })
      .click();
    await expect(
      page.getByRole("status").filter({ hasText: "保存结果待核查" }),
    ).toBeVisible();
    const original = await page.evaluate(() => {
      const f = window.catalogRecoveryFixture,
        r = f.records().records[0];
      return {
        request: r.request,
        raw: r.raw,
        catalog: r.catalog,
        backend: f.backend(),
      };
    });
    expect(JSON.parse(original.raw).version).toBe(2);
    expect(Object.keys(original.request!)).toHaveLength(5);
    expect(original.backend.probes).toBe(mode === "lost" ? 1 : 0);
    expect(original.backend.metadata).toBe(1);
    await page.reload();
    await page.evaluate(
      () => (window.catalogRecoveryFixture.failQueries = false),
    );
    await page.getByRole("button", { name: "核查原结果", exact: true }).click();
    await expect(
      page.getByText("原操作已确认保存。", { exact: true }),
    ).toBeVisible();
    const after = await page.evaluate(() => ({
      records: window.catalogRecoveryFixture.records(),
      backend: window.catalogRecoveryFixture.backend(),
      commands: window.catalogRecoveryFixture.commands,
    }));
    expect(after.records.records).toHaveLength(0);
    expect(after.backend.probes).toBe(original.backend.probes);
    expect(after.backend.metadata).toBe(1);
    expect(
      after.commands.some((c) =>
        ["commit_provider_operation", "get_provider_operation"].includes(
          c.name,
        ),
      ),
    ).toBe(false);
    const query = after.commands.find(
      (c) => c.name === "get_catalog_provider_operation",
    )!;
    expect(query.args).toEqual({
      input: { operation: original.request, catalog: original.catalog },
    });
  });

test("pending external request retries use the original choice, cancellation uses the original ledger and finalizes metadata", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-catalog-recovery.html?mode=seed-pending",
  );
  await page.evaluate(() => (window.catalogRecoveryFixture.loseReply = false));
  await page.getByRole("button", { name: "重试原请求", exact: true }).click();
  await expect(
    page.getByText(
      "原连接仍待核查；查询不会重新探测，可再次查询或取消原操作。",
      { exact: true },
    ),
  ).toBeVisible();
  await page.getByRole("button", { name: "取消原操作", exact: true }).click();
  await expect(
    page.getByText("原操作已确认取消。", { exact: true }),
  ).toBeVisible();
  const f = await page.evaluate(() => ({
    records: window.catalogRecoveryFixture.records(),
    backend: window.catalogRecoveryFixture.backend(),
    calls: window.catalogRecoveryFixture.commands.map((c) => c.name),
  }));
  expect(f.backend.probes).toBe(0);
  expect(f.records.records).toHaveLength(0);
  expect(
    f.calls.filter((c) => c === "commit_catalog_provider_operation"),
  ).toHaveLength(1);
  expect(f.calls).toContain("cancel_provider_operation");
  expect(f.calls).toContain("get_catalog_provider_operation");
  expect(f.calls).not.toContain("commit_provider_operation");
});

test("same-turn replaced catalog recovery metadata refuses retry before any backend command", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-catalog-recovery.html?mode=seed-changed",
  );
  const before = await page.evaluate(() => {
    const f = window.catalogRecoveryFixture,
      r = f.records().records[0];
    const next = JSON.parse(r.raw);
    next.catalog.digest = "b".repeat(64);
    localStorage.setItem(r.key, JSON.stringify(next));
    return f.commands.length;
  });
  await page.getByRole("button", { name: "重试原请求", exact: true }).click();
  await expect(
    page.getByText(/原操作的目录选择或恢复记录已变化/),
  ).toBeVisible();
  expect(
    await page.evaluate(() => window.catalogRecoveryFixture.commands.length),
  ).toBe(before);
  expect(
    await page.evaluate(() => window.catalogRecoveryFixture.records().records),
  ).toHaveLength(1);
});

test("a changed backend store rejects external recovery before a catalog mutation", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-catalog-recovery.html?mode=seed-store",
  );
  await page.evaluate(
    () =>
      (window.catalogRecoveryFixture.backendStoreId =
        window.catalogRecoveryFixture.otherStoreId),
  );
  await page.getByRole("button", { name: "核查原结果", exact: true }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "保存结果待核查" }),
  ).toBeVisible();
  const calls = await page.evaluate(() =>
    window.catalogRecoveryFixture.commands.map((c) => c.name),
  );
  expect(calls).toEqual(["get_provider_write_protocol"]);
  expect(
    await page.evaluate(() => window.catalogRecoveryFixture.records().records),
  ).toHaveLength(1);
});

test("backend recovery retires a late catalog lookup without deleting the original envelope", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-catalog-recovery.html?mode=seed-late",
  );
  await page.getByRole("button", { name: "核查原结果", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() => !!window.catalogRecoveryFixture.finishQuery),
    )
    .toBe(true);
  await page.evaluate(() => {
    window.catalogRecoveryFixture.recover();
    window.catalogRecoveryFixture.finishQuery?.();
  });
  await expect(
    page.getByText("后台连接已恢复，原请求仍保留；请核查原结果，未重新提交。", {
      exact: true,
    }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => window.catalogRecoveryFixture.records().records),
  ).toHaveLength(1);
  await expect(
    page.getByText("原操作已确认保存。", { exact: true }),
  ).toHaveCount(0);
});
test("an invalid selection result cannot turn an external submission into the native path", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-catalog-recovery.html?mode=invalid-choice",
  );
  await page
    .getByRole("button", { name: "提交目录原请求", exact: true })
    .click();
  await expect(page.getByRole("alert")).toHaveText(
    "目录选择无法核查，本次未提交，也未改用内置目录。",
  );
  const f = await page.evaluate(() => ({
    calls: window.catalogRecoveryFixture.commands.map((c) => c.name),
    records: window.catalogRecoveryFixture.records().records,
    backend: window.catalogRecoveryFixture.backend(),
  }));
  expect(f.calls.every((c) => c === "get_provider_write_protocol")).toBe(true);
  expect(f.records).toHaveLength(0);
  expect(f.backend.metadata).toBe(0);
  expect(f.backend.probes).toBe(0);
});

test("explicit source envelopes retain builtin versus legacy and reject unknown authority or invalid CAS tokens", () => {
  const request = {
    storeId: "c9e027fe-4536-4d08-8a55-74e6cbaf6313",
    operationId: "69c2a053-bffc-4d5e-b5eb-c22c7bc2c2d9",
    expectedSequence: 0,
    kind: "save",
    provider: responseProvider,
  };
  const route = {
    source: { kind: "builtin" },
    expectedSourceRevision: "b".repeat(64),
  };
  const envelope = { version: 3, request, route };
  expect(storedProviderRequest(envelope)).toBeNull();
  expect(storedProviderSubmission(envelope)?.catalog).toEqual(route);
  expect(storedProviderSubmission(request)?.catalog).toBeNull();
  for (const bad of [
    { ...envelope, future: true },
    { ...envelope, version: 4 },
    {
      ...envelope,
      route: { ...route, expectedSourceRevision: "B".repeat(64) },
    },
    {
      ...envelope,
      route: { ...route, source: { kind: "builtin", catalog: {} } },
    },
    { ...envelope, route: { source: { kind: "builtin" } } },
  ])
    expect(storedProviderSubmission(bad)).toBeNull();
});
for (const mode of ["unified-builtin-lost", "unified-external-lost"])
  test(`${mode}: reload queries exact explicit source without original or v1 catalog replay`, async ({
    page,
  }) => {
    await page.goto(
      `/tests/frontend/provider-catalog-recovery.html?mode=${mode}`,
    );
    await page
      .getByRole("button", { name: "提交目录原请求", exact: true })
      .click();
    await expect(
      page.getByText("保存结果待核查", { exact: true }),
    ).toBeVisible();
    const before = await page.evaluate(
      () => window.catalogRecoveryFixture.records().records[0],
    );
    expect(JSON.parse(before.raw).version).toBe(3);
    await page.reload();
    await page.evaluate(() => {
      window.catalogRecoveryFixture.failQueries = false;
    });
    await page.getByRole("button", { name: "核查原结果", exact: true }).click();
    await expect
      .poll(() =>
        page.evaluate(
          () => window.catalogRecoveryFixture.records().records.length,
        ),
      )
      .toBe(0);
    const commands = await page.evaluate(
      () => window.catalogRecoveryFixture.commands,
    );
    expect(
      commands.filter((c) =>
        [
          "commit_provider_operation",
          "get_provider_operation",
          "commit_catalog_provider_operation",
          "get_catalog_provider_operation",
        ].includes(c.name),
      ),
    ).toHaveLength(0);
    expect(
      commands.find((c) => c.name === "perform_provider_source_operation")
        ?.args,
    ).toEqual({
      input: {
        kind: "query",
        request: { operation: before.request, ...before.catalog },
      },
    });
  });
test("same-turn CAS replacement in version3 recovery cannot switch the operation to another source", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-catalog-recovery.html?mode=seed-unified-changed",
  );
  await page.evaluate(() => {
    const row = window.catalogRecoveryFixture.records().records[0];
    const value = JSON.parse(row.raw);
    value.route.expectedSourceRevision = null;
    localStorage.setItem(row.key, JSON.stringify(value));
    window.catalogRecoveryFixture.commands.length = 0;
  });
  await page.getByRole("button", { name: "重试原请求", exact: true }).click();
  await expect(page.getByText(/恢复记录已变化/)).toBeVisible();
  expect(
    await page.evaluate(() => window.catalogRecoveryFixture.commands),
  ).toHaveLength(0);
});
test("version3 pending recovery retains the same source on retry and uses the original cancellation ledger", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-catalog-recovery.html?mode=seed-unified-pending",
  );
  await page.evaluate(() => {
    window.catalogRecoveryFixture.loseReply = false;
  });
  await page.getByRole("button", { name: "重试原请求", exact: true }).click();
  await expect(page.getByText("保存结果待核查", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "取消原操作", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () => window.catalogRecoveryFixture.records().records.length,
      ),
    )
    .toBe(0);
  const calls = await page.evaluate(() =>
    window.catalogRecoveryFixture.commands.map((c) => c.name),
  );
  expect(calls).toContain("cancel_provider_operation");
  expect(calls).toContain("perform_provider_source_operation");
  expect(calls).not.toContain("commit_provider_operation");
  expect(calls).not.toContain("commit_catalog_provider_operation");
});
