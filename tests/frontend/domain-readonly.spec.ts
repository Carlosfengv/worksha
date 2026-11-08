import { test, expect } from "@playwright/test";

test("native default never calls the unified reader and decimal parsing preserves precision", async ({
  page,
}) => {
  await page.goto("/tests/frontend/domain-readonly.html");
  await expect(page.getByText("原生会话保持")).toBeVisible();
  expect(await page.evaluate(() => window.domainFixture.calls)).toEqual([]);
  expect(
    await page.evaluate(() => window.domainFixture.contractCheck()),
  ).toEqual([true, true, false, false, false]);
});

test("native scope comes from mounted providers and supports multiple non-Claude sources", async ({
  page,
}) => {
  await page.goto("/tests/frontend/domain-readonly.html?unified=1&builtins=1");
  await page.locator("summary").filter({ hasText: "选择来源" }).click();
  await page.getByRole("checkbox", { name: /Reader · 已授权/ }).uncheck();
  await page.getByRole("checkbox", { name: /OpenCode · 待授权/ }).check();
  await expect
    .poll(async () =>
      page.evaluate(() => window.domainFixture.queries.at(-1)?.scope),
    )
    .toEqual({ kind: "builtin", sourceIds: ["opencode"] });
  await expect(page.getByText("当前范围尚无可读会话")).toBeVisible();
  await page.getByRole("checkbox", { name: /Codex CLI.*待授权/ }).check();
  await expect
    .poll(async () =>
      page.evaluate(() => window.domainFixture.queries.at(-1)?.scope),
    )
    .toEqual({ kind: "builtin", sourceIds: ["codex", "opencode"] });
  await page.getByRole("checkbox", { name: /Reader · 已授权/ }).check();
  await expect
    .poll(async () =>
      page.evaluate(() => window.domainFixture.queries.at(-1)?.scope),
    )
    .toEqual({
      kind: "combined",
      sourceIds: ["codex", "opencode", "org.example.reader:source"],
    });
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter(
      (c) => c.startsWith("body:") || c.startsWith("consent:"),
    ),
  ).toEqual([]);
});
test("unified metadata does not read body until the user selects a session", async ({
  page,
}) => {
  await page.goto("/tests/frontend/domain-readonly.html?unified=1");
  await expect(page.getByRole("cell", { name: /Alpha/ })).toBeVisible();
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter((c) =>
      c.startsWith("body:"),
    ),
  ).toEqual([]);
  await expect(page.getByText("选择会话后按需读取正文。")).toBeVisible();
  await page.getByRole("row").filter({ hasText: "Alpha" }).focus();
  await page.getByRole("row").filter({ hasText: "Alpha" }).press("Enter");
  await expect(
    page.getByText("Alpha private body", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText(/9,007,199,254,741,000 Token/).first(),
  ).toBeVisible();
  await page.getByLabel("搜索统一会话").fill("Beta");
  await expect(page.getByText("选择会话后按需读取正文。")).toBeVisible();
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter((c) =>
      c.startsWith("body:"),
    ),
  ).toEqual(["body:org.example.reader:Alpha"]);
  await page.getByRole("row").filter({ hasText: "Beta" }).click();
  await expect(
    page.getByText("Beta private body", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("Alpha private body", { exact: true }),
  ).toHaveCount(0);
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).every(
      (c) => c === "catalog" || c === "sessions" || c.startsWith("body:"),
    ),
  ).toBe(true);
});
test("late body results cannot replace another selected session", async ({
  page,
}) => {
  await page.goto("/tests/frontend/domain-readonly.html?unified=1&delay=1");
  await expect(page.getByRole("cell", { name: /Beta/ })).toBeVisible();
  await page.getByRole("row").filter({ hasText: "Alpha" }).click();
  await expect
    .poll(async () =>
      (await page.evaluate(() => window.domainFixture.calls)).includes(
        "body:org.example.reader:Alpha",
      ),
    )
    .toBe(true);
  await page.getByRole("row").filter({ hasText: "Beta" }).click();
  await expect(
    page.getByText("Beta private body", { exact: true }),
  ).toBeVisible();
  await page.evaluate(() => window.domainFixture.release());
  await expect(
    page.getByText("Alpha private body", { exact: true }),
  ).toHaveCount(0);
});

test("retry obtains a new snapshot and retains a session beyond the first page", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/domain-readonly.html?unified=1&many=1&staleBody=1",
  );
  await page.getByRole("button", { name: "下一页会话" }).click();
  await page.getByRole("row").filter({ hasText: "Paged-040" }).click();
  await expect(page.getByText("快照已失效，请重新读取。")).toBeVisible();
  await page.getByRole("button", { name: "重试读取正文" }).click();
  await expect(page.getByRole("heading", { name: "Paged-040" })).toBeVisible();
  await expect(page.getByText("Paged-040 private body")).toBeVisible();
  const evidence = await page.evaluate(() => ({
    calls: window.domainFixture.calls,
    queries: window.domainFixture.queries,
  }));
  expect(evidence.calls.filter((call) => call.startsWith("snapshot:"))).toEqual(
    ["snapshot:fixture-snapshot-1", "snapshot:fixture-snapshot-2"],
  );
  expect(evidence.queries.at(-1)).toMatchObject({
    snapshotToken: null,
    cursor: null,
  });
});

test("retry does not reuse session details when the refreshed scope is revoked", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/domain-readonly.html?unified=1&staleBody=revoke",
  );
  await page.getByRole("row").filter({ hasText: "Alpha" }).click();
  await page.getByRole("button", { name: "重试读取正文" }).click();
  await expect(
    page.getByText("当前快照中无法读取原会话", { exact: false }),
  ).toBeVisible();
  await expect(page.getByRole("heading", { name: "Alpha" })).toHaveCount(0);
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter((call) =>
      call.startsWith("snapshot:"),
    ),
  ).toEqual(["snapshot:fixture-snapshot-1"]);
});
test("withdrawing unified authorization clears visible rows and body immediately", async ({
  page,
}) => {
  await page.goto("/tests/frontend/domain-readonly.html?unified=1");
  await page.getByRole("row").filter({ hasText: "Alpha" }).click();
  await expect(
    page.getByText("Alpha private body", { exact: true }),
  ).toBeVisible();
  await page.evaluate(() => window.domainFixture.revoke());
  await expect(
    page.getByText("Alpha private body", { exact: true }),
  ).toHaveCount(0);
  await expect(page.getByText("当前范围尚无可读会话")).toBeVisible();
});
test("view authorization and optional metadata storage require an explicit action", async ({
  page,
}) => {
  await page.goto("/tests/frontend/domain-readonly.html?unapproved=1");
  await page.getByRole("button", { name: "统一会话展示", exact: true }).click();
  await page
    .getByRole("checkbox", { name: "保存元数据供重开读取", exact: true })
    .check();
  await page.getByRole("button", { name: "确认加入统一展示" }).click();
  await expect(page.getByText("已加入统一展示 · 保存元数据")).toBeVisible();
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter((c) =>
      c.startsWith("consent:"),
    ),
  ).toEqual(["consent:true:true"]);
  await page.getByRole("button", { name: "停止统一展示" }).click();
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter((c) =>
      c.startsWith("consent:"),
    ),
  ).toEqual(["consent:true:true", "consent:false:true"]);
});

test("minimum window keeps the list and body readable with independent scrolling", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/domain-readonly.html?unified=1");
  await page.getByRole("row").filter({ hasText: "Alpha" }).click();
  await expect(
    page.getByText("Alpha private body", { exact: true }),
  ).toBeVisible();
  const body = await page
    .getByLabel("统一正文滚动区域", { exact: true })
    .boundingBox();
  expect(body?.height).toBeGreaterThanOrEqual(80);
  await expect(page.getByRole("cell", { name: /Alpha/ })).toBeInViewport();
  await expect(
    page.getByText("Alpha private body", { exact: true }),
  ).toBeInViewport();
  const summary = page.locator("summary").filter({ hasText: "选择来源" });
  await summary.focus();
  await summary.press("Enter");
  await expect(
    page.getByRole("checkbox", { name: /Reader · 已授权/ }),
  ).toBeVisible();
  await summary.press("Enter");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});

test("a stale visible resource review cannot grant access or silently resubmit", async ({
  page,
}) => {
  await page.goto("/tests/frontend/domain-readonly.html?unapproved=1");
  await page.getByRole("button", { name: "统一会话展示", exact: true }).click();
  await expect(
    page.getByText("来源文件：/temporary/reader.jsonl", { exact: true }),
  ).toBeVisible();
  await page.evaluate(() => window.domainFixture.changeResource());
  await page.getByRole("button", { name: "确认加入统一展示" }).click();
  await expect(page.getByRole("alert")).toHaveText(
    "展示范围已变化，请重新核查",
  );
  await expect(
    page.getByRole("button", { name: "确认加入统一展示" }),
  ).toBeDisabled();
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter((c) =>
      c.startsWith("attempt:"),
    ),
  ).toEqual(["attempt:review-1"]);
  await page.getByRole("button", { name: "重新核查展示范围" }).click();
  await expect(
    page.getByText("来源文件：/temporary/changed-reader.jsonl", {
      exact: true,
    }),
  ).toBeVisible();
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter((c) =>
      c.startsWith("consent:"),
    ),
  ).toEqual([]);
  await page.getByRole("button", { name: "确认加入统一展示" }).click();
  await expect(
    page.getByText("已加入统一展示 · 仅本次运行", { exact: true }),
  ).toBeVisible();
});

test("server pages retain the same snapshot and full-scope totals across back navigation", async ({
  page,
}) => {
  await page.goto("/tests/frontend/domain-readonly.html?unified=1&many=1");
  await expect(page.getByRole("cell", { name: /Paged-000/ })).toBeVisible();
  await expect(page.getByText(/83 个会话/)).toBeVisible();
  await page.getByRole("button", { name: "下一页会话", exact: true }).click();
  await expect(page.getByRole("cell", { name: /Paged-040/ })).toBeVisible();
  await expect(page.getByText(/83 个会话/)).toBeVisible();
  await page.getByRole("button", { name: "下一页会话", exact: true }).click();
  await expect(page.getByRole("cell", { name: /Paged-080/ })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "下一页会话", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "上一页会话", exact: true }).click();
  await expect(page.getByRole("cell", { name: /Paged-040/ })).toBeVisible();
  expect(
    (await page.evaluate(() => window.domainFixture.calls)).filter((c) =>
      c.startsWith("body:"),
    ),
  ).toEqual([]);
});
