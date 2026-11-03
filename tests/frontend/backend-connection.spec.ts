import { test, expect } from "@playwright/test";
import type {} from "./backend-connection-harness";

const open = (page: import("@playwright/test").Page, mode = "ready") =>
  page.goto("/tests/frontend/backend-connection.html?mode=" + mode);
test("startup failure can be explicitly reconnected without replaying any business command", async ({
  page,
}) => {
  await open(page);
  await expect(page.getByText("启动未完成", { exact: true })).toBeVisible();
  const button = page.getByRole("button", {
    name: "核对并重新连接",
    exact: true,
  });
  await button.evaluate((b: HTMLButtonElement) => {
    b.click();
    b.click();
  });
  await expect
    .poll(() => page.evaluate(() => window.backendFixture.pending.length))
    .toBe(1);
  await expect(button).toBeDisabled();
  await page.evaluate(() => window.backendFixture.pending[0]());
  await expect(
    page.getByText("后台连接已通过身份核对。数据读取状态以页面提示为准。", {
      exact: true,
    }),
  ).toBeVisible();
  await expect
    .poll(() => page.evaluate(() => window.backendFixture.refreshes))
    .toBe(1);
  const calls = await page.evaluate(() => window.backendFixture.calls);
  expect(calls.filter((n) => n === "reconnect_backend")).toHaveLength(1);
  expect(
    calls.every(
      (n) => n === "get_backend_diagnostics" || n === "reconnect_backend",
    ),
  ).toBe(true);
});
test("rejected or unconfirmed identity never refreshes business data", async ({
  page,
}) => {
  for (const mode of ["rejected", "unconfirmed"]) {
    await open(page, mode);
    await page
      .getByRole("button", { name: "核对并重新连接", exact: true })
      .click();
    await page.evaluate(() => window.backendFixture.pending[0]());
    await expect(
      page.getByText(
        mode === "rejected"
          ? "后台身份不匹配，未连接。"
          : "后台连接尚未确认，请检查连接状态。",
        { exact: true },
      ),
    ).toBeVisible();
    expect(await page.evaluate(() => window.backendFixture.refreshes)).toBe(0);
    await expect(
      page.getByText("后台连接已通过身份核对。数据读取状态以页面提示为准。", {
        exact: true,
      }),
    ).toHaveCount(0);
  }
});
test("unmounting while verification runs does not refresh or issue follow-up commands", async ({
  page,
}) => {
  await open(page);
  await page
    .getByRole("button", { name: "核对并重新连接", exact: true })
    .click();
  await page.getByRole("button", { name: "切换挂载", exact: true }).click();
  const count = await page.evaluate(() => window.backendFixture.calls.length);
  await page.evaluate(async () => {
    window.backendFixture.pending[0]();
    await new Promise(requestAnimationFrame);
  });
  expect(await page.evaluate(() => window.backendFixture.refreshes)).toBe(0);
  expect(await page.evaluate(() => window.backendFixture.calls.length)).toBe(
    count,
  );
});
test("a confirmed connection is retained when the subsequent data refresh fails", async ({
  page,
}) => {
  await open(page, "refresh-error");
  await page
    .getByRole("button", { name: "核对并重新连接", exact: true })
    .click();
  await page.evaluate(() => window.backendFixture.pending[0]());
  await expect(
    page.getByText("页面数据刷新未完成，请再次刷新数据。", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByText("后台连接已通过身份核对。数据读取状态以页面提示为准。", {
      exact: true,
    }),
  ).toBeVisible();
  expect(await page.evaluate(() => window.backendFixture.refreshes)).toBe(1);
});
test("late diagnostics cannot overwrite the post-reconnect status and closing cannot reconnect", async ({
  page,
}) => {
  await open(page, "old-read");
  await expect(page.getByText("启动未完成", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "刷新诊断记录", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => !!window.backendFixture.oldRead))
    .toBe(true);
  await page
    .getByRole("button", { name: "核对并重新连接", exact: true })
    .click();
  await page.evaluate(() => window.backendFixture.pending[0]());
  await expect(
    page.getByText("后台连接已通过身份核对。数据读取状态以页面提示为准。", {
      exact: true,
    }),
  ).toBeVisible();
  await page.evaluate(() => window.backendFixture.oldRead?.());
  await expect(page.getByText(/最近已确认连接/)).toBeVisible();
  await open(page, "closing");
  await expect(page.getByText("正在退出", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "核对并重新连接", exact: true }),
  ).toBeDisabled();
});

test("the actual application exposes connection diagnostics when its first snapshot fails", async ({
  page,
}) => {
  await page.goto("/tests/frontend/render-boundary.html?app#/providers");
  await page.getByRole("button", { name: "查看连接诊断", exact: true }).click();
  await expect(page).toHaveURL(/#\/settings$/);
  await expect(
    page.getByRole("region", { name: "本地服务连接", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("设置暂不可用", { exact: true })).toBeVisible();
});
test("malformed diagnostics never expose unexpected payloads; records paginate and fit narrow dark layout", async ({
  page,
}) => {
  await open(page, "malformed");
  await expect(
    page.getByText("本地连接诊断回复无法确认，请重新检查状态。", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(page.locator("body")).not.toContainText(
    "private-payload-sentinel",
  );
  await open(page);
  await page.setViewportSize({ width: 960, height: 800 });
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  await page
    .getByRole("button", { name: "最近诊断记录", exact: true })
    .press("Enter");
  const table = page.getByLabel("最近后台连接诊断", { exact: true });
  await expect(table.getByRole("row")).toHaveCount(11);
  await expect(
    table.getByText("退出码：7；信号：未提供", { exact: true }),
  ).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth + 1,
      ),
    )
    .toBe(true);
  await expect
    .poll(() =>
      table.evaluate((node) => {
        const region = node.closest('[role="region"]');
        return (
          !!region &&
          region.getBoundingClientRect().bottom + 1 >=
            node.getBoundingClientRect().bottom
        );
      }),
    )
    .toBe(true);
});
test("runtime diagnostics validation bounds events, identities, exit values and state", async ({
  page,
}) => {
  await open(page);
  const results = await page.evaluate(() => {
    const base = {
      state: "connected",
      verifiedAt: 100,
      canReconnect: true,
      events: [],
    };
    const event = {
      id: crypto.randomUUID(),
      operationId: crypto.randomUUID(),
      at: 100,
      phase: "process",
      category: "child_exited",
      exitCode: 7,
      signal: null,
    };
    return [
      { ...base, verifiedAt: null },
      { ...base, events: Array(33).fill(event) },
      { ...base, events: [event, event] },
      { ...base, events: [{ ...event, exitCode: 2147483648 }] },
      { ...base, events: [{ ...event, operationId: "bad" }] },
      { ...base, events: [{ ...event, category: "raw-payload" }] },
      { ...base, state: "stopped" },
    ].map((v) => {
      try {
        window.parseBackendFixture(v);
        return false;
      } catch {
        return true;
      }
    });
  });
  expect(results.every(Boolean)).toBe(true);
});
