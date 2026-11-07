import { expect, test, type Page } from "@playwright/test";
import type {} from "./local-token-harness";

async function open(page: Page) {
  await page.goto("/tests/frontend/local-token.html");
  await expect(
    page.getByRole("button", { name: "准备入口", exact: true }),
  ).toBeVisible();
}
const snapshot = (page: Page) =>
  page.evaluate(() => window.localTokenFixture.snapshot());
const panel = (page: Page) =>
  page.getByRole("region", { name: "本机入口结果核查", exact: true });
for (const purpose of [
  "fixed_route",
  "dynamic_binding",
  "model_service",
] as const) {
  test(`${purpose}: lost replies retain one original identity; query then confirmed continuation reuses it`, async ({
    page,
  }) => {
    await open(page);
    await page.evaluate(
      (purpose) => window.localTokenFixture.submit(purpose),
      purpose,
    );
    const before = await snapshot(page);
    expect(before.recovery.hints).toHaveLength(1);
    expect(before.operation?.request.purpose).toBe(purpose);
    await panel(page)
      .getByRole("button", { name: "查询原入口结果", exact: true })
      .click();
    const resume = panel(page).getByRole("button", {
      name: "继续原入口",
      exact: true,
    });
    await expect(resume).toBeDisabled();
    await panel(page)
      .getByRole("checkbox", { name: "已核对原入口来源", exact: true })
      .check();
    await resume.click();
    await expect(page.getByTestId("adopted")).toHaveText(
      before.identity.operationId,
    );
    const after = await snapshot(page);
    expect(after.recovery.hints).toHaveLength(0);
    expect(after.preview?.id).toBe(before.identity.operationId);
    expect(
      after.calls.filter((call) => call.name.startsWith("prepare_")),
    ).toHaveLength(1);
    expect(
      after.calls.filter(
        (call) => call.name === "continue_local_token_operation",
      ),
    ).toHaveLength(1);
    expect(JSON.stringify(after.recovery)).not.toContain(providerEndpoint);
  });
}
const providerEndpoint = "http://127.0.0.1:32123";

for (const purpose of [
  "fixed_route",
  "dynamic_binding",
  "model_service",
] as const) {
  test(`${purpose}: confirmed pre-admission cancellation blocks late requests without inventing an owner`, async ({
    page,
  }) => {
    await open(page);
    await page.evaluate(async (purpose) => {
      window.localTokenFixture.setMode("before-admission");
      await window.localTokenFixture.submit(purpose);
    }, purpose);
    expect((await snapshot(page)).operation).toBeNull();
    await panel(page)
      .getByRole("button", { name: "停止这次创建或预览", exact: true })
      .click();
    await expect(panel(page).getByText(/原编号已阻止后续创建/)).toBeVisible();
    const state = await snapshot(page);
    expect(state.recovery.hints).toHaveLength(0);
    expect(state.cancellation?.identity).toEqual(state.identity);
    expect(state.operation).toBeNull();
    const late = await page.evaluate(
      (purpose) => window.localTokenFixture.submit(purpose),
      purpose,
    );
    expect(late).toContain("原编号已阻止后续创建");
    expect((await snapshot(page)).operation).toBeNull();
    await panel(page)
      .getByRole("button", { name: "查询原入口结果", exact: true })
      .click();
    await expect
      .poll(async () => (await snapshot(page)).recovery.hints.length)
      .toBe(0);
  });
}
test("a lost cancellation reply retains its hint until a separate query finds the durable proof", async ({
  page,
}) => {
  await open(page);
  await page.evaluate(async () => {
    window.localTokenFixture.setMode("before-admission");
    await window.localTokenFixture.submit("dynamic_binding");
    window.localTokenFixture.setMode("cancel-lost");
  });
  await panel(page)
    .getByRole("button", { name: "停止这次创建或预览", exact: true })
    .click();
  await expect(panel(page).getByText(/停止回复丢失/)).toBeVisible();
  expect((await snapshot(page)).recovery.hints).toHaveLength(1);
  await panel(page)
    .getByRole("button", { name: "查询原入口结果", exact: true })
    .click();
  await expect(panel(page).getByText(/原编号已阻止后续创建/)).toBeVisible();
  expect((await snapshot(page)).recovery.hints).toHaveLength(0);
  expect(
    (await snapshot(page)).calls.filter((value) =>
      value.name.startsWith("prepare_"),
    ),
  ).toHaveLength(1);
});
test("a cancellation proof from another namespace cannot clear the original hint", async ({
  page,
}) => {
  await open(page);
  await page.evaluate(async () => {
    window.localTokenFixture.setMode("before-admission");
    await window.localTokenFixture.submit("fixed_route");
    window.localTokenFixture.setMode("cancel-foreign");
  });
  await panel(page)
    .getByRole("button", { name: "停止这次创建或预览", exact: true })
    .click();
  await expect(panel(page).getByText(/回复或来源不可核查/)).toBeVisible();
  expect((await snapshot(page)).recovery.hints).toHaveLength(1);
});
for (const kind of ["reconnect", "unmount"] as const) {
  test(`a late cancellation reply after ${kind} preserves the original hint`, async ({
    page,
  }) => {
    await open(page);
    await page.evaluate(async () => {
      window.localTokenFixture.setMode("before-admission");
      await window.localTokenFixture.submit("fixed_route");
      window.localTokenFixture.setMode("cancel-hold");
    });
    await panel(page)
      .getByRole("button", { name: "停止这次创建或预览", exact: true })
      .click();
    await expect
      .poll(
        async () =>
          (await snapshot(page)).calls.filter(
            (value) => value.name === "cancel_local_token_operation",
          ).length,
      )
      .toBe(1);
    await page.evaluate((kind) => {
      window.localTokenFixture[kind]();
      window.localTokenFixture.release();
    }, kind);
    expect((await snapshot(page)).recovery.hints).toHaveLength(1);
  });
}
test("cancellation replies reject old versions, missing proof, contradictory work and weak capabilities", async ({
  page,
}) => {
  await open(page);
  const results = await page.evaluate(() => {
    const { identity, protocol } = window.localTokenFixture.snapshot();
    const valid = {
      version: 2,
      identity,
      operation: null,
      credential: null,
      preview: null,
      cancellation: { version: 1, identity },
      canContinue: false,
      sourceMessage: "原编号后续创建已阻止",
    };
    return {
      replies: [
        valid,
        { ...valid, version: 1 },
        { ...valid, cancellation: null },
        { ...valid, cancellation: { version: 2, identity } },
        { ...valid, canContinue: true },
        {
          ...valid,
          cancellation: {
            version: 1,
            identity: { ...identity, operationId: crypto.randomUUID() },
          },
        },
      ].map((value) => window.localTokenFixture.resultValid(value)),
      protocols: [
        { ...protocol, version: 1 },
        { ...protocol, version: 3 },
        { ...protocol, cancellationLimit: 0 },
      ].map((value) => window.localTokenFixture.protocolValid(value)),
    };
  });
  expect(results.replies).toEqual([true, false, false, false, false, false]);
  expect(results.protocols).toEqual([false, false, false]);
});
test("an existing preview is queried and explicitly adopted without resubmitting creation", async ({
  page,
}) => {
  await open(page);
  await page.evaluate(() => window.localTokenFixture.setMode("preview-lost"));
  await page.getByRole("button", { name: "准备入口", exact: true }).click();
  await panel(page)
    .getByRole("button", { name: "查询原入口结果", exact: true })
    .click();
  await panel(page)
    .getByRole("button", { name: "采用已核查预览", exact: true })
    .click();
  const state = await snapshot(page);
  expect(state.recovery.hints).toHaveLength(0);
  expect(state.calls).toHaveLength(2);
  await expect(page.getByTestId("adopted")).toHaveText(
    state.identity.operationId,
  );
});
test("foreign query replies and unknown cancellation preserve the original hint", async ({
  page,
}) => {
  await open(page);
  await page.getByRole("button", { name: "准备入口", exact: true }).click();
  await page.evaluate(() => window.localTokenFixture.setMode("foreign"));
  await panel(page)
    .getByRole("button", { name: "查询原入口结果", exact: true })
    .click();
  await expect(panel(page).getByText(/回复或来源不可核查/)).toBeVisible();
  expect((await snapshot(page)).recovery.hints).toHaveLength(1);
  await page.evaluate(() => window.localTokenFixture.setMode("unknown"));
  await panel(page)
    .getByRole("button", { name: "停止这次创建或预览", exact: true })
    .click();
  await expect(panel(page).getByText(/停止结果尚不可核查/)).toBeVisible();
  expect((await snapshot(page)).recovery.hints).toHaveLength(1);
});
test("known cancellation closes the hint while preserving durable ownership metadata", async ({
  page,
}) => {
  await open(page);
  await page.getByRole("button", { name: "准备入口", exact: true }).click();
  await panel(page)
    .getByRole("button", { name: "停止这次创建或预览", exact: true })
    .click();
  await expect(panel(page).getByText(/已停止这次创建/)).toBeVisible();
  const state = await snapshot(page);
  expect(state.operation?.status).toBe("cancelled");
  expect(state.recovery.hints).toHaveLength(0);
});
for (const kind of ["reconnect", "unmount"] as const) {
  test(`late successful submission after ${kind} retains the original recovery hint`, async ({
    page,
  }) => {
    await open(page);
    await page.evaluate(() => {
      window.localTokenFixture.setMode("hold");
      void window.localTokenFixture.submit("fixed_route");
    });
    await expect.poll(async () => (await snapshot(page)).calls.length).toBe(1);
    await page.evaluate((kind) => {
      window.localTokenFixture[kind]();
      window.localTokenFixture.release();
    }, kind);
    await expect
      .poll(async () => (await snapshot(page)).operation?.status)
      .toBe("committed");
    expect((await snapshot(page)).recovery.hints).toHaveLength(1);
  });
}
test("corrupt local bytes block a new submission without erasing them", async ({
  page,
}) => {
  await open(page);
  await page.evaluate(() =>
    localStorage.setItem("local-agent.local-token.v1.broken", "{broken"),
  );
  const message = await page.evaluate(() =>
    window.localTokenFixture.submit("fixed_route"),
  );
  expect(message).toContain("不可核查");
  expect((await snapshot(page)).calls).toHaveLength(0);
  expect(
    await page.evaluate(() =>
      localStorage.getItem("local-agent.local-token.v1.broken"),
    ),
  ).toBe("{broken");
});
test("Unicode model boundaries roundtrip by Unicode scalar count", async ({
  page,
}) => {
  await open(page);
  const valid = await page.evaluate(() =>
    [
      "模".repeat(200),
      "😀".repeat(200),
      "a".repeat(200),
      "模".repeat(201),
      "😀".repeat(201),
    ].map((model) => window.localTokenFixture.modelValid(model)),
  );
  expect(valid).toEqual([true, true, true, false, false]);
});
test("old and future ownership schemas cannot admit a new source", async ({
  page,
}) => {
  await open(page);
  const valid = await page.evaluate(() => {
    const protocol = window.localTokenFixture.snapshot().protocol;
    return [15, 16, 17].map((schemaVersion) =>
      window.localTokenFixture.protocolValid({ ...protocol, schemaVersion }),
    );
  });
  expect(valid).toEqual([false, true, false]);
});
