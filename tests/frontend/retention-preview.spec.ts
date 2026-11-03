import { expect, test } from "@playwright/test";
const source = "/tests/frontend/retention-preview.html";
const first = "00000000-0000-4000-8000-000000000001";
const second = "00000000-0000-4000-8000-000000000002";

test("activity cache inventory negotiates v2 and distinguishes saved copies from native transcripts", async ({
  page,
}) => {
  await page.goto(source);
  await page.getByRole("button", { name: "预览影响" }).click();
  expect(
    await page.evaluate(
      () => window.retentionCalls[0].request.inventoryVersion,
    ),
  ).toBe(2);
  await page.evaluate(() => {
    const at = new Date().toISOString();
    window.retentionCalls[0].resolve({
      activityCaches: {
        gateway: {
          status: "checked",
          readStartedAt: at,
          readFinishedAt: at,
          value: {
            records: 4,
            olderThanCutoff: 1,
            withinWindow: 1,
            unknownTime: 2,
            fileBytes: 900,
          },
        },
        native: { status: "unverified", reason: "source_absent" },
      },
    });
  });
  const gateway = page.getByRole("region", {
    name: "网关活动摘要缓存",
    exact: true,
  });
  const native = page.getByRole("region", {
    name: "原生活动摘要缓存",
    exact: true,
  });
  await expect(
    gateway.getByText("当前保存摘要", { exact: true }),
  ).toBeVisible();
  await expect(gateway.getByText("4", { exact: true })).toBeVisible();
  await expect(
    gateway.getByText("摘要时间待核查", { exact: true }),
  ).toBeVisible();
  await expect(native.getByText(/未找到来源文件/)).toBeVisible();
  await expect(
    native.getByText(/已确认本次检查的活动摘要缓存为空/),
  ).toHaveCount(0);
  await expect(page.getByText(/原生会话文件仍不在清理范围内/)).toBeVisible();
  await expect(
    page.getByText(/冻结查询、未完成导出及其他派生副本尚未核查/),
  ).toBeVisible();
});
test("failed activity cache preserves the other checked empty copy and retries independently", async ({
  page,
}) => {
  await page.goto(source);
  await page.getByRole("button", { name: "预览影响" }).click();
  await page.evaluate(() => {
    const at = new Date().toISOString();
    window.retentionCalls[0].resolve({
      activityCaches: {
        gateway: { status: "unverified", reason: "unsupported_schema" },
        native: {
          status: "checked",
          readStartedAt: at,
          readFinishedAt: at,
          value: {
            records: 0,
            olderThanCutoff: 0,
            withinWindow: 0,
            unknownTime: 0,
            fileBytes: 23,
          },
        },
      },
    });
  });
  await expect(
    page
      .getByRole("region", { name: "网关活动摘要缓存", exact: true })
      .getByText(/来源格式尚不受支持/),
  ).toBeVisible();
  await expect(
    page
      .getByRole("region", { name: "原生活动摘要缓存", exact: true })
      .getByText(/已确认本次检查的活动摘要缓存为空/),
  ).toBeVisible();
  await expect(
    page.getByText("当前保存工具记录", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "预览影响" }).click();
  await page.evaluate(() => window.retentionCalls[1].resolve());
  await expect(page.getByText(/来源格式尚不受支持/)).toHaveCount(0);
});
for (const mode of [
  "bad-total",
  "capacity",
  "bytes",
  "content",
  "time",
  "old-format",
] as const)
  test(`activity cache contract rejects ${mode} without revealing content`, async ({
    page,
  }) => {
    await page.goto(source);
    await page.getByRole("button", { name: "预览影响" }).click();
    await page.evaluate((mode) => {
      const at = new Date().toISOString();
      const value = {
        records: mode === "capacity" ? 513 : 1,
        olderThanCutoff: mode === "bad-total" ? 0 : 1,
        withinWindow: 0,
        unknownTime: 0,
        fileBytes: mode === "bytes" ? 2097153 : 300,
        ...(mode === "content" ? { text: "private-excerpt" } : {}),
      };
      window.retentionCalls[0].resolve({
        ...(mode === "old-format"
          ? { version: 1, scope: "product_owned_saved_metadata" }
          : {}),
        activityCaches: {
          gateway: {
            status: "checked",
            readStartedAt: mode === "time" ? "2000-01-01T00:00:00Z" : at,
            readFinishedAt: at,
            value,
          },
          native: { status: "unverified", reason: "source_absent" },
        },
      } as never);
    }, mode);
    await expect(page.getByText("预览未完成", { exact: true })).toBeVisible();
    await expect(page.getByText("当前保存摘要", { exact: true })).toHaveCount(
      0,
    );
    await expect(
      page.getByText("private-excerpt", { exact: true }),
    ).toHaveCount(0);
  });
test("legacy composite client retains its v1 reply and omits version negotiation", async ({
  page,
}) => {
  await page.goto(source);
  const pending = page.evaluate(
    (first) =>
      window.retentionClient.previewRetentionInventory({
        storeId: first,
        expectedSettingsRevision: 1,
        days: 30,
      }),
    first,
  );
  await expect
    .poll(() => page.evaluate(() => window.retentionCalls.length))
    .toBe(1);
  expect(
    await page.evaluate(
      () => window.retentionCalls[0].request.inventoryVersion,
    ),
  ).toBeUndefined();
  await page.evaluate(() => window.retentionCalls[0].resolve());
  const result = await pending;
  expect(result.version).toBe(1);
  expect(result.scope).toBe("product_owned_saved_metadata");
  expect("activityCaches" in result).toBe(false);
});

test("missing tool source preserves gateway counts without claiming tool history is empty", async ({
  page,
}) => {
  await page.goto(source);
  await page.getByRole("button", { name: "预览影响" }).click();
  await page.evaluate(() =>
    window.retentionCalls[0].resolve({
      tools: { status: "unverified", reason: "source_absent" },
    }),
  );
  await expect(page.getByText("已入库尝试", { exact: true })).toBeVisible();
  await expect(page.getByText("当前保存工具记录", { exact: true })).toHaveCount(
    0,
  );
  await expect(
    page.getByText("未找到来源文件，无法确认历史是否为空。", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText(/已确认本次检查的工具保存集合为空/)).toHaveCount(
    0,
  );
});

test("a failed gateway source does not hide a checked tool collection or its prior-history gap", async ({
  page,
}) => {
  await page.goto(source);
  await page.getByRole("button", { name: "预览影响" }).click();
  await page.evaluate(() =>
    window.retentionCalls[0].resolve({
      gateway: { status: "unverified", reason: "unsupported_schema" },
    }),
  );
  await expect(page.getByText("已入库尝试", { exact: true })).toHaveCount(0);
  await expect(
    page.getByText("当前保存工具记录", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText(/不能确认曾经的完整历史/)).toBeVisible();
  await expect(page.getByText(/超时、取消和传输失败仍受保护/)).toBeVisible();
});

test("checked empty tool storage and an unreadable source are different states across retry", async ({
  page,
}) => {
  await page.goto(source);
  await page.getByRole("button", { name: "预览影响" }).click();
  await page.evaluate(() => {
    const at = new Date().toISOString();
    window.retentionCalls[0].resolve({
      tools: {
        status: "checked",
        readStartedAt: at,
        readFinishedAt: at,
        value: {
          records: 0,
          expired: 0,
          withinWindow: 0,
          protected: {
            active: 0,
            unknownOutcome: 0,
            unknownTime: 0,
            unsupportedSource: 0,
          },
          unreadableRecords: 0,
          classificationComplete: true,
          capacityLimit: 10_000,
        },
      },
    });
  });
  await expect(
    page.getByText(/已确认本次检查的工具保存集合为空/),
  ).toBeVisible();
  await page.getByRole("button", { name: "预览影响" }).click();
  await expect(page.getByText(/已确认本次检查的工具保存集合为空/)).toHaveCount(
    0,
  );
  await page.evaluate(() =>
    window.retentionCalls[1].resolve({
      tools: { status: "unverified", reason: "unavailable" },
    }),
  );
  await expect(
    page.getByText("来源读取未完成，请重试。", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("当前保存工具记录", { exact: true })).toHaveCount(
    0,
  );
});

for (const mode of [
  "bad-total",
  "unknown-reason",
  "future-capacity",
  "source-interval",
  "source-before-window",
] as const)
  test(`tool contract rejects ${mode} before presenting checked quantities`, async ({
    page,
  }) => {
    await page.goto(source);
    await page.getByRole("button", { name: "预览影响" }).click();
    await page.evaluate((mode) => {
      const at = new Date().toISOString();
      if (mode === "unknown-reason") {
        window.retentionCalls[0].resolve({
          tools: { status: "unverified", reason: "future" } as never,
        });
        return;
      }
      window.retentionCalls[0].resolve({
        tools: {
          status: "checked",
          readStartedAt:
            mode === "source-interval"
              ? "2099-01-01T00:00:00Z"
              : mode === "source-before-window"
                ? "2000-01-01T00:00:00Z"
                : at,
          readFinishedAt: at,
          value: {
            records: mode === "bad-total" ? 2 : 1,
            expired: 1,
            withinWindow: 0,
            protected: {
              active: 0,
              unknownOutcome: 0,
              unknownTime: 0,
              unsupportedSource: 0,
            },
            unreadableRecords: 0,
            classificationComplete: true,
            capacityLimit: mode === "future-capacity" ? 123 : 10_000,
          },
        } as never,
      });
    }, mode);
    await expect(page.getByText("预览未完成", { exact: true })).toBeVisible();
    await expect(
      page.getByText("当前保存工具记录", { exact: true }),
    ).toHaveCount(0);
  });

test("legacy gateway-only IPC remains readable with its original flat response", async ({
  page,
}) => {
  await page.goto(source);
  const result = page.evaluate(
    (first) =>
      window.retentionClient.previewRetention({
        storeId: first,
        expectedSettingsRevision: 1,
        days: 30,
      }),
    first,
  );
  await expect
    .poll(() => page.evaluate(() => window.retentionCalls.length))
    .toBe(1);
  await page.evaluate(() => window.retentionCalls[0].resolve());
  expect((await result).scope).toBe("committed_gateway_only");
  expect(await page.evaluate(() => window.retentionCommands)).toEqual([
    "preview_retention",
  ]);
});

test("initial state is unchecked and read-only; unavailable identity cannot scan", async ({
  page,
}) => {
  await page.goto(source);
  await expect(
    page.getByText("尚未检查影响数量。", { exact: true }),
  ).toBeVisible();
  expect(await page.evaluate(() => window.retentionCalls.length)).toBe(0);
  await expect(page.getByText(/确认入口尚未开放/)).toBeVisible();
  await page.evaluate(() => window.retentionScope(undefined, 1));
  await expect(page.getByRole("button", { name: "预览影响" })).toHaveCount(0);
  await expect(page.getByText(/本机设置身份尚未确认/)).toBeVisible();
});

test("successful preview renders logical-request coverage without enabling cleanup", async ({
  page,
}) => {
  await page.goto(source);
  await page.getByRole("button", { name: "预览影响" }).click();
  await expect(page.getByRole("status")).toContainText("正在检查");
  await expect(
    page.getByRole("combobox", { name: "预览保留期" }),
  ).toBeDisabled();
  await page.evaluate(() => window.retentionCalls[0].resolve());
  await expect(
    page.getByText("可分组的逻辑请求", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("超过所选期限", { exact: true })).toBeVisible();
  await expect(page.getByText(/此结果最多保留 5 分钟/)).toBeVisible();
  expect(await page.evaluate(() => window.retentionCommands)).toEqual([
    "preview_retention_inventory",
  ]);
});

test("selecting a different window invalidates the old preview and preserves settings", async ({
  page,
}) => {
  await page.goto(source);
  await page.getByRole("button", { name: "预览影响" }).click();
  await page.evaluate(() => window.retentionCalls[0].resolve());
  for (const days of [7, 90]) {
    await page.getByRole("combobox", { name: "预览保留期" }).click();
    await page.getByRole("option", { name: `${days} 天`, exact: true }).click();
    await expect(
      page.getByText("尚未检查影响数量。", { exact: true }),
    ).toBeVisible();
    await page.getByRole("button", { name: "预览影响" }).click();
    const index = days === 7 ? 1 : 2;
    await expect
      .poll(() =>
        page.evaluate((i) => window.retentionCalls[i]?.request.days, index),
      )
      .toBe(days);
    await page.evaluate((i) => window.retentionCalls[i].resolve(), index);
  }
  expect(await page.evaluate(() => window.retentionCommands)).toEqual([
    "preview_retention_inventory",
    "preview_retention_inventory",
    "preview_retention_inventory",
  ]);
});

test("transport failure stays unknown and a retry can confirm an empty source", async ({
  page,
}) => {
  await page.goto(source);
  await page.getByRole("button", { name: "预览影响" }).click();
  await page.evaluate(() =>
    window.retentionCalls[0].reject("retention_source_absent"),
  );
  await expect(page.getByText("预览未完成", { exact: true })).toBeVisible();
  await expect(page.getByText("已入库尝试", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "预览影响" }).click();
  await page.evaluate(() =>
    window.retentionCalls[1].resolve({
      gateway: {
        attempts: 0,
        logicalRequests: 0,
        expired: 0,
        withinWindow: 0,
        protected: {
          active: 0,
          unknownOutcome: 0,
          unknownTime: 0,
          unsupportedSource: 0,
        },
        unreadableAttempts: 0,
        classificationComplete: true,
      },
    }),
  );
  await expect(page.getByText(/已确认本次检查的网关来源为空/)).toBeVisible();
});

test("unreadable sources and queued writes remain visible blockers", async ({
  page,
}) => {
  await page.goto(source);
  await page.getByRole("button", { name: "预览影响" }).click();
  await page.evaluate(() =>
    window.retentionCalls[0].resolve({
      pendingWrites: 4,
      writerIssue: true,
      gateway: {
        attempts: 4,
        logicalRequests: 1,
        expired: 0,
        withinWindow: 0,
        protected: {
          active: 0,
          unknownOutcome: 0,
          unknownTime: 0,
          unsupportedSource: 1,
        },
        unreadableAttempts: 3,
        classificationComplete: false,
      },
    }),
  );
  await expect(
    page.getByText("覆盖范围尚不完整", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText(/已确认本次检查的网关来源为空/)).toHaveCount(0);
});

for (const change of ["store", "revision"] as const)
  test(`late response after ${change} change cannot replace current scope`, async ({
    page,
  }) => {
    await page.goto(source);
    await page.getByRole("button", { name: "预览影响" }).click();
    await page.evaluate(
      ({ change, first, second }) =>
        window.retentionScope(change === "store" ? second : first, 2),
      { change, first, second },
    );
    await expect(
      page.getByText("尚未检查影响数量。", { exact: true }),
    ).toBeVisible();
    await page.evaluate(() => window.retentionCalls[0].resolve());
    await expect(page.getByText("已入库尝试", { exact: true })).toHaveCount(0);
    await page.getByRole("button", { name: "预览影响" }).click();
    await page.evaluate(() => window.retentionCalls[1].resolve());
    await expect(page.getByText("已入库尝试", { exact: true })).toBeVisible();
  });

test("preview expiry is announced and requires a fresh scan", async ({
  page,
}) => {
  await page.clock.install();
  await page.goto(source);
  await page.getByRole("button", { name: "预览影响" }).click();
  await page.evaluate(() => window.retentionCalls[0].resolve());
  await expect(page.getByText(/此结果最多保留/)).toBeVisible();
  await page.clock.fastForward(300_001);
  await expect(page.getByText("预览已过期", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "预览影响" }).click();
  await page.evaluate(() => window.retentionCalls[1].resolve());
  await expect(page.getByText("预览已过期", { exact: true })).toHaveCount(0);
});

for (const mode of ["wrong-store", "counts", "future-enabled"] as const)
  test(`runtime contract rejects ${mode} rather than rendering unverified numbers`, async ({
    page,
  }) => {
    await page.goto(source);
    await page.getByRole("button", { name: "预览影响" }).click();
    await page.evaluate(
      ({ mode, second }) => {
        if (mode === "wrong-store")
          window.retentionCalls[0].resolve({ storeId: second });
        if (mode === "counts")
          window.retentionCalls[0].resolve({
            gateway: {
              attempts: 1,
              logicalRequests: 10,
              expired: 10,
              withinWindow: 0,
              protected: {
                active: 0,
                unknownOutcome: 0,
                unknownTime: 0,
                unsupportedSource: 0,
              },
              unreadableAttempts: 0,
              classificationComplete: true,
            },
          });
        if (mode === "future-enabled")
          window.retentionCalls[0].resolve({ cleanupEnabled: true } as never);
      },
      { mode, second },
    );
    await expect(page.getByText("预览未完成", { exact: true })).toBeVisible();
    await expect(page.getByText("已入库尝试", { exact: true })).toHaveCount(0);
  });

test("invalid request is rejected before controlled IPC", async ({ page }) => {
  await page.goto(source);
  const error = await page.evaluate(async () => {
    try {
      await window.retentionClient.previewRetention({
        storeId: "wrong",
        expectedSettingsRevision: 1,
        days: 30,
      });
      return "accepted";
    } catch (reason) {
      return reason instanceof Error ? reason.message : "unknown";
    }
  });
  expect(error).toContain("尚未通过核对");
  expect(await page.evaluate(() => window.retentionCommands)).toEqual([]);
});
