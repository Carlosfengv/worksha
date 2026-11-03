import { expect, test, type Page } from "@playwright/test";
import type {} from "./provider-auto-connect-harness";
const PROVIDER_FIXTURE_STORE_ID = "c9e027fe-4536-4d08-8a55-74e6cbaf6313";
const PROVIDER_FIXTURE_OTHER_STORE_ID = "6087597b-0625-42ea-8fd0-e51f6ad16d01";

async function open(page: Page, mode: string, key = false) {
  await page.goto(`/tests/frontend/provider-auto-connect.html?mode=${mode}`);
  await page
    .getByRole("button", { name: "自动识别并验证", exact: true })
    .click();
  await page
    .getByRole("textbox", { name: "服务地址 URL", exact: true })
    .fill(key ? "https://fixture.invalid/v1" : "http://127.0.0.1:11434/v1");
  if (key)
    await page
      .getByRole("textbox", { name: "API Key", exact: true })
      .fill("disposable-operation-key");
}
async function submit(page: Page) {
  await page.getByRole("button", { name: "连接并保存", exact: true }).click();
}

test("the full supported 2000-model catalog and disabled list fit the recoverable write protocol", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=operation-large-catalog",
  );
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "取消", exact: true })
    .click();
  const expected = await page.evaluate((storeId) => {
    const models = Array.from(
      { length: 2000 },
      (_, n) => `${String(n).padStart(4, "0")}-${"m".repeat(195)}`,
    );
    const operationId = crypto.randomUUID();
    const request = {
      storeId,
      operationId,
      expectedSequence: 0,
      kind: "save",
      provider: {
        id: `custom-${crypto.randomUUID()}`,
        name: "完整目录",
        vendor: "custom",
        endpoint: "http://127.0.0.1:11434/v1",
        protocol: "responses",
        model: models[0],
        models,
        disabledModels: models.slice(1),
        configured: false,
        credentialRef: null,
        credentialLabel: null,
        revision: 0,
        category: "local",
      },
    };
    const raw = JSON.stringify(request);
    localStorage.setItem(
      "local-agent.provider-operation.v1." + operationId,
      raw,
    );
    window.dispatchEvent(new Event("provider-operations-changed"));
    return { operationId, bytes: new TextEncoder().encode(raw).length };
  }, PROVIDER_FIXTURE_STORE_ID);
  expect(expected.bytes).toBeGreaterThan(512 * 1024);
  const region = page.getByRole("region", {
    name: "服务商操作恢复",
    exact: true,
  });
  await expect(
    region.getByRole("button", { name: "重试原请求", exact: true }),
  ).toBeVisible();
  await region.getByRole("button", { name: "重试原请求", exact: true }).click();
  await expect(
    region.getByText("原操作已确认保存。", { exact: true }),
  ).toBeVisible();
  const state = await page.evaluate(() =>
    window.providerOperationsFixture.snapshot(),
  );
  const saved =
    state.stores[PROVIDER_FIXTURE_STORE_ID].operations[expected.operationId]
      .provider;
  expect(saved?.models).toHaveLength(2000);
  expect(saved?.disabledModels).toHaveLength(1999);
  expect(saved?.revision).toBe(1);
  expect(await records(page)).toHaveLength(0);
});
const records = (page: Page) =>
  page.evaluate(() =>
    Object.keys(localStorage)
      .filter((key) => key.startsWith("local-agent.provider-operation.v1."))
      .map((key) => localStorage.getItem(key)),
  );
test("lost commit reply is recovered by a read without repeating inference or metadata", async ({
  page,
}) => {
  await open(page, "operation-lost");
  await submit(page);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const state = await page.evaluate(() =>
    window.providerOperationsFixture.snapshot(),
  );
  expect(state.stores[PROVIDER_FIXTURE_STORE_ID].probes).toBe(1);
  expect(state.stores[PROVIDER_FIXTURE_STORE_ID].commits).toBe(1);
  expect(await records(page)).toHaveLength(0);
  const commands = await page.evaluate(() =>
    window.providerOperationsFixture.commands.map((row) => row.name),
  );
  expect(
    commands.filter((name) => name === "commit_provider_operation"),
  ).toHaveLength(1);
  expect(commands).toContain("get_provider_operation");
});
test("unknown result blocks new saves and recovers after closing without persisting the key", async ({
  page,
}) => {
  await open(page, "operation-query-unavailable", true);
  await submit(page);
  await expect(page.getByText("保存结果待核查", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "连接并保存", exact: true }),
  ).toBeDisabled();
  const stored = await records(page);
  expect(stored).toHaveLength(1);
  expect(JSON.stringify(stored)).not.toContain("disposable-operation-key");
  await page
    .getByRole("button", { name: "关闭并保留记录", exact: true })
    .click();
  await page.evaluate(() => {
    window.providerOperationsFixture.failQueries = false;
  });
  await page
    .getByRole("region", { name: "服务商操作恢复", exact: true })
    .getByRole("button", { name: "核查原结果", exact: true })
    .click();
  await expect(
    page.getByText("原操作已确认保存。", { exact: true }),
  ).toBeVisible();
  expect(await records(page)).toHaveLength(0);
  expect(
    (await page.evaluate(() => window.providerOperationsFixture.snapshot()))
      .stores[PROVIDER_FIXTURE_STORE_ID].probes,
  ).toBe(1);
});
test("pending query never probes again and cancellation prevents a late commit", async ({
  page,
}) => {
  await open(page, "operation-pending");
  await submit(page);
  await expect(page.getByText("保存结果待核查", { exact: true })).toBeVisible();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "核查原结果", exact: true }).click();
  await expect(
    dialog.getByText("保存结果待核查", { exact: true }),
  ).toBeVisible();
  await dialog.getByRole("button", { name: "取消原操作", exact: true }).click();
  await expect(
    dialog.getByText("原操作已取消，后续结果不会保存。", { exact: true }),
  ).toBeVisible();
  await page.evaluate(() => window.providerOperationsFixture.release());
  await expect
    .poll(async () => {
      const s = await page.evaluate(() =>
        window.providerOperationsFixture.snapshot(),
      );
      return s.stores[PROVIDER_FIXTURE_STORE_ID].commits;
    })
    .toBe(0);
  const state = await page.evaluate(() =>
    window.providerOperationsFixture.snapshot(),
  );
  expect(state.stores[PROVIDER_FIXTURE_STORE_ID].probes).toBe(1);
  expect(
    Object.values(state.stores[PROVIDER_FIXTURE_STORE_ID].operations).map(
      (op) => op.status,
    ),
  ).toEqual(["cancelled"]);
});
test("reopening after local recovery data is lost can recover pending IDs from the backend", async ({
  page,
}) => {
  await open(page, "operation-pending");
  await submit(page);
  await expect(page.getByText("保存结果待核查", { exact: true })).toBeVisible();
  await page.evaluate(() => {
    for (const key of Object.keys(localStorage))
      if (key.startsWith("local-agent.provider-operation.v1."))
        localStorage.removeItem(key);
  });
  await page.reload();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "取消", exact: true })
    .click();
  const region = page.getByRole("region", {
    name: "服务商操作恢复",
    exact: true,
  });
  await region
    .getByRole("button", { name: "读取后台待核查操作", exact: true })
    .click();
  await expect(
    region.getByRole("button", { name: "核查原结果", exact: true }),
  ).toBeVisible();
  await region.getByRole("button", { name: "取消原操作", exact: true }).click();
  await expect(
    region.getByText("原操作已确认取消。", { exact: true }),
  ).toBeVisible();
  const state = await page.evaluate(() =>
    window.providerOperationsFixture.snapshot(),
  );
  expect(state.stores[PROVIDER_FIXTURE_STORE_ID].probes).toBe(1);
  expect(state.stores[PROVIDER_FIXTURE_STORE_ID].commits).toBe(0);
});
test("changing the backend before first submit preserves the draft and sends no credential or probe", async ({
  page,
}) => {
  await open(page, "operation-source");
  await page.evaluate(
    (id) => window.providerOperationsFixture.switchStore(id),
    PROVIDER_FIXTURE_OTHER_STORE_ID,
  );
  await submit(page);
  await expect(page.getByText(/草稿属于另一个本机数据库/)).toBeVisible();
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections),
  ).toHaveLength(0);
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(0);
  expect(await records(page)).toHaveLength(0);
  await expect(
    page.getByRole("textbox", { name: "服务地址 URL", exact: true }),
  ).toHaveValue("http://127.0.0.1:11434/v1");
});
test("a completed write from the old connection stays recoverable and is not applied to a new view", async ({
  page,
}) => {
  await open(page, "hold");
  await submit(page);
  await expect
    .poll(() =>
      page.evaluate(() => window.autoConnectFixture.connections.length),
    )
    .toBe(1);
  await page.evaluate((id) => {
    window.providerOperationsFixture.switchStore(id);
    window.autoConnectFixture.release?.();
  }, PROVIDER_FIXTURE_OTHER_STORE_ID);
  await expect(page.getByText("保存结果待核查", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => window.autoConnectFixture.saved)).toBe(0);
  expect(await records(page)).toHaveLength(1);
  await page.evaluate(
    (id) => window.providerOperationsFixture.switchStore(id),
    PROVIDER_FIXTURE_STORE_ID,
  );
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "核查原结果", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    (await page.evaluate(() => window.providerOperationsFixture.snapshot()))
      .stores[PROVIDER_FIXTURE_STORE_ID].probes,
  ).toBe(1);
});
test("an older backend blocks credential creation and metadata writes", async ({
  page,
}) => {
  await open(page, "operation-unsupported", true);
  await submit(page);
  await expect(page.getByText(/当前后台不支持服务商结果核查/)).toBeVisible();
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(0);
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections),
  ).toHaveLength(0);
});

test("a confirmed save remains successful when its list refresh fails", async ({
  page,
}) => {
  await open(page, "operation-refresh-error");
  await submit(page);
  await expect(page.getByText("保存已确认", { exact: true })).toBeVisible();
  await expect(page.getByText(/合成列表刷新失败/)).toBeVisible();
  await expect(
    page.getByRole("button", { name: "连接并保存", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "重新读取列表", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const state = await page.evaluate(() =>
    window.providerOperationsFixture.snapshot(),
  );
  expect(state.stores[PROVIDER_FIXTURE_STORE_ID].probes).toBe(1);
  expect(state.stores[PROVIDER_FIXTURE_STORE_ID].commits).toBe(1);
});
test("failed recovery storage prevents sending the operation", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const original = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (key.startsWith("local-agent.provider-operation.v1."))
        throw new DOMException("合成恢复记录写入失败", "QuotaExceededError");
      return original.call(this, key, value);
    };
  });
  await open(page, "operation-storage-failed");
  await submit(page);
  await expect(page.getByText(/合成恢复记录写入失败/)).toBeVisible();
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections),
  ).toHaveLength(0);
  expect(
    await page.evaluate(() =>
      window.providerOperationsFixture.commands.filter(
        (row) => row.name === "commit_provider_operation",
      ),
    ),
  ).toHaveLength(0);
});
test("an unverified snapshot origin disables saving before any side effects", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=operation-origin-missing",
  );
  await expect(
    page.getByText("配置来源尚未核对", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "保存连接", exact: true }),
  ).toBeDisabled();
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(0);
  expect(
    await page.evaluate(() => window.providerOperationsFixture.commands),
  ).toHaveLength(0);
});
test("a changed snapshot cannot reassign an open draft and reconnecting the original source preserves it", async ({
  page,
}) => {
  await open(page, "operation-prop-source");
  await page.evaluate((id) => {
    window.autoConnectFixture.sourceChange?.(id);
    window.providerOperationsFixture.switchStore(id);
  }, PROVIDER_FIXTURE_OTHER_STORE_ID);
  await expect(
    page.getByText("草稿属于原来的本机数据库，请连接原后台后核查。", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "连接并保存", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByRole("textbox", { name: "服务地址 URL", exact: true }),
  ).toHaveValue("http://127.0.0.1:11434/v1");
  await page.evaluate((id) => {
    window.autoConnectFixture.sourceChange?.(id);
    window.providerOperationsFixture.switchStore(id);
  }, PROVIDER_FIXTURE_STORE_ID);
  await expect(
    page.getByRole("button", { name: "连接并保存", exact: true }),
  ).toBeEnabled();
  await submit(page);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    (await page.evaluate(() => window.providerOperationsFixture.snapshot()))
      .stores[PROVIDER_FIXTURE_OTHER_STORE_ID].probes,
  ).toBe(0);
});

test("a snapshot becoming unknown suppresses a late result while preserving its recovery request", async ({
  page,
}) => {
  await open(page, "hold");
  await submit(page);
  await expect
    .poll(() =>
      page.evaluate(() => window.autoConnectFixture.connections.length),
    )
    .toBe(1);
  await page.evaluate(() => {
    window.autoConnectFixture.sourceChange?.(undefined);
  });
  await expect(
    page.getByText("配置来源尚未核对", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "连接并保存", exact: true }),
  ).toBeDisabled();
  await page.evaluate(() => window.autoConnectFixture.release?.());
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.providerOperationsFixture.snapshot().stores[
            "c9e027fe-4536-4d08-8a55-74e6cbaf6313"
          ].commits,
      ),
    )
    .toBe(1);
  expect(await page.evaluate(() => window.autoConnectFixture.saved)).toBe(0);
  expect(await records(page)).toHaveLength(1);
  await page.evaluate(
    (id) => window.autoConnectFixture.sourceChange?.(id),
    PROVIDER_FIXTURE_STORE_ID,
  );
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "取消", exact: true })
    .click();
  await page
    .getByRole("region", { name: "服务商操作恢复", exact: true })
    .getByRole("button", { name: "核查原结果", exact: true })
    .click();
  await expect(
    page.getByText("原操作已确认保存。", { exact: true }),
  ).toBeVisible();
});
test("unreadable recovery bytes require explicit review and can be kept or removed", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?mode=operation-corrupt-local",
  );
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "取消", exact: true })
    .click();
  const key = await page.evaluate(() => {
    const key = "local-agent.provider-operation.v1." + crypto.randomUUID();
    localStorage.setItem(key, "invalid-synthetic-recovery");
    window.dispatchEvent(new Event("provider-operations-changed"));
    return key;
  });
  await page
    .getByRole("button", { name: "手动核对后移除提示", exact: true })
    .click();
  await page.getByRole("button", { name: "返回核查", exact: true }).click();
  expect(await page.evaluate((key) => localStorage.getItem(key), key)).toBe(
    "invalid-synthetic-recovery",
  );
  await page
    .getByRole("button", { name: "手动核对后移除提示", exact: true })
    .click();
  await page.evaluate(() => {
    const original = Storage.prototype.removeItem;
    Storage.prototype.removeItem = () => {
      Storage.prototype.removeItem = original;
    };
  });
  await page
    .getByRole("button", { name: "已核对，移除本地提示", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toContainText(
    "恢复记录未能移除，请重新核查。",
  );
  expect(await page.evaluate((key) => localStorage.getItem(key), key)).toBe(
    "invalid-synthetic-recovery",
  );
  await page
    .getByRole("button", { name: "已核对，移除本地提示", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    await page.evaluate((key) => localStorage.getItem(key), key),
  ).toBeNull();
  expect(
    (await page.evaluate(() => window.providerOperationsFixture.snapshot()))
      .stores[PROVIDER_FIXTURE_STORE_ID].commits,
  ).toBe(0);
});
test("recovery remains scrollable and keyboard reachable at narrow dark size", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await open(page, "operation-query-unavailable");
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  await submit(page);
  await expect(page.getByText("保存结果待核查", { exact: true })).toBeVisible();
  await page
    .getByRole("button", { name: "关闭并保留记录", exact: true })
    .click();
  const region = page.getByRole("region", {
    name: "服务商操作恢复",
    exact: true,
  });
  await region
    .getByRole("button", { name: "读取后台待核查操作", exact: true })
    .focus();
  await page.keyboard.press("Tab");
  await expect(
    region.getByRole("button", { name: "核查原结果", exact: true }),
  ).toBeFocused();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});
test("a newly rejected stale sequence can be retried as a new explicit action without probing the rejected attempt", async ({
  page,
}) => {
  await open(page, "operation-sequence-race");
  await submit(page);
  await expect(
    page.getByText("旧操作序号不可重放", { exact: true }),
  ).toBeVisible();
  expect(await records(page)).toHaveLength(0);
  expect(
    (await page.evaluate(() => window.providerOperationsFixture.snapshot()))
      .stores[PROVIDER_FIXTURE_STORE_ID].probes,
  ).toBe(0);
  await submit(page);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const state = await page.evaluate(() =>
    window.providerOperationsFixture.snapshot(),
  );
  expect(state.stores[PROVIDER_FIXTURE_STORE_ID].probes).toBe(1);
});
for (const field of [
  "version",
  "request",
  "provider",
  "status",
  "verification",
] as const) {
  test(`malformed ${field} receipt stays unresolved and never closes the editor`, async ({
    page,
  }) => {
    await open(page, `operation-malformed-${field}`);
    await page.evaluate((value) => {
      window.providerOperationsFixture.corrupt = value;
    }, field);
    await submit(page);
    await expect(
      page.getByText("保存结果待核查", { exact: true }),
    ).toBeVisible();
    expect(await page.evaluate(() => window.autoConnectFixture.saved)).toBe(0);
    expect(await records(page)).toHaveLength(1);
    await page.evaluate(() => {
      window.providerOperationsFixture.corrupt = null;
    });
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "核查原结果", exact: true })
      .click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(
      (await page.evaluate(() => window.providerOperationsFixture.snapshot()))
        .stores[PROVIDER_FIXTURE_STORE_ID].probes,
    ).toBe(1);
  });
}
