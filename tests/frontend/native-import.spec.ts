import { test, expect, type Page } from "@playwright/test";
import type {} from "./native-import-harness";
import {
  nativeImportOperation,
  nativeImportProtocol,
  sameNativeImport,
  type NativeImportRequest,
} from "../../src/features/providers/native-import-operation";
import type { Provider } from "../../src/features/providers/types";

const records = (page: Page) =>
  page.evaluate(() =>
    Object.entries(localStorage).filter(([key]) =>
      key.startsWith("local-agent.native-import.v1."),
    ),
  );
const submits = (page: Page) =>
  page.evaluate(() => window.nativeImportFixture.submits);
async function begin(page: Page, mode: string) {
  await page.goto("/tests/frontend/native-import.html?mode=" + mode);
  await page.getByRole("button", { name: "导入本机配置", exact: true }).click();
  await page
    .getByRole("button", { name: "导入到 Providers", exact: true })
    .click();
}
test("lost committed reply is queried independently and never resubmitted", async ({
  page,
}) => {
  await begin(page, "lost-success");
  await expect(page.getByTestId("native-import-recovery")).toBeVisible();
  expect(await records(page)).toHaveLength(1);
  await page
    .getByRole("button", { name: "查询原导入结果", exact: true })
    .click();
  await expect(
    page.getByText(/原导入已保存 Fixture native provider/),
  ).toBeVisible();
  expect(await submits(page)).toHaveLength(1);
  expect(await records(page)).toHaveLength(0);
  expect(
    await page.evaluate(() => window.nativeImportFixture.queries.length),
  ).toBe(1);
});
test("pending result requires source review and separate explicit continuation of the same request", async ({
  page,
}) => {
  await begin(page, "pending");
  await expect(
    page.getByText("请重新识别并核对导入目标；输入已保留。", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("后台连接已更新，请重新识别并核对导入目标；输入已保留。", {
      exact: true,
    }),
  ).toHaveCount(0);
  const original = (await submits(page))[0];
  await page
    .getByRole("button", { name: "查询原导入结果", exact: true })
    .click();
  await expect(page.getByText(/原导入尚未完成/)).toBeVisible();
  expect(await submits(page)).toHaveLength(1);
  await page.getByRole("button", { name: "核对原来源", exact: true }).click();
  await expect(
    page.getByText("原来源：Fixture native provider", { exact: true }),
  ).toBeVisible();
  const resume = page.getByRole("button", { name: "继续原导入", exact: true });
  await expect(resume).toBeDisabled();
  await page
    .getByRole("checkbox", { name: "已核对原导入来源", exact: true })
    .check();
  await resume.click();
  await expect(
    page.getByText(/原导入已保存 Fixture native provider/),
  ).toBeVisible();
  expect(await submits(page)).toEqual([original, original]);
  expect(await records(page)).toHaveLength(0);
});
test("changed source keeps the pending request and disables continuation; cancellation has its own action", async ({
  page,
}) => {
  await begin(page, "source-change");
  const original = (await submits(page))[0];
  await page.evaluate(() => window.nativeImportFixture.rotate());
  await page.getByRole("button", { name: "核对原来源", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "继续原导入", exact: true }),
  ).toHaveCount(0);
  expect(await submits(page)).toHaveLength(1);
  expect(await records(page)).toHaveLength(1);
  await page.getByRole("button", { name: "取消这次导入", exact: true }).click();
  await expect(page.getByText(/已取消原导入/)).toBeVisible();
  expect(await page.evaluate(() => window.nativeImportFixture.cancels)).toEqual(
    [original],
  );
  expect(await submits(page)).toHaveLength(1);
  expect(await records(page)).toHaveLength(0);
});
test("reload preserves the exact request and server discovery restores a missing local hint", async ({
  page,
}) => {
  await begin(page, "reload");
  const original = (await submits(page))[0];
  expect(await records(page)).toHaveLength(1);
  await page.reload();
  await expect(page.getByTestId("native-import-recovery")).toBeVisible();
  expect(await submits(page)).toHaveLength(0);
  expect((await records(page))[0][1]).toBe(JSON.stringify(original));
  await page.evaluate(() => localStorage.clear());
  await page.reload();
  await expect(page.getByTestId("native-import-recovery")).toHaveCount(0);
  await page.getByRole("button", { name: "导入本机配置", exact: true }).click();
  await expect(page.getByTestId("native-import-recovery")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "导入到 Providers", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("button", { name: "查询原导入结果", exact: true })
    .click();
  expect(await submits(page)).toHaveLength(0);
  expect(await page.evaluate(() => window.nativeImportFixture.queries)).toEqual(
    [original],
  );
});
test("directory refresh failure never becomes a second import", async ({
  page,
}) => {
  await begin(page, "refresh-fails");
  await expect(page.getByText(/已保存，页面刷新未完成/)).toBeVisible();
  expect(await submits(page)).toHaveLength(1);
  expect(await records(page)).toHaveLength(0);
  expect(await page.evaluate(() => window.nativeImportFixture.refreshes)).toBe(
    1,
  );
});
test("a malformed success response retains the original request until independent query", async ({
  page,
}) => {
  await begin(page, "bad-result");
  await expect(page.getByTestId("native-import-recovery")).toBeVisible();
  expect(await records(page)).toHaveLength(1);
  await page
    .getByRole("button", { name: "查询原导入结果", exact: true })
    .click();
  await expect(page.getByText(/原导入已保存/)).toBeVisible();
  expect(await records(page)).toHaveLength(0);
  expect(await submits(page)).toHaveLength(1);
});
test("storage refusal prevents submitting any import", async ({ page }) => {
  await begin(page, "storage-fails");
  await expect(page.getByText(/QuotaExceededError|quota/)).toBeVisible();
  expect(await submits(page)).toHaveLength(0);
  expect(await records(page)).toHaveLength(0);
});
test("corrupt recovery bytes remain intact and prevent a new import", async ({
  page,
}) => {
  await page.goto("/tests/frontend/native-import.html?mode=bad-storage");
  await expect(
    page.getByText("导入记录需要核查", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "导入本机配置", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "导入到 Providers", exact: true }),
  ).toBeDisabled();
  expect(
    await page.evaluate(() =>
      localStorage.getItem("local-agent.native-import.v1.bad"),
    ),
  ).toBe("original-unreadable-bytes");
  expect(await submits(page)).toHaveLength(0);
});
test("backend recovery during a held import preserves the request and releases the busy UI", async ({
  page,
}) => {
  await begin(page, "hold");
  await expect.poll(() => submits(page)).toHaveLength(1);
  await page.evaluate(() => window.nativeImportFixture.recover());
  await page.evaluate(() => window.nativeImportFixture.release?.());
  await expect(
    page.getByRole("button", { name: "取消", exact: true }),
  ).toBeEnabled();
  expect(await records(page)).toHaveLength(1);
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "导入本机配置", exact: true }),
  ).toBeEnabled();
  expect(await submits(page)).toHaveLength(1);
});

test("a future preview capability stays an explicit read error and never enables an import", async ({
  page,
}) => {
  await page.goto("/tests/frontend/native-import.html?mode=weak-protocol");
  await page.getByRole("button", { name: "导入本机配置", exact: true }).click();
  await expect(
    page.getByText(/当前后台的原生导入能力或来源无法核查/),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "导入到 Providers", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByText("没有发现可识别的本机配置。", { exact: true }),
  ).toHaveCount(0);
  expect(await submits(page)).toHaveLength(0);
  expect(await records(page)).toHaveLength(0);
});

test("import result validation rejects uncommitted metadata, a changed request and unknown credential fields", () => {
  const request: NativeImportRequest = {
    candidateId: "claude-fixture",
    expectedRevision: "a".repeat(64),
    model: "model",
    operation: {
      storeId: "71a3e175-52cf-41da-8631-c1fb652bb2a7",
      namespaceId: "6e183d0d-6268-4e1b-9abc-25d3e66f8249",
      operationId: "558073dc-5a3d-47c3-8ecf-0c00b8020f99",
      expectedSequence: 0,
    },
  };
  const provider: Provider = {
    id: "custom-" + request.operation.operationId,
    name: "Fixture",
    vendor: "custom",
    endpoint: "https://fixture.invalid",
    protocol: "messages",
    model: "model",
    configured: true,
    credentialRef: "8dff2797-d847-4e5f-8c99-73d32ef378db",
    credentialLabel: "Native import",
    models: ["model"],
    disabledModels: [],
    modelCatalog: null,
    revision: 1,
    category: "custom",
  };
  const valid = {
    version: 1,
    sequence: 1,
    status: "committed",
    request,
    provider,
    reused: false,
    createsCredential: true,
  };
  expect(nativeImportOperation(valid, request).status).toBe("committed");
  for (const invalid of [
    { ...valid, provider: { ...provider, revision: 0 } },
    { ...valid, provider: { ...provider, configured: false } },
    { ...valid, provider: { ...provider, credentialRef: null } },
    { ...valid, request: { ...request, model: "changed" } },
    { ...valid, status: "pending" },
    { ...valid, secret: "unexpected-field" },
    { ...valid, reused: true },
  ])
    expect(() => nativeImportOperation(invalid, request)).toThrow(
      "原导入回复无法核查",
    );
  expect(
    nativeImportOperation(
      { ...valid, status: "pending", provider: { ...provider, revision: 0 } },
      request,
    ).status,
  ).toBe("pending");
  expect(
    sameNativeImport(request, {
      operation: request.operation,
      model: request.model,
      expectedRevision: request.expectedRevision,
      candidateId: request.candidateId,
    }),
  ).toBe(true);
  try {
    nativeImportProtocol({ version: 2 });
    throw new Error("future protocol accepted");
  } catch (error) {
    expect(error).toMatchObject({
      code: "native_import_protocol_unavailable",
      outcomeUnknown: false,
    });
  }
});

test("a proven expired sequence clears only its original hint and requires a fresh preview", async ({
  page,
}) => {
  await begin(page, "expired");
  expect(await records(page)).toHaveLength(1);
  await page
    .getByRole("button", { name: "查询原导入结果", exact: true })
    .click();
  await expect(
    page.getByText(/原导入请求已过期，后台未接纳这次导入/),
  ).toBeVisible();
  expect(await records(page)).toHaveLength(0);
  expect(await submits(page)).toHaveLength(1);
  await expect(
    page.getByRole("button", { name: "导入到 Providers", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "重新识别", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "导入到 Providers", exact: true }),
  ).toBeEnabled();
  expect(await submits(page)).toHaveLength(1);
});

test("a full retained journal permits result checks but prevents creating another hint", async ({
  page,
}) => {
  await page.goto("/tests/frontend/native-import.html?mode=full");
  await page.getByRole("button", { name: "导入本机配置", exact: true }).click();
  await expect(
    page.getByText("导入记录容量已满", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "导入到 Providers", exact: true }),
  ).toBeDisabled();
  expect(await records(page)).toHaveLength(0);
  expect(await submits(page)).toHaveLength(0);
});

test("unmounting during a held import preserves its request for a later independent query", async ({
  page,
}) => {
  await begin(page, "hold");
  await expect.poll(() => submits(page)).toHaveLength(1);
  const original = (await submits(page))[0];
  await page.evaluate(() => window.nativeImportFixture.unmount?.());
  await page.evaluate(() => window.nativeImportFixture.release?.());
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          Object.values(window.nativeImportFixture.state.operations)[0]?.status,
      ),
    )
    .toBe("committed");
  expect(await records(page)).toHaveLength(1);
  expect(await page.evaluate(() => window.nativeImportFixture.refreshes)).toBe(
    0,
  );
  await page.reload();
  await expect(page.getByTestId("native-import-recovery")).toBeVisible();
  await page
    .getByRole("button", { name: "查询原导入结果", exact: true })
    .click();
  await expect(page.getByText(/原导入已保存/)).toBeVisible();
  expect(await records(page)).toHaveLength(0);
  expect(await submits(page)).toHaveLength(0);
  expect(await page.evaluate(() => window.nativeImportFixture.queries)).toEqual(
    [original],
  );
});
