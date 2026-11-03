import { expect, test } from "@playwright/test";
import { writeFileSync } from "node:fs";

test("long conversation paging keeps mounted and retained bodies bounded", async ({
  page,
}, info) => {
  await page.goto("/tests/frontend/reading-capacity.html");
  await expect(
    page.getByRole("article", { name: "page-0-event-1", exact: true }),
  ).toBeVisible();
  for (let n = 1; n < 8; n++) {
    await page.getByRole("button", { name: /加载后续对话|下一页对话/ }).click();
    await expect(
      page.getByRole("article", { name: `page-${n}-event-1`, exact: true }),
    ).toBeAttached();
  }
  const result = await page.evaluate(() => ({
    ...window.readingCapacity.measure(),
    articles: document.querySelectorAll("article").length,
    domNodes: document.querySelectorAll("*").length,
    calls: window.readingCapacity.reads.length,
  }));
  writeFileSync(
    process.env.READING_CAPACITY_REPORT ?? info.outputPath("capacity.json"),
    JSON.stringify(result, null, 2),
  );
  expect(result.articles).toBeLessThanOrEqual(40);
  expect(result.events).toBeLessThanOrEqual(40);
});

test("backward navigation restores expanded details, scroll and keyboard reading focus", async ({
  page,
}) => {
  await page.goto("/tests/frontend/reading-capacity.html?words=8");
  const region = page.getByLabel("对话滚动区域", { exact: true });
  const detail = page
    .getByRole("article", { name: "page-0-event-0", exact: true })
    .getByRole("button", { name: "参数与结果详情" });
  await detail.click();
  await expect(detail).toHaveAttribute("aria-expanded", "true");
  await region.click({ position: { x: 8, y: 8 } });
  await region.evaluate((node) => {
    node.scrollTop = 500;
  });
  await expect.poll(() => region.evaluate((node) => node.scrollTop)).toBe(500);
  await page.getByRole("button", { name: "下一页对话", exact: true }).click();
  await expect(
    page.getByRole("article", { name: "page-0-event-1", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("article", { name: "page-1-event-1", exact: true }),
  ).toBeAttached();
  const back = page.getByRole("button", { name: "上一页对话", exact: true });
  await back.focus();
  await back.press("Enter");
  await expect(detail).toHaveAttribute("aria-expanded", "true");
  await expect(region).toBeFocused();
  await expect.poll(() => region.evaluate((node) => node.scrollTop)).toBe(500);
});

test("body eviction preserves a session's cursor and reading position across ten sessions", async ({
  page,
}) => {
  await page.goto("/tests/frontend/reading-capacity.html");
  for (let n = 0; n < 2; n++) {
    await page.getByRole("button", { name: "下一页对话", exact: true }).click();
    await expect(
      page.getByRole("article", { name: `page-${n + 1}-event-1`, exact: true }),
    ).toBeAttached();
  }
  const region = page.getByLabel("对话滚动区域", { exact: true });
  await region.evaluate((node) => {
    node.scrollTop = 700;
  });
  await expect.poll(() => region.evaluate((node) => node.scrollTop)).toBe(700);
  for (let n = 1; n < 10; n++) {
    await page
      .getByRole("button", { name: `session-${n}`, exact: true })
      .click();
    await expect(
      page.getByRole("heading", { name: `claude:fixture-${n}`, exact: true }),
    ).toBeVisible();
    const kept = await page.evaluate(() => window.readingCapacity.measure());
    expect(kept.estimatedBytes).toBeLessThanOrEqual(8 * 1024 * 1024);
    expect(kept.events).toBeLessThanOrEqual(400);
  }
  await page.getByRole("button", { name: "session-0", exact: true }).click();
  await expect(
    page.getByRole("article", { name: "page-2-event-1", exact: true }),
  ).toBeAttached();
  await expect.poll(() => region.evaluate((node) => node.scrollTop)).toBe(700);
  const reads = await page.evaluate(() => window.readingCapacity.reads);
  expect(reads.at(-1)).toEqual({
    id: "claude:fixture-0",
    cursor: "snapshot:2",
    anchor: undefined,
  });
});

test("an oversized page remains readable without bypassing the retained-body budget", async ({
  page,
}) => {
  await page.goto("/tests/frontend/reading-capacity.html?words=5500");
  await expect(page.getByRole("article")).toHaveCount(40);
  expect(
    (await page.evaluate(() => window.readingCapacity.measure())).bodies,
  ).toBe(0);
  await page
    .getByRole("button", { name: "toggle-reader", exact: true })
    .click();
  await expect(page.getByRole("article")).toHaveCount(0);
  await page
    .getByRole("button", { name: "toggle-reader", exact: true })
    .click();
  await expect(page.getByRole("article")).toHaveCount(40);
  expect(await page.evaluate(() => window.readingCapacity.reads.length)).toBe(
    2,
  );
});

for (const code of ["unauthorized", "not_found", "stale_snapshot"]) {
  test(`${code} on paging removes the previous readable body and cached position`, async ({
    page,
  }) => {
    await page.goto("/tests/frontend/reading-capacity.html?words=1");
    await expect(page.getByRole("article")).toHaveCount(40);
    await page.evaluate((code) => {
      window.readingCapacity.error = code;
    }, code);
    await page.getByRole("button", { name: "下一页对话", exact: true }).click();
    await expect(page.getByRole("article")).toHaveCount(0);
    expect(
      (await page.evaluate(() => window.readingCapacity.measure())).sessions,
    ).toBe(0);
    await page.evaluate(() => {
      window.readingCapacity.error = null;
    });
    await page.getByRole("button", { name: "刷新对话", exact: true }).click();
    await expect(page.getByRole("article")).toHaveCount(40);
    await expect(
      page.getByRole("button", { name: "上一页对话", exact: true }),
    ).toBeDisabled();
  });
}

test("a late page after connection recovery cannot reappear or refill the cache", async ({
  page,
}) => {
  await page.goto("/tests/frontend/reading-capacity.html?words=1");
  await expect(page.getByRole("article")).toHaveCount(40);
  await page.evaluate(() => {
    window.readingCapacity.hold = true;
  });
  await page.getByRole("button", { name: "下一页对话", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => Boolean(window.readingCapacity.release)))
    .toBe(true);
  await page.evaluate(() => {
    window.readingCapacity.hold = false;
    window.readingCapacity.recover();
  });
  await expect(page.getByText(/source-1 synthetic/)).toHaveCount(39);
  await page.evaluate(() => window.readingCapacity.release?.());
  await page
    .getByRole("button", { name: "toggle-reader", exact: true })
    .click();
  await page
    .getByRole("button", { name: "toggle-reader", exact: true })
    .click();
  await expect(page.getByText(/source-1 synthetic/)).toHaveCount(39);
  await expect(
    page.getByRole("article", { name: "page-1-event-1", exact: true }),
  ).toHaveCount(0);
  expect(await page.evaluate(() => window.readingCapacity.reads.length)).toBe(
    3,
  );
});

test("an anchor opens its page and returning preserves later paging rather than jumping back", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/reading-capacity.html?words=1&anchor=claude:fixture-0-page-7-event-0",
  );
  const anchored = page.getByRole("article", {
    name: "page-7-event-0",
    exact: true,
  });
  await expect(
    anchored.getByRole("button", { name: "参数与结果详情" }),
  ).toHaveAttribute("aria-expanded", "true");
  await page.getByRole("button", { name: "下一页对话", exact: true }).click();
  await expect(
    page.getByRole("article", { name: "page-8-event-1", exact: true }),
  ).toBeAttached();
  await page
    .getByRole("button", { name: "toggle-reader", exact: true })
    .click();
  await page
    .getByRole("button", { name: "toggle-reader", exact: true })
    .click();
  await expect(
    page.getByRole("article", { name: "page-8-event-1", exact: true }),
  ).toBeAttached();
  await page.getByRole("button", { name: "上一页对话", exact: true }).click();
  await expect(
    anchored.getByRole("button", { name: "参数与结果详情" }),
  ).toHaveAttribute("aria-expanded", "true");
});

test("a pending page preserves tab changes and the departing page's later expanded state", async ({
  page,
}) => {
  await page.goto("/tests/frontend/reading-capacity.html?words=1");
  await expect(page.getByRole("article")).toHaveCount(40);
  await page.evaluate(() => {
    window.readingCapacity.hold = true;
  });
  await page.getByRole("button", { name: "下一页对话", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => Boolean(window.readingCapacity.release)))
    .toBe(true);
  await page.getByRole("tab", { name: "调用时间线", exact: true }).click();
  await page
    .getByRole("article", { name: "page-0-event-0", exact: true })
    .getByRole("button", { name: "参数与结果详情" })
    .click();
  await page.evaluate(() => {
    window.readingCapacity.hold = false;
    window.readingCapacity.release?.();
  });
  await expect(
    page.getByRole("article", { name: "page-1-event-0", exact: true }),
  ).toBeAttached();
  await expect(
    page.getByRole("tab", { name: "调用时间线", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await page.getByRole("button", { name: "上一页对话", exact: true }).click();
  await expect(
    page
      .getByRole("article", { name: "page-0-event-0", exact: true })
      .getByRole("button", { name: "参数与结果详情" }),
  ).toHaveAttribute("aria-expanded", "true");
});

test("the timeline shares bounded paging and restart remains available beyond the back trail", async ({
  page,
}) => {
  test.setTimeout(90000);
  await page.goto("/tests/frontend/reading-capacity.html?words=1&pages=110");
  await page.getByRole("tab", { name: "调用时间线", exact: true }).click();
  for (let n = 1; n < 103; n++) {
    await page.getByRole("button", { name: "下一页对话", exact: true }).click();
    await expect(
      page.getByRole("article", { name: `page-${n}-event-0`, exact: true }),
    ).toBeAttached();
  }
  expect(
    (await page.evaluate(() => window.readingCapacity.measure())).trail,
  ).toBe(100);
  await expect(page.getByRole("article")).toHaveCount(1);
  await page
    .getByRole("button", { name: "从对话开头阅读", exact: true })
    .click();
  await expect(
    page.getByRole("article", { name: "page-0-event-0", exact: true }),
  ).toBeAttached();
  await expect(
    page.getByRole("tab", { name: "调用时间线", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(
    page.getByRole("button", { name: "上一页对话", exact: true }),
  ).toBeDisabled();
});
