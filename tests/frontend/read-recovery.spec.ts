import { expect, test, type Page } from "@playwright/test";
import type {} from "./read-recovery-harness";
const count = (page: Page, name: string, total: number) =>
  expect
    .poll(() =>
      page.evaluate(
        (name) =>
          window.readRecovery.calls.filter((call) => call === name).length,
        name,
      ),
    )
    .toBe(total);
async function recover(page: Page) {
  await page
    .getByRole("button", { name: "核对并重新连接", exact: true })
    .click();
  await expect
    .poll(() => page.evaluate(() => window.readRecovery.connect.length))
    .toBe(1);
  await page.evaluate(() => window.readRecovery.connect[0]());
}
test.beforeEach(async ({ page }) => {
  const time = new Date("2026-10-27T00:00:00.000Z");
  await page.clock.install({ time });
  await page.clock.setFixedTime(time);
});

test("confirmed recovery refreshes active reads once and preserves settings and client drafts", async ({
  page,
}) => {
  await page.goto("/tests/frontend/read-recovery.html");
  await expect(page.getByTestId("root")).toHaveText("snapshot-0");
  await count(page, "get_provider_status", 1);
  await page
    .getByRole("switch", { name: "额外保存消息与工具内容", exact: true })
    .click();
  await page
    .getByLabel("客户端名称", { exact: true })
    .fill("unfinished client");
  await recover(page);
  await expect(page.getByTestId("root")).toHaveText("snapshot-1");
  await expect(page.getByTestId("provider-a")).toHaveText("provider-1");
  await expect(page.getByTestId("provider-b")).toHaveText("provider-1");
  await expect(page.getByTestId("local")).toHaveText("local-1");
  await expect(page.getByText("client-1", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("switch", { name: "额外保存消息与工具内容", exact: true }),
  ).toBeChecked();
  await expect(page.getByText("设置已有新版本", { exact: true })).toBeVisible();
  await expect(page.getByLabel("客户端名称", { exact: true })).toHaveValue(
    "unfinished client",
  );
  for (const name of [
    "get_snapshot",
    "get_gateway_status",
    "get_provider_status",
    "local",
  ])
    await count(page, name, 2);
  const calls = await page.evaluate(() => window.readRecovery.calls);
  expect(
    calls.every((name) =>
      [
        "get_snapshot",
        "get_gateway_status",
        "get_provider_status",
        "get_backend_diagnostics",
        "local",
        "reconnect_backend",
      ].includes(name),
    ),
  ).toBe(true);
});

test("unmounting the reconnect button does not prevent remaining readers from recovering", async ({
  page,
}) => {
  await page.goto("/tests/frontend/read-recovery.html");
  await expect(page.getByTestId("root")).toHaveText("snapshot-0");
  await page
    .getByRole("button", { name: "核对并重新连接", exact: true })
    .click();
  await page
    .getByRole("button", { name: "toggle-connection", exact: true })
    .click();
  await page.evaluate(() => window.readRecovery.connect[0]());
  await expect(page.getByTestId("root")).toHaveText("snapshot-1");
  await count(page, "get_gateway_status", 2);
});

test("unconfirmed, rejected or malformed recovery does not invalidate readers", async ({
  page,
}) => {
  for (const connection of ["unconfirmed", "reject", "malformed"] as const) {
    await page.goto("/tests/frontend/read-recovery.html");
    await expect(page.getByTestId("root")).toHaveText("snapshot-0");
    await page.evaluate((value) => {
      window.readRecovery.connection = value;
    }, connection);
    await recover(page);
    await page.clock.runFor(100);
    await count(page, "get_snapshot", 1);
    await count(page, "get_gateway_status", 1);
    await count(page, "get_provider_status", 1);
    await expect(page.getByTestId("root")).toHaveText("snapshot-0");
  }
});

test("initial gateway failure is retried after recovery without replaying a mutation", async ({
  page,
}) => {
  await page.goto("/tests/frontend/read-recovery.html?fail=get_gateway_status");
  await expect(page.getByText("网关状态不可用", { exact: true })).toBeVisible();
  await page.evaluate(() => {
    window.readRecovery.fail = [];
  });
  await recover(page);
  await expect(page.getByText("client-1", { exact: true })).toBeVisible();
  await count(page, "get_gateway_status", 2);
});

test("post-recovery read failure retains confirmed connection and marks old data", async ({
  page,
}) => {
  await page.goto("/tests/frontend/read-recovery.html");
  await expect(page.getByText("client-0", { exact: true })).toBeVisible();
  await page.evaluate(() => {
    window.readRecovery.fail = ["get_gateway_status"];
  });
  await recover(page);
  await expect(
    page.getByText("后台连接已通过身份核对。数据读取状态以页面提示为准。", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByText(/fixture read unavailable: get_gateway_status/),
  ).toBeVisible();
  await expect(page.getByText("client-0", { exact: true })).toBeVisible();
});

test("raw domain API reads bypass the shared UI resource", async ({ page }) => {
  await page.goto("/tests/frontend/read-recovery.html");
  await count(page, "get_provider_status", 1);
  await page.evaluate(async () => {
    await window.readRecovery.rawProviderStatus();
    await window.readRecovery.rawProviderStatus();
  });
  await count(page, "get_provider_status", 3);
});

test("paginated native readers recover to a fresh first page", async ({
  page,
}) => {
  await page.goto("/tests/frontend/read-recovery.html?view=native");
  await expect(page.getByTestId("native")).toHaveText("page-0-first");
  await page.getByRole("button", { name: "next-page", exact: true }).click();
  await expect(page.getByTestId("native")).toHaveText(
    "page-0-first,page-0-next",
  );
  await recover(page);
  await expect(page.getByTestId("native")).toHaveText("page-1-first");
  await count(page, "sessions", 3);
});

test("overview recovery does not reuse the previous connection's ten-second analysis cache", async ({
  page,
}) => {
  await page.goto("/tests/frontend/read-recovery.html?view=overview");
  await expect(page.getByTestId("analysis")).toHaveText(
    "2026-10-27T00:00:00.000Z",
  );
  await recover(page);
  await expect(page.getByTestId("analysis")).toHaveText(
    "2026-10-27T00:00:00.001Z",
  );
  await expect(page.getByTestId("analysis-updating")).toHaveText("false");
  await count(page, "overview", 2);
});

test("a cached conversation hidden during recovery is reread on return", async ({
  page,
}) => {
  await page.goto("/tests/frontend/read-recovery.html?view=conversation");
  await expect(
    page.getByRole("heading", { name: "conversation-0", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "toggle-reader", exact: true })
    .click();
  await recover(page);
  await count(page, "conversation", 1);
  await page
    .getByRole("button", { name: "toggle-reader", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "conversation-1", exact: true }),
  ).toBeVisible();
  await count(page, "conversation", 2);
});

test("a late old conversation cannot overwrite or repopulate the new connection's cache", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/read-recovery.html?view=conversation&hold=conversation",
  );
  await count(page, "conversation", 1);
  await page.evaluate(() => {
    window.readRecovery.hold = [];
  });
  await recover(page);
  await expect(
    page.getByRole("heading", { name: "conversation-1", exact: true }),
  ).toBeVisible();
  await page.evaluate(() => window.readRecovery.held[0].release());
  await page
    .getByRole("button", { name: "toggle-reader", exact: true })
    .click();
  await page
    .getByRole("button", { name: "toggle-reader", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "conversation-1", exact: true }),
  ).toBeVisible();
  await count(page, "conversation", 2);
});

for (const [view, name] of [
  ["memory", "query_agent_memory"],
  ["mcp", "mcp"],
  ["requests", "requests"],
])
  test(`${view} active local reads are invalidated on recovery`, async ({
    page,
  }) => {
    await page.goto("/tests/frontend/read-recovery.html?view=" + view);
    await count(page, name, 1);
    await recover(page);
    await count(page, name, 2);
  });

test("an open native memory preview is revalidated while retaining the selected file", async ({
  page,
}) => {
  await page.goto("/tests/frontend/read-recovery.html?view=memory");
  await page
    .getByRole("button", { name: "查看 memory-0.md", exact: true })
    .click();
  const preview = page.getByRole("region", { name: "记忆正文", exact: true });
  await expect(preview).toContainText("memory-0");
  await recover(page);
  await expect(preview).toContainText("memory-1");
  await count(page, "query_agent_memory", 3);
});

test("on-demand diffs revalidate on recovery but do not start background polling", async ({
  page,
}) => {
  await page.goto("/tests/frontend/read-recovery.html?view=diff");
  await page.getByRole("button", { name: "查看当前差异", exact: true }).click();
  await expect(page.getByText("diff-0", { exact: true })).toBeVisible();
  await page.clock.fastForward(60000);
  await count(page, "get_project_diff", 1);
  await recover(page);
  await expect(page.getByText("diff-1", { exact: true })).toBeVisible();
  await count(page, "get_project_diff", 2);
});

test("an opened backup inventory refreshes without replaying its restore action", async ({
  page,
}) => {
  await page.goto("/tests/frontend/read-recovery.html?view=backups");
  await page
    .getByRole("button", { name: "配置备份与恢复", exact: true })
    .click();
  await count(page, "backups", 1);
  await page.clock.fastForward(60000);
  await count(page, "backups", 1);
  await recover(page);
  await count(page, "backups", 2);
  expect(
    await page.evaluate(() =>
      window.readRecovery.calls.every((name) =>
        ["backups", "get_backend_diagnostics", "reconnect_backend"].includes(
          name,
        ),
      ),
    ),
  ).toBe(true);
});

test("recovery invalidates an import preview without discarding its model draft or auto-scanning", async ({
  page,
}) => {
  await page.goto("/tests/frontend/read-recovery.html?view=import");
  await page
    .getByRole("button", { name: "核对并重新连接", exact: true })
    .click();
  await page.getByRole("button", { name: "导入本机配置", exact: true }).click();
  const model = page.getByRole("textbox", {
    name: "默认模型",
    exact: true,
  });
  await model.fill("unfinished-model");
  const submit = page.getByRole("button", {
    name: "导入到 Providers",
    exact: true,
  });
  await expect(submit).toBeEnabled();
  await page.evaluate(() => window.readRecovery.connect[0]());
  await expect(
    page.getByText("请重新识别并核对导入目标；输入已保留。", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(model).toHaveValue("unfinished-model");
  await expect(submit).toBeDisabled();
  await count(page, "preview_native_provider_import", 1);
  await page.getByRole("button", { name: "重新识别", exact: true }).click();
  await count(page, "preview_native_provider_import", 2);
  await model.fill("newly-confirmed-model");
  await expect(submit).toBeEnabled();
  expect(
    await page.evaluate(() =>
      window.readRecovery.calls.includes("import_native_provider"),
    ),
  ).toBe(false);
});

test("a late pre-recovery native scan cannot turn an unknown preview into confirmed empty data", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/read-recovery.html?view=import&hold=preview_native_provider_import",
  );
  await page
    .getByRole("button", { name: "核对并重新连接", exact: true })
    .click();
  await page.getByRole("button", { name: "导入本机配置", exact: true }).click();
  await count(page, "preview_native_provider_import", 1);
  await page.evaluate(() => window.readRecovery.connect[0]());
  await expect(
    page.getByText("请重新识别并核对导入目标；输入已保留。", {
      exact: true,
    }),
  ).toBeVisible();
  await page.evaluate(() => window.readRecovery.held[0].release());
  await page.clock.runFor(100);
  await expect(
    page.getByText("没有发现可识别的本机配置。", { exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "导入到 Providers", exact: true }),
  ).toBeDisabled();
  await page.evaluate(() => {
    window.readRecovery.hold = [];
  });
  await page.getByRole("button", { name: "重新识别", exact: true }).click();
  await expect(
    page.getByRole("textbox", { name: "默认模型", exact: true }),
  ).toBeVisible();
  await count(page, "preview_native_provider_import", 2);
});
