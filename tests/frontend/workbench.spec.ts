import { expect, test, type Page } from "@playwright/test";
import type {} from "./workbench-harness";

async function open(page: Page) {
  await page.clock.install();
  await page.goto("/tests/frontend/workbench.html#a");
  await expect
    .poll(() => page.evaluate(() => window.workbenchHarness.reads.length))
    .toBe(1);
  await page.evaluate(() => window.workbenchHarness.completeRead(0, {}));
  await expect(
    page.getByRole("heading", { name: "任务 a", exact: true }),
  ).toBeVisible();
}

test("迟到的轮询不会覆盖刚发送的内容，连续点击只发送一次", async ({ page }) => {
  await open(page);
  await page.clock.runFor(1500);
  await expect
    .poll(() => page.evaluate(() => window.workbenchHarness.reads.length))
    .toBe(2);
  await page.getByRole("textbox", { name: "继续对话" }).fill("新消息");
  await page.getByRole("button", { name: "发送", exact: true }).dblclick();
  await expect
    .poll(() => page.evaluate(() => window.workbenchHarness.operations.length))
    .toBe(1);
  await page.evaluate(() => {
    const turnId = window.workbenchHarness.operations[0].action.turnId;
    window.workbenchHarness.completeOperation(0, {
      revision: 3,
      phase: "running",
      turnId,
      parts: [{ id: turnId, kind: "user", text: "新消息", truncated: false }],
    });
    window.workbenchHarness.completeRead(1, { revision: 2 });
  });
  await expect(page.getByText("新消息", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "停止本轮", exact: true }),
  ).toBeEnabled();
});

test("离开会话后，原任务迟到的发送结果不会替换新会话", async ({ page }) => {
  await open(page);
  await page.getByRole("textbox", { name: "继续对话" }).fill("发给 a");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await page.evaluate(() => {
    location.hash = "b";
  });
  await expect
    .poll(() =>
      page.evaluate(() => window.workbenchHarness.reads.at(-1)?.taskId),
    )
    .toBe("b");
  await page.evaluate(() => {
    window.workbenchHarness.completeRead(1, {});
    window.workbenchHarness.completeOperation(0, {
      revision: 100,
      phase: "running",
    });
  });
  await expect(
    page.getByRole("heading", { name: "任务 b", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("历史回复 b", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "停止本轮", exact: true }),
  ).toHaveCount(0);
});

test("审批绑定原轮次且只提交一次，旧审批不会被迟到状态恢复", async ({
  page,
}) => {
  await open(page);
  await page.clock.runFor(1500);
  await expect
    .poll(() => page.evaluate(() => window.workbenchHarness.reads.length))
    .toBe(2);
  await page.evaluate(() =>
    window.workbenchHarness.completeRead(
      1,
      {
        revision: 4,
        phase: "approval",
        approval: {
          id: "approval-a",
          tool: "bash",
          targets: ["./temporary.sh"],
        },
      },
      "a",
    ),
  );
  await page.clock.runFor(280);
  await expect
    .poll(() => page.evaluate(() => window.workbenchHarness.reads.length))
    .toBe(3);
  await page.getByRole("button", { name: "拒绝", exact: true }).dblclick();
  expect(
    await page.evaluate(() =>
      window.workbenchHarness.operations.map((item) => item.action),
    ),
  ).toEqual([
    {
      taskId: "a",
      turnId: "turn-a",
      kind: "decide",
      approvalId: "approval-a",
      allow: false,
    },
  ]);
  await page.evaluate(() => {
    window.workbenchHarness.completeOperation(0, {
      revision: 5,
      phase: "running",
    });
    window.workbenchHarness.completeRead(
      2,
      {
        revision: 4,
        phase: "approval",
        approval: {
          id: "approval-a",
          tool: "bash",
          targets: ["./temporary.sh"],
        },
      },
      "a",
    );
  });
  await expect(
    page.getByRole("button", { name: "拒绝", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "停止本轮", exact: true }).click();
  expect(
    await page.evaluate(() => window.workbenchHarness.operations[1].action),
  ).toEqual({ taskId: "a", turnId: "turn-a", kind: "stop" });
  await page.evaluate(() =>
    window.workbenchHarness.completeOperation(1, {
      revision: 6,
      phase: "stopped",
    }),
  );
  await page.getByRole("textbox", { name: "继续对话" }).fill("停止后继续");
  await expect(
    page.getByRole("button", { name: "发送", exact: true }),
  ).toBeEnabled();
});

test("发送结果不确定时保留草稿，重试沿用原操作编号", async ({ page }) => {
  await open(page);
  await page.getByRole("textbox", { name: "继续对话" }).fill("保留原消息");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await page.evaluate(() => window.workbenchHarness.failOperation(0, true));
  await expect(page.getByRole("alert")).toContainText("发送结果待核对");
  await expect(page.getByRole("textbox", { name: "继续对话" })).toHaveValue(
    "保留原消息",
  );
  await page.getByRole("button", { name: "发送", exact: true }).click();
  const actions = await page.evaluate(() =>
    window.workbenchHarness.operations.map((item) => item.action),
  );
  expect(actions).toHaveLength(2);
  expect(actions[1]).toEqual(actions[0]);
});
