import { expect, test } from "@playwright/test";
import { writeFileSync } from "node:fs";
import path from "node:path";

test("gateway sessions share the title list and open requests without native consent", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/session-navigation.html?gateway=1#/sessions",
  );
  const list = page.getByLabel("会话列表滚动区域", { exact: true });
  await expect(list.getByRole("button", { name: /^Session 0 / })).toBeVisible();
  const observed = list.getByRole("button", { name: /^Codex 测试会话 / });
  await expect(observed).toBeVisible();
  await expect(page.getByLabel("会话来源范围")).toHaveCount(0);
  await observed.click();
  await expect(
    page.getByLabel("会话模型请求").getByText("fixture-gateway-model"),
  ).toBeVisible();
  expect(
    await page.evaluate(() => window.sessionNav.conversations),
  ).not.toContain("codex:00000000-0000-4000-8000-000000000001");
});

test("eight session pages keep title navigation bounded", async ({
  page,
}, info) => {
  await page.goto(
    "/tests/frontend/session-navigation.html#/sessions/claude%3Asession-0",
  );
  const list = page.getByLabel("会话列表滚动区域", { exact: true });
  await expect(list.getByRole("button", { name: /^Session 0 / })).toBeVisible();
  for (let n = 1; n < 8; n++) {
    await page.getByRole("button", { name: /加载更多会话|下一页会话/ }).click();
    await expect(
      list.getByRole("button", { name: new RegExp(`^Session ${n * 40} `) }),
    ).toBeAttached();
  }
  const result = await list.evaluate((node) => ({
    rows: node.querySelectorAll("button[aria-pressed]").length,
    domNodes: document.querySelectorAll("*").length,
  }));
  writeFileSync(
    process.env.SESSION_NAV_REPORT ?? info.outputPath("navigation.json"),
    JSON.stringify(result, null, 2),
  );
  expect(result.rows).toBeLessThanOrEqual(41);
});

test("implicit and explicit selections keep the reader stable while the title page changes", async ({
  page,
}) => {
  await page.goto("/tests/frontend/session-navigation.html");
  await expect(
    page.getByRole("heading", { name: "Session 0", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "下一页会话", exact: true }).click();
  const list = page.getByLabel("会话列表滚动区域", { exact: true });
  await expect(
    list.getByRole("button", { name: /^Session 40 / }),
  ).toBeAttached();
  await expect(
    page.getByRole("heading", { name: "Session 0", exact: true }),
  ).toBeVisible();
  await list.getByRole("button", { name: /^Session 40 / }).click();
  await expect(
    page.getByRole("heading", { name: "Session 40", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "下一页会话", exact: true }).click();
  await expect(
    list.getByRole("button", { name: /^Session 80 / }),
  ).toBeAttached();
  await expect(
    page.getByRole("heading", { name: "Session 40", exact: true }),
  ).toBeVisible();
  const previous = page.getByRole("button", {
    name: "上一页会话",
    exact: true,
  });
  await previous.focus();
  await previous.press("Enter");
  await expect(
    list.getByRole("button", { name: /^Session 40 / }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(list).toBeFocused();
  await expect(
    page.getByRole("status").filter({ hasText: "本页 40 个 · 共 10,000 个" }),
  ).toBeVisible();
});

test("a filtered selection stays pinned on later pages and no matches clear both panes", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/session-navigation.html#/sessions/claude%3Asession-0",
  );
  const search = page.getByPlaceholder("搜索标题、项目或路径");
  await search.fill("filter");
  await expect(
    page.getByRole("heading", { name: "Session 5000", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "下一页会话", exact: true }).click();
  await expect(
    page.getByRole("button", { name: /^Session 5040 / }),
  ).toBeAttached();
  await expect(
    page.getByRole("heading", { name: "Session 5000", exact: true }),
  ).toBeVisible();
  await search.fill("none");
  await expect(page.getByRole("heading", { name: /^Session / })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /^Session / })).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "没有匹配会话", exact: true }),
  ).toHaveCount(2);
  await search.fill("");
  await expect(
    page.getByRole("heading", { name: "Session 0", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "上一页会话", exact: true }),
  ).toBeDisabled();
});

test("grouping preserves this page's identities, expanded projects and independent scroll on return", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/session-navigation.html#/sessions/claude%3Asession-0",
  );
  await page.getByRole("button", { name: "下一页会话", exact: true }).click();
  const list = page.getByLabel("会话列表滚动区域", { exact: true });
  const labels = () =>
    list.getByRole("button", { name: /^Session / }).allTextContents();
  await expect(list.getByRole("button", { name: /^Session / })).toHaveCount(41);
  const flat = await labels();
  await page.getByRole("button", { name: "按项目分组", exact: true }).click();
  const groups = list.getByRole("button", { name: / · 本页 \d+ 个/ });
  await expect(groups).toHaveCount(4);
  for (const group of await groups.all()) await group.click();
  await expect(list.getByRole("button", { name: /^Session / })).toHaveCount(41);
  expect((await labels()).sort()).toEqual(flat.sort());
  await list.click({ position: { x: 8, y: 8 } });
  await list.evaluate((node) => {
    node.scrollTop = 450;
  });
  await expect.poll(() => list.evaluate((node) => node.scrollTop)).toBe(450);
  const body = page.getByLabel("对话滚动区域", { exact: true });
  const bodyScroll = await body.evaluate((node) => node.scrollTop);
  await page.getByRole("button", { name: "下一页会话", exact: true }).click();
  await expect(list.getByRole("button", { name: /^Session / })).toHaveCount(1);
  await page.getByRole("button", { name: "上一页会话", exact: true }).click();
  await expect(list.getByRole("button", { name: /^Session / })).toHaveCount(41);
  await expect.poll(() => list.evaluate((node) => node.scrollTop)).toBe(450);
  expect(await body.evaluate((node) => node.scrollTop)).toBe(bodyScroll);
});

test("an external old-session link gets a visible stable-id pin without choosing a similarly named row", async ({
  page,
}) => {
  await page.goto("/tests/frontend/session-navigation.html");
  const list = page.getByLabel("会话列表滚动区域", { exact: true });
  await list
    .getByRole("button", { name: /^Session 30 / })
    .scrollIntoViewIfNeeded();
  await page.evaluate(() => {
    location.hash = "/sessions/claude%3Asession-9999";
  });
  await expect(
    page.getByRole("heading", { name: "Session 9999", exact: true }),
  ).toBeVisible();
  const pin = list.getByRole("button", { name: /^Session 9999 / });
  await expect(pin).toBeInViewport();
  await expect(pin).toHaveAttribute("aria-pressed", "true");
  await expect(
    list.getByRole("button", { name: /^Session 9 / }),
  ).toHaveAttribute("aria-pressed", "false");
});

test("320px drawer paginates, selects by keyboard and restores focus on Escape", async ({
  page,
}) => {
  await page.setViewportSize({ width: 320, height: 700 });
  await page.goto("/tests/frontend/session-navigation.html");
  const trigger = page.getByRole("button", {
    name: "切换会话 / 项目",
    exact: true,
  });
  await trigger.click();
  const dialog = page.getByRole("dialog", {
    name: "选择会话或项目",
    exact: true,
  });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "下一页会话", exact: true }).click();
  const item = dialog.getByRole("button", { name: /^Session 40 / });
  await expect(item).toBeAttached();
  await item.focus();
  await item.press("Enter");
  await expect(dialog).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Session 40", exact: true }),
  ).toBeVisible();
  await expect(trigger).toBeFocused();
  await trigger.click();
  await expect(
    dialog.getByRole("button", { name: /^Session 40 / }),
  ).toHaveAttribute("aria-pressed", "true");
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(trigger).toBeFocused();
});

test("transient page errors preserve the current page and retry without advancing the trail", async ({
  page,
}) => {
  await page.goto("/tests/frontend/session-navigation.html");
  await expect(
    page.getByRole("heading", { name: "Session 0", exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    window.sessionNav.error = "unavailable";
  });
  await page.getByRole("button", { name: "下一页会话", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "synthetic list unavailable",
  );
  await expect(page.getByRole("button", { name: /^Session 0 / })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "上一页会话", exact: true }),
  ).toBeDisabled();
  await page.evaluate(() => {
    window.sessionNav.error = null;
  });
  await page.getByRole("button", { name: "下一页会话", exact: true }).click();
  await expect(
    page.getByRole("button", { name: /^Session 40 / }),
  ).toBeAttached();
  await expect(page.getByRole("alert")).toHaveCount(0);
});

for (const code of ["unauthorized", "stale_snapshot", "not_found"]) {
  test(`${code} discards old titles and body until a verified reload`, async ({
    page,
  }) => {
    await page.goto(
      "/tests/frontend/session-navigation.html#/sessions/claude%3Asession-0",
    );
    await expect(
      page.getByRole("heading", { name: "Session 0", exact: true }),
    ).toBeVisible();
    await page.evaluate((code) => {
      window.sessionNav.error = code;
    }, code);
    await page.getByRole("button", { name: "下一页会话", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText(
      "synthetic list unavailable",
    );
    await expect(page.getByRole("button", { name: /^Session / })).toHaveCount(
      0,
    );
    await expect(page.getByRole("heading", { name: /^Session / })).toHaveCount(
      0,
    );
    await expect(
      page.getByRole("heading", { name: "没有匹配会话", exact: true }),
    ).toHaveCount(0);
    await page.evaluate(() => {
      window.sessionNav.error = null;
    });
    await page.getByRole("button", { name: "刷新列表", exact: true }).click();
    await expect(
      page.getByRole("heading", { name: "Session 0", exact: true }),
    ).toBeVisible();
    expect(
      await page.evaluate(() => window.sessionNav.conversations.length),
    ).toBe(2);
  });
}

test("a pending old page cannot survive connection recovery or repopulate the new title set", async ({
  page,
}) => {
  await page.goto("/tests/frontend/session-navigation.html");
  await expect(
    page.getByRole("heading", { name: "Session 0", exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    window.sessionNav.hold = true;
  });
  await page.getByRole("button", { name: "下一页会话", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.sessionNav.releases.length))
    .toBe(1);
  await page.evaluate(() => window.sessionNav.recover());
  await expect(page.getByRole("button", { name: /^Session / })).toHaveCount(0);
  await page.evaluate(() => {
    window.sessionNav.hold = false;
    window.sessionNav.releases[0]();
  });
  await expect(
    page.getByRole("button", { name: /^Session 0 revision-1 / }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: /^Session 40 / })).toHaveCount(
    0,
  );
  await expect(
    page.getByRole("button", { name: "上一页会话", exact: true }),
  ).toBeDisabled();
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
  await page.getByRole("tab", { name: "概览", exact: true }).click();
  await expect(page.getByRole("button", { name: /^Session / })).toHaveCount(5);
  await expect(page.getByRole("button", { name: /^Session 0 / })).toBeVisible();
  await page.getByRole("tab", { name: "Sessions", exact: true }).click();
  await expect(
    page.getByRole("button", { name: /^Session 40 / }),
  ).toBeAttached();
});

test("an external session outside a search relaxes that conflicting filter without choosing a fallback row", async ({
  page,
}) => {
  await page.goto("/tests/frontend/session-navigation.html");
  const search = page.getByPlaceholder("搜索标题、项目或路径");
  await search.fill("filter");
  await expect(
    page.getByRole("heading", { name: "Session 5000", exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    location.hash = "/sessions/claude%3Asession-9999";
  });
  await expect(search).toHaveValue("");
  await expect(
    page.getByRole("heading", { name: "Session 9999", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^Session 9999 / }),
  ).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("button", { name: "下一页会话", exact: true }).click();
  await expect(
    page.getByRole("button", { name: /^Session 40 / }),
  ).toBeAttached();
  await expect(
    page.getByRole("heading", { name: "Session 9999", exact: true }),
  ).toBeVisible();
});

test("a confirmed empty refresh removes an implicit selection from both panes", async ({
  page,
}) => {
  await page.clock.install();
  await page.goto("/tests/frontend/session-navigation.html");
  await expect(
    page.getByRole("heading", { name: "Session 0", exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    window.sessionNav.empty = true;
  });
  await page.clock.fastForward(5000);
  await expect(page.getByRole("button", { name: /^Session / })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: /^Session / })).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "没有匹配会话", exact: true }),
  ).toHaveCount(2);
});

test("a partial empty source is visibly unverified rather than a complete zero-history claim", async ({
  page,
}) => {
  await page.clock.install();
  await page.goto("/tests/frontend/session-navigation.html");
  await expect(
    page.getByRole("heading", { name: "Session 0", exact: true }),
  ).toBeVisible();
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
  await page.goto("/tests/frontend/session-navigation.html");
  await expect(
    page.getByRole("heading", { name: "Session 0", exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    window.sessionNav.connected = false;
  });
  await page.clock.fastForward(5000);
  await expect(page.getByRole("heading", { name: /^Session / })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /^Session / })).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "尚未读取会话", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("status").filter({ hasText: "会话数量尚未核查" }),
  ).toBeVisible();
});

test("first-page polling preserves list scroll and the focused stable session", async ({
  page,
}) => {
  await page.clock.install();
  await page.goto("/tests/frontend/session-navigation.html");
  const list = page.getByLabel("会话列表滚动区域", { exact: true });
  await expect(
    list.getByRole("button", { name: /^Session 20 / }),
  ).toBeAttached();
  await list.click({ position: { x: 8, y: 8 } });
  const item = list.getByRole("button", { name: /^Session 20 / });
  await item.focus();
  await list.evaluate((node) => {
    node.scrollTop = 450;
  });
  await expect.poll(() => list.evaluate((node) => node.scrollTop)).toBe(450);
  await page.clock.fastForward(5000);
  await expect
    .poll(() => page.evaluate(() => window.sessionNav.queries.length))
    .toBe(2);
  await expect(item).toBeFocused();
  await expect.poll(() => list.evaluate((node) => node.scrollTop)).toBe(450);
});

test("navigation controls fit desktop and drawer in light and dark layouts", async ({
  page,
}, info) => {
  for (const width of [1280, 320])
    for (const dark of [false, true]) {
      await page.setViewportSize({ width, height: width === 320 ? 700 : 860 });
      await page.goto("/tests/frontend/session-navigation.html");
      await page.evaluate(
        (dark) => document.documentElement.classList.toggle("dark", dark),
        dark,
      );
      if (width === 320)
        await page
          .getByRole("button", { name: "切换会话 / 项目", exact: true })
          .click();
      const nav =
        width === 320
          ? page
              .getByRole("dialog", { name: "选择会话或项目", exact: true })
              .getByRole("navigation", { name: "会话分页", exact: true })
          : page.getByRole("navigation", { name: "会话分页", exact: true });
      await expect(
        nav.getByRole("button", { name: "下一页会话", exact: true }),
      ).toBeEnabled();
      // A real click waits for the drawer's enter transition to settle.
      await nav
        .getByRole("button", { name: "下一页会话", exact: true })
        .click();
      await expect(
        page.getByRole("button", { name: /^Session 40 / }),
      ).toBeAttached();
      for (const button of await nav.getByRole("button").all()) {
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
});
