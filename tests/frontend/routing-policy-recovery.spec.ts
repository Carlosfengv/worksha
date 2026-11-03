import { expect, test, type Page } from "@playwright/test";

async function open(page: Page) {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/routing-policy.html");
  await page
    .getByRole("button", { name: "核查策略范围与恢复", exact: true })
    .click();
  const dialog = page.getByRole("dialog", {
    name: "策略范围核查与恢复",
    exact: true,
  });
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText("尚未读取恢复范围");
  return dialog;
}

test("legacy service identifiers remain exact through list inspection and explicit recovery", async ({
  page,
}) => {
  const dialog = await open(page);
  await page.evaluate(() => {
    window.routingPolicyFixture.recoveryLegacyIds = true;
  });
  await dialog
    .getByRole("button", { name: "读取恢复范围", exact: true })
    .click();
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  const raw = "ABCDEFAB-CDEF-4ABC-8DEF-ABCDEFABCDEF";
  await dialog
    .getByRole("row")
    .filter({ hasText: raw })
    .getByRole("button", { name: "核查此范围", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "确认使用内置策略", exact: true })
    .click();
  const confirm = page.getByRole("dialog", {
    name: "确认切回内置策略",
    exact: true,
  });
  await expect(confirm).toContainText(raw);
  await confirm
    .getByRole("button", { name: "确认恢复此范围", exact: true })
    .click();
  await expect(confirm).toHaveCount(0);
  const action = await page.evaluate(() =>
    window.routingPolicyFixture.recoveryCalls.find((c) => c.kind === "resolve"),
  );
  expect(action?.kind === "resolve" && action.reference.scope.serviceId).toBe(
    raw,
  );
});

test("a folded legacy service identity cannot replace the selected recovery reference", async ({
  page,
}) => {
  const dialog = await open(page);
  await page.evaluate(() => {
    window.routingPolicyFixture.recoveryLegacyIds = true;
    window.routingPolicyFixture.recoveryFoldedReference = true;
  });
  await dialog
    .getByRole("button", { name: "读取恢复范围", exact: true })
    .click();
  await dialog
    .getByRole("row")
    .filter({ hasText: "ABCDEFAB-CDEF-4ABC-8DEF-ABCDEFABCDEF" })
    .getByRole("button", { name: "核查此范围", exact: true })
    .click();
  await expect(dialog.getByRole("alert")).toContainText("恢复核查身份已变化");
  expect(
    await page.evaluate(() =>
      window.routingPolicyFixture.recoveryCalls.some(
        (c) => c.kind === "resolve",
      ),
    ),
  ).toBe(false);
});

test("a consistent orphan requires explicit retirement and retains its original scope", async ({
  page,
}) => {
  const dialog = await open(page);
  await page.evaluate(() => {
    window.routingPolicyFixture.recoveryConsistentOrphan = true;
  });
  await dialog
    .getByRole("button", { name: "读取恢复范围", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "核查此范围", exact: true })
    .first()
    .click();
  await expect(dialog).toContainText("服务当前不存在，可明确解除遗留策略要求");
  await dialog
    .getByRole("button", { name: "解除遗留策略要求", exact: true })
    .click();
  const confirm = page.getByRole("dialog", {
    name: "确认解除遗留策略要求",
    exact: true,
  });
  await expect(confirm).toContainText("不会创建服务或提交原发布");
  expect(
    await page.evaluate(() =>
      window.routingPolicyFixture.recoveryCalls.filter(
        (c) => c.kind === "resolve",
      ),
    ),
  ).toEqual([]);
  await confirm
    .getByRole("button", { name: "确认恢复此范围", exact: true })
    .click();
  await expect(confirm).toHaveCount(0);
  await expect(dialog).toContainText("已保存遗留范围的解除状态，原历史保留");
  expect(
    await page.evaluate(() =>
      window.routingPolicyFixture.recoveryCalls.filter(
        (c) => c.kind === "resolve",
      ),
    ),
  ).toEqual([
    {
      kind: "resolve",
      reference: {
        scope: {
          storeId: "00000000-0000-4000-8000-000000000001",
          serviceId: "00000000-0000-4000-8000-000000000002",
        },
        referenceId: "00000000-0000-4000-8000-000000000200",
      },
      mode: { kind: "native" },
    },
  ]);
});

test("recovery lists orphan scopes, inspects read-only, cancels with focus and confirms exact scope", async ({
  page,
}) => {
  const dialog = await open(page);
  expect(
    await page.evaluate(() => window.routingPolicyFixture.recoveryCalls),
  ).toEqual([]);
  await dialog
    .getByRole("button", { name: "读取恢复范围", exact: true })
    .click();
  await expect(dialog).toContainText("服务当前不存在");
  await dialog
    .getByRole("button", { name: "核查此范围", exact: true })
    .first()
    .click();
  await expect(dialog).toContainText("控制记录丢失");
  const trigger = dialog.getByRole("button", {
    name: "确认使用内置策略",
    exact: true,
  });
  await trigger.focus();
  await page.keyboard.press("Enter");
  const confirm = page.getByRole("dialog", {
    name: "确认切回内置策略",
    exact: true,
  });
  await expect(confirm).toContainText("00000000-0000-4000-8000-000000000200");
  expect(
    await page.evaluate(() =>
      window.routingPolicyFixture.recoveryCalls.some(
        (c) => c.kind === "resolve",
      ),
    ),
  ).toBe(false);
  await page.keyboard.press("Escape");
  await expect(confirm).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await page.keyboard.press("Enter");
  await confirm
    .getByRole("button", { name: "确认恢复此范围", exact: true })
    .click();
  await expect(confirm).toHaveCount(0);
  await expect(dialog).toContainText("已明确切回内置策略");
  expect(
    await page.evaluate(() =>
      window.routingPolicyFixture.recoveryCalls.filter(
        (c) => c.kind === "resolve",
      ),
    ),
  ).toEqual([
    {
      kind: "resolve",
      reference: {
        scope: {
          storeId: "00000000-0000-4000-8000-000000000001",
          serviceId: "00000000-0000-4000-8000-000000000002",
        },
        referenceId: "00000000-0000-4000-8000-000000000200",
      },
      mode: { kind: "native" },
    },
  ]);
  await dialog.getByRole("button", { name: "关闭", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "核查策略范围与恢复", exact: true }),
  ).toBeFocused();
});

test("pending restoration never publishes and uncertain receipt can only finish the same original reference", async ({
  page,
}) => {
  const dialog = await open(page);
  await page.evaluate(() => {
    window.routingPolicyFixture.recoveryPending = true;
    window.routingPolicyFixture.recoveryUnknownOnce = true;
  });
  await dialog
    .getByRole("button", { name: "读取恢复范围", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "核查此范围", exact: true })
    .first()
    .click();
  await expect(
    dialog.getByRole("button", { name: "确认使用内置策略", exact: true }),
  ).toBeDisabled();
  await dialog
    .getByRole("button", { name: "恢复原编号核查入口", exact: true })
    .click();
  const confirm = page.getByRole("dialog", {
    name: "确认恢复原编号核查入口",
    exact: true,
  });
  await confirm
    .getByRole("button", { name: "确认恢复此范围", exact: true })
    .click();
  await expect(confirm.getByRole("alert")).toContainText("恢复结果待核查");
  await expect(confirm).toContainText("00000000-0000-4000-8000-000000000200");
  await confirm
    .getByRole("button", { name: "核查并完成本次恢复", exact: true })
    .click();
  await expect(confirm).toHaveCount(0);
  await expect(dialog).toContainText("已恢复原编号核查入口");
  const calls = await page.evaluate(() => ({
    recovery: window.routingPolicyFixture.recoveryCalls,
    policy: window.routingPolicyFixture.calls,
  }));
  const resolves = calls.recovery.filter((c) => c.kind === "resolve");
  expect(resolves).toHaveLength(2);
  expect(resolves[0]).toEqual(resolves[1]);
  expect(
    calls.policy.some((c) =>
      ["publish", "resume-publish", "bind"].includes(c.kind),
    ),
  ).toBe(false);
});

test("corrupt evidence and foreign responses remain blocked with no scope mutation", async ({
  page,
}) => {
  const dialog = await open(page);
  await page.evaluate(() => {
    window.routingPolicyFixture.recoveryBlocked = true;
  });
  await dialog
    .getByRole("button", { name: "读取恢复范围", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "核查此范围", exact: true })
    .first()
    .click();
  await expect(dialog).toContainText("记录损坏或暂不可读取");
  await expect(
    dialog.getByRole("button", { name: "确认使用内置策略", exact: true }),
  ).toBeDisabled();
  await expect(
    dialog.getByRole("button", { name: "恢复原编号核查入口", exact: true }),
  ).toBeDisabled();
  await page.evaluate(() => {
    window.routingPolicyFixture.recoveryForeign = true;
  });
  await dialog
    .getByRole("button", { name: "读取恢复范围", exact: true })
    .click();
  await expect(dialog.getByRole("alert")).toContainText("恢复目录范围已变化");
  expect(
    await page.evaluate(() =>
      window.routingPolicyFixture.recoveryCalls.some(
        (c) => c.kind === "resolve",
      ),
    ),
  ).toBe(false);
});

test("backend reconnection discards a late inspection and minimum dialog scroll stays contained", async ({
  page,
}) => {
  const dialog = await open(page);
  await dialog
    .getByRole("button", { name: "读取恢复范围", exact: true })
    .click();
  await page.evaluate(() => {
    window.routingPolicyFixture.holdRecoveryInspect = true;
  });
  await dialog
    .getByRole("button", { name: "核查此范围", exact: true })
    .first()
    .click();
  await page.evaluate(() => {
    window.routingPolicyFixture.reconnect();
    window.routingPolicyFixture.releaseRecoveryInspect();
  });
  await expect(
    dialog.getByRole("region", { name: "所选恢复范围" }),
  ).toHaveCount(0);
  await expect(dialog).toContainText("尚未读取恢复范围");
  await page.evaluate(() => {
    window.routingPolicyFixture.holdRecoveryInspect = false;
  });
  await dialog
    .getByRole("button", { name: "读取恢复范围", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "核查此范围", exact: true })
    .first()
    .click();
  await dialog
    .getByRole("button", { name: "恢复原编号核查入口", exact: true })
    .focus();
  await page.keyboard.press("End");
  const layout = await dialog.evaluate((el) => {
    const r = el.getBoundingClientRect();
    return {
      left: r.left,
      right: r.right,
      top: r.top,
      bottom: r.bottom,
      overflow: document.documentElement.scrollWidth > innerWidth,
    };
  });
  expect(layout.left).toBeGreaterThanOrEqual(0);
  expect(layout.right).toBeLessThanOrEqual(720);
  expect(layout.top).toBeGreaterThanOrEqual(0);
  expect(layout.bottom).toBeLessThanOrEqual(560);
  expect(layout.overflow).toBe(false);
});
