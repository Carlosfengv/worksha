import { expect, test, type Page } from "@playwright/test";
import { writeFileSync } from "node:fs";
import path from "node:path";

const fixture = "/tests/frontend/session-navigation.html";
const list = (page: Page) =>
  page.getByLabel("会话列表滚动区域", { exact: true });
const next = (page: Page) =>
  page.getByRole("button", { name: "下一页会话", exact: true });
const previous = (page: Page) =>
  page.getByRole("button", { name: "上一页会话", exact: true });
const links = (page: Page) =>
  list(page).getByRole("link", { name: /^Session / });
const item = (page: Page, index: number, revision?: number) =>
  list(page).getByRole("link", {
    name: `Session ${index}${revision ? ` revision-${revision}` : ""}`,
    exact: true,
  });
const detail = (page: Page, index: number) =>
  page.getByRole("heading", { name: `Session ${index}`, exact: true });
const route = (index: number) => `#/sessions/claude%3Asession-${index}`;
const scope = (page: Page) =>
  page.getByRole("navigation", { name: "会话分页", exact: true });
async function openList(page: Page, url = fixture) {
  await page.goto(url);
  const time = page.getByRole("tab", { name: "按时间排序", exact: true });
  await expect(time).toBeVisible();
  if ((await time.getAttribute("aria-selected")) !== "true") await time.click();
}

test("an exact request link opens the anchored call in the timeline and keeps its return route", async ({
  page,
}) => {
  const anchor = "claude:session-0:event-2";
  const search = new URLSearchParams({
    anchor,
    view: "timeline",
    returnTo: "/requests/request-001?range=all",
  });
  await page.goto(
    `/tests/frontend/session-navigation.html?trace=1#/sessions/claude%3Asession-0?${search}`,
  );
  const timeline = page.getByLabel("调用时间线滚动区域");
  await expect(
    timeline.getByText("关联模型响应", { exact: true }),
  ).toBeVisible();
  await expect(timeline.getByText("耗时未知")).toBeVisible();
  await expect(timeline.getByText("message", { exact: true })).toHaveCount(0);
  expect(await page.evaluate(() => window.sessionNav.anchors)).toEqual([
    anchor,
  ]);
  await page.getByRole("button", { name: "返回来源请求" }).click();
  await expect(page).toHaveURL(/#\/requests\/request-001\?range=all$/);
});

test("gateway sessions share the title list and open requests without native consent", async ({
  page,
}) => {
  await openList(page, fixture + "?gateway=1#/sessions");
  await expect(item(page, 0)).toBeVisible();
  const observed = list(page).getByRole("link", {
    name: "Codex 测试会话",
    exact: true,
  });
  await expect(observed).toBeVisible();
  await expect(page.getByLabel("会话来源范围")).toHaveCount(0);
  await observed.click();
  await expect(
    page.getByLabel("会话模型请求").getByText("fixture-gateway-model"),
  ).toBeVisible();
  expect(await page.evaluate(() => window.sessionNav.conversations)).toEqual(
    [],
  );
});

test("eight session pages keep title navigation bounded", async ({
  page,
}, info) => {
  await openList(page);
  await expect(item(page, 0)).toBeVisible();
  for (let n = 1; n < 8; n++) {
    await next(page).click();
    await expect(item(page, n * 40)).toBeAttached();
    await expect(links(page)).toHaveCount(40);
  }
  const result = await list(page).evaluate((node) => ({
    rows: node.querySelectorAll("tbody tr").length,
    domNodes: document.querySelectorAll("*").length,
  }));
  writeFileSync(
    process.env.SESSION_NAV_REPORT ?? info.outputPath("navigation.json"),
    JSON.stringify(result, null, 2),
  );
  expect(result.rows).toBe(40);
});

test("the reader opens explicitly and returning preserves the current session page", async ({
  page,
}) => {
  await openList(page);
  await expect(item(page, 0)).toBeVisible();
  await next(page).click();
  await expect(item(page, 40)).toBeVisible();
  expect(await page.evaluate(() => window.sessionNav.conversations)).toEqual(
    [],
  );
  await item(page, 40).click();
  await expect(detail(page, 40)).toBeVisible();
  await expect(list(page)).toHaveCount(0);
  await page.goBack();
  await expect(item(page, 40)).toBeVisible();
  await scope(page)
    .getByRole("status")
    .filter({ hasText: "本页 40 个 · 共 10,000 个" })
    .waitFor();
  await next(page).press("Enter");
  await expect(item(page, 80)).toBeVisible();
  await expect(list(page)).toBeFocused();
  await previous(page).press("Enter");
  await expect(item(page, 40)).toBeVisible();
  expect(await page.evaluate(() => window.sessionNav.conversations)).toEqual([
    "claude:session-40",
  ]);
});

test("filter and page survive reading a result and empty search clears old rows", async ({
  page,
}) => {
  await openList(page);
  const search = page.getByRole("textbox", { name: "搜索会话", exact: true });
  await search.fill("filter");
  await expect(item(page, 5000)).toBeVisible();
  await next(page).click();
  await expect(item(page, 5040)).toBeVisible();
  await item(page, 5040).click();
  await expect(detail(page, 5040)).toBeVisible();
  await page.goBack();
  await expect(search).toHaveValue("filter");
  await expect(item(page, 5040)).toBeVisible();
  await search.fill("none");
  await expect(links(page)).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "没有匹配会话", exact: true }),
  ).toBeVisible();
  await search.fill("");
  await expect(item(page, 0)).toBeVisible();
  await expect(previous(page)).toBeDisabled();
});

test("grouping preserves this page's identities, expanded projects and independent scroll on return", async ({
  page,
}) => {
  await openList(page);
  await next(page).click();
  await expect(item(page, 40)).toBeVisible();
  await expect(links(page)).toHaveCount(40);
  const flat = await links(page).allTextContents();
  await page.getByRole("tab", { name: "按项目分组", exact: true }).click();
  const groups = list(page).getByRole("button", { name: / · 本页 \d+ 个/ });
  await expect(groups).toHaveCount(4);
  for (const group of await groups.all()) await group.click();
  await expect(links(page)).toHaveCount(40);
  expect((await links(page).allTextContents()).sort()).toEqual(flat.sort());
  await list(page).click({ position: { x: 8, y: 8 } });
  await list(page).evaluate((node) => {
    node.scrollTop = 450;
  });
  await expect
    .poll(() => list(page).evaluate((node) => node.scrollTop))
    .toBe(450);
  await next(page).click();
  await expect(links(page)).toHaveCount(0);
  await previous(page).click();
  await expect(links(page)).toHaveCount(40);
  await expect
    .poll(() => list(page).evaluate((node) => node.scrollTop))
    .toBe(450);
});

test("a deep link opens the exact old session without choosing a similarly named row", async ({
  page,
}) => {
  await openList(page);
  await item(page, 30).scrollIntoViewIfNeeded();
  await page.evaluate((hash) => {
    location.hash = hash;
  }, route(9999));
  await expect(detail(page, 9999)).toBeVisible();
  await expect(detail(page, 9)).toHaveCount(0);
  expect(await page.evaluate(() => window.sessionNav.conversations)).toEqual([
    "claude:session-9999",
  ]);
  await page.goBack();
  await expect(item(page, 9)).toBeAttached();
});

test("320px session list paginates and opens the reader by keyboard", async ({
  page,
}) => {
  await page.setViewportSize({ width: 320, height: 700 });
  await openList(page);
  await next(page).press("Enter");
  await expect(item(page, 40)).toBeVisible();
  await expect(list(page)).toBeFocused();
  await item(page, 40).press("Enter");
  await expect(detail(page, 40)).toBeVisible();
  await expect(list(page)).toHaveCount(0);
  await page.goBack();
  await expect(item(page, 40)).toBeVisible();
});

test("transient page errors preserve the current page and retry without advancing the trail", async ({
  page,
}) => {
  await openList(page);
  await expect(item(page, 0)).toBeVisible();
  await page.evaluate(() => {
    window.sessionNav.error = "unavailable";
  });
  await next(page).click();
  await expect(page.getByRole("alert")).toContainText(
    "synthetic list unavailable",
  );
  await expect(item(page, 0)).toBeVisible();
  await expect(previous(page)).toBeDisabled();
  await page.evaluate(() => {
    window.sessionNav.error = null;
  });
  await next(page).click();
  await expect(item(page, 40)).toBeAttached();
  await expect(page.getByRole("alert")).toHaveCount(0);
});

for (const code of ["unauthorized", "stale_snapshot", "not_found"]) {
  test(`${code} discards old titles until a verified reload`, async ({
    page,
  }) => {
    await openList(page);
    await expect(item(page, 0)).toBeVisible();
    await page.evaluate((code) => {
      window.sessionNav.error = code;
    }, code);
    await next(page).click();
    await expect(page.getByRole("alert")).toContainText(
      "synthetic list unavailable",
    );
    await expect(links(page)).toHaveCount(0);
    await expect(
      page.getByRole("heading", { name: "没有匹配会话", exact: true }),
    ).toHaveCount(0);
    await page.evaluate(() => {
      window.sessionNav.error = null;
    });
    await page.getByRole("button", { name: "刷新列表", exact: true }).click();
    await expect(item(page, 0)).toBeVisible();
    expect(await page.evaluate(() => window.sessionNav.conversations)).toEqual(
      [],
    );
  });
}

test("a pending old page cannot survive connection recovery or repopulate the new title set", async ({
  page,
}) => {
  await openList(page);
  await expect(item(page, 0)).toBeVisible();
  await page.evaluate(() => {
    window.sessionNav.hold = true;
  });
  await next(page).click();
  await expect
    .poll(() => page.evaluate(() => window.sessionNav.releases.length))
    .toBe(1);
  await page.evaluate(() => window.sessionNav.recover());
  await expect(links(page)).toHaveCount(0);
  await page.evaluate(() => {
    window.sessionNav.hold = false;
    window.sessionNav.releases[0]();
  });
  await expect(item(page, 0, 1)).toBeVisible();
  await expect(item(page, 40)).toHaveCount(0);
  await expect(previous(page)).toBeDisabled();
});

test("Agent detail keeps the first five recent sessions while its paged list stays bounded", async ({
  page,
}) => {
  await page.goto("/tests/frontend/session-navigation.html?agent=1");
  await expect(page.getByRole("button", { name: /^Session / })).toHaveCount(40);
  await page.getByRole("button", { name: "下一页会话", exact: true }).click();
  await expect(
    page.getByRole("button", { name: /^Session 40 / }),
  ).toBeAttached();
  await expect(page.getByRole("button", { name: /^Session / })).toHaveCount(40);
  await page.evaluate(() => {
    location.hash = "/agents/claude-code-cli?tab=overview";
  });
  await expect(page.getByRole("button", { name: /^Session / })).toHaveCount(5);
  await expect(page.getByRole("button", { name: /^Session 0 / })).toBeVisible();
  await page.evaluate(() => {
    location.hash = "/agents/claude-code-cli?tab=sessions";
  });
  await expect(
    page.getByRole("button", { name: /^Session 40 / }),
  ).toBeAttached();
});

test("an external session outside a search relaxes that conflicting filter without choosing a fallback row", async ({
  page,
}) => {
  await openList(page);
  const search = page.getByRole("textbox", { name: "搜索会话", exact: true });
  await search.fill("filter");
  await expect(item(page, 5000)).toBeVisible();
  await page.evaluate((hash) => {
    location.hash = hash;
  }, route(9999));
  await expect(detail(page, 9999)).toBeVisible();
  expect(await page.evaluate(() => window.sessionNav.conversations)).toEqual([
    "claude:session-9999",
  ]);
  await page.goBack();
  await expect(search).toHaveValue("");
  await expect(item(page, 0)).toBeVisible();
});

test("a confirmed empty refresh clears old rows and opens no implicit selection", async ({
  page,
}) => {
  await page.clock.install();
  await openList(page);
  await expect(item(page, 0)).toBeVisible();
  await page.evaluate(() => {
    window.sessionNav.empty = true;
  });
  await page.clock.fastForward(5000);
  await expect(links(page)).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "没有匹配会话", exact: true }),
  ).toBeVisible();
  expect(await page.evaluate(() => window.sessionNav.conversations)).toEqual(
    [],
  );
});

test("a partial empty source is visibly unverified rather than a complete zero-history claim", async ({
  page,
}) => {
  await page.clock.install();
  await openList(page);
  await expect(item(page, 0)).toBeVisible();
  await page.evaluate(() => {
    window.sessionNav.empty = true;
    window.sessionNav.partial = true;
  });
  await page.clock.fastForward(5000);
  await expect(
    page.getByRole("heading", { name: "会话列表尚未完成核查", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "没有匹配会话", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByText("部分来源尚未完成核查，数量仅含已读取记录。", {
      exact: true,
    }),
  ).toBeVisible();
});

test("a disconnected source clears the old body without pretending it has no history", async ({
  page,
}) => {
  await page.clock.install();
  await page.goto(fixture + route(0));
  await expect(detail(page, 0)).toBeVisible();
  await page.evaluate(() => {
    window.sessionNav.connected = false;
  });
  await page.clock.fastForward(5000);
  await expect(detail(page, 0)).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "会话暂不可读", exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    location.hash = "/sessions";
  });
  await expect(
    page.getByRole("heading", { name: "尚未读取会话", exact: true }),
  ).toBeVisible();
  await expect(
    scope(page).getByRole("status").filter({ hasText: "会话数量尚未核查" }),
  ).toBeVisible();
});

test("first-page polling preserves list scroll and the focused stable session", async ({
  page,
}) => {
  await page.clock.install();
  await openList(page);
  await expect(item(page, 20)).toBeAttached();
  await list(page).click({ position: { x: 8, y: 8 } });
  await item(page, 20).focus();
  await list(page).evaluate((node) => {
    node.scrollTop = 450;
  });
  await expect
    .poll(() => list(page).evaluate((node) => node.scrollTop))
    .toBe(450);
  await page.clock.fastForward(5000);
  await expect
    .poll(() => page.evaluate(() => window.sessionNav.queries.length))
    .toBe(2);
  await expect(item(page, 20)).toBeFocused();
  await expect
    .poll(() => list(page).evaluate((node) => node.scrollTop))
    .toBe(450);
});

test("navigation controls fit desktop and narrow lists in light and dark layouts", async ({
  page,
}, info) => {
  for (const width of [1280, 320]) {
    for (const dark of [false, true]) {
      await page.setViewportSize({ width, height: width === 320 ? 700 : 860 });
      await page.emulateMedia({ colorScheme: dark ? "dark" : "light" });
      await openList(page);
      await expect(next(page)).toBeEnabled();
      await next(page).click();
      await expect(item(page, 40)).toBeAttached();
      for (const button of await scope(page).getByRole("button").all()) {
        const bounds = await button.boundingBox();
        expect(bounds).not.toBeNull();
        expect(bounds!.x).toBeGreaterThanOrEqual(0);
        expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width + 1);
      }
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= innerWidth + 1,
        ),
      ).toBe(true);
      const name = `browser-${width}-${dark ? "dark" : "light"}.png`;
      await page.screenshot({
        path: process.env.SESSION_NAV_IMAGE_DIR
          ? path.join(process.env.SESSION_NAV_IMAGE_DIR, name)
          : info.outputPath(name),
      });
    }
  }
});
test("session tabs keep model requests separate from authorized conversation reads", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto(
    "/tests/frontend/session-navigation.html#/sessions/claude%3Asession-0?view=requests",
  );
  const tabs = page.getByRole("tablist", { name: "会话详情" });
  await expect(tabs.getByRole("tab", { name: "模型请求" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(
    page.getByRole("region", { name: "会话模型请求" }),
  ).toBeVisible();
  expect(await page.evaluate(() => window.sessionNav.conversations)).toEqual(
    [],
  );
  const conversation = tabs.getByRole("tab", { name: "对话", exact: true });
  await conversation.click();
  const timeline = page.getByRole("button", {
    name: "调用时间线",
    exact: true,
  });
  await expect(timeline).toBeVisible();
  await timeline.click();
  await expect(timeline).toHaveAttribute("aria-pressed", "true");
  await expect(conversation).toHaveAttribute("aria-selected", "true");
});
