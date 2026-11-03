import { expect, test, type Page } from "@playwright/test";
import type {} from "./shared-read-harness";

const count = (page: Page, expected: number) =>
  expect
    .poll(() => page.evaluate(() => window.sharedReads.calls.length))
    .toBe(expected);
const resolve = (page: Page, index: number, label: string) =>
  page.evaluate(
    ({ index, label }) => window.sharedReads.calls[index].resolve({ label }),
    { index, label },
  );
const open = (page: Page, query = "") =>
  page.goto("/tests/frontend/shared-read.html" + query);
test.beforeEach(async ({ page }) => {
  await page.clock.install();
});

test("same parameters share their read and one poll; last unsubscribe clears cached data", async ({
  page,
}) => {
  await open(page);
  await count(page, 1);
  await resolve(page, 0, "shared");
  await expect(page.getByTestId("a-data")).toHaveText("shared");
  await expect(page.getByTestId("b-data")).toHaveText("shared");
  await page.clock.fastForward(10000);
  await count(page, 2);
  await page.getByRole("button", { name: "reload-a", exact: true }).click();
  await page.getByRole("button", { name: "reload-b", exact: true }).click();
  await count(page, 2);
  await resolve(page, 1, "polled");
  await page.getByRole("button", { name: "toggle-a", exact: true }).click();
  await page.clock.fastForward(10000);
  await count(page, 3);
  await resolve(page, 2, "last subscriber");
  await page.getByRole("button", { name: "toggle-b", exact: true }).click();
  await page.clock.fastForward(30000);
  await count(page, 3);
  await page.getByRole("button", { name: "toggle-a", exact: true }).click();
  await count(page, 4);
  await expect(page.getByTestId("a-data")).toHaveText("empty");
  await resolve(page, 3, "new lease");
  await expect(page.getByTestId("a-data")).toHaveText("new lease");
});

test("different query parameters and family identities never share data", async ({
  page,
}) => {
  await open(page);
  await count(page, 1);
  await page.getByRole("button", { name: "source-b", exact: true }).click();
  await count(page, 2);
  await page.getByRole("button", { name: "toggle-c", exact: true }).click();
  await count(page, 3);
  const scopes = await page.evaluate(() =>
    window.sharedReads.calls.map(({ scope, parameters }) => ({
      scope,
      parameters,
    })),
  );
  expect(scopes[0].parameters.source).toBe("gateway");
  expect(scopes[1].parameters.source).toBe("native");
  expect(scopes[2].scope).toBe("other");
  await resolve(page, 1, "native");
  await resolve(page, 0, "gateway");
  await resolve(page, 2, "other family");
  await expect(page.getByTestId("a-data")).toHaveText("gateway");
  await expect(page.getByTestId("b-data")).toHaveText("native");
  await expect(page.getByTestId("c-data")).toHaveText("other family");
});

test("mutations suppress old publication and a later mutation drives another replacement", async ({
  page,
}) => {
  await open(page);
  await count(page, 1);
  await page.getByRole("button", { name: "invalidate-a", exact: true }).click();
  await page.getByRole("button", { name: "invalidate-b", exact: true }).click();
  await resolve(page, 0, "obsolete initial");
  await count(page, 2);
  await page.getByRole("button", { name: "invalidate-b", exact: true }).click();
  await resolve(page, 1, "obsolete replacement");
  await count(page, 3);
  await page.clock.runFor(100);
  await expect(page.getByTestId("a-data")).toHaveText("empty");
  await expect(page.getByTestId("b-data")).toHaveText("empty");
  await resolve(page, 2, "latest");
  await expect(page.getByTestId("a-data")).toHaveText("latest");
  await expect(page.getByTestId("b-data")).toHaveText("latest");
});

test("failed reads keep data and do not automatically retry in a loop", async ({
  page,
}) => {
  await open(page);
  await count(page, 1);
  await resolve(page, 0, "confirmed");
  await page.clock.fastForward(10000);
  await count(page, 2);
  await page.evaluate(() =>
    window.sharedReads.calls[1].reject(Error("offline")),
  );
  await expect(page.getByTestId("a-error")).toHaveText("offline");
  await expect(page.getByTestId("b-data")).toHaveText("confirmed");
  await page.clock.runFor(100);
  await count(page, 2);
  await page.clock.fastForward(10000);
  await count(page, 3);
  await resolve(page, 2, "recovered");
  await expect(page.getByTestId("b-error")).toBeEmpty();
});

test("synchronous errors release pending work; disabled hooks neither read nor invalidate other subscribers", async ({
  page,
}) => {
  await open(page);
  await count(page, 1);
  await resolve(page, 0, "confirmed");
  await page.getByRole("button", { name: "enable-b", exact: true }).click();
  await page.getByRole("button", { name: "invalidate-b", exact: true }).click();
  await count(page, 1);
  await expect(page.getByTestId("b-loading")).toHaveText("false");
  await page.evaluate(() => {
    window.sharedReads.throws = true;
  });
  await page.getByRole("button", { name: "reload-a", exact: true }).click();
  await expect(page.getByTestId("a-error")).toHaveText(
    "synchronous shared loader failure",
  );
  await expect(page.getByTestId("a-loading")).toHaveText("false");
  await page.evaluate(() => {
    window.sharedReads.throws = false;
  });
  await page.getByRole("button", { name: "reload-a", exact: true }).click();
  await count(page, 2);
  await resolve(page, 1, "retried");
  await expect(page.getByTestId("a-data")).toHaveText("retried");
});

test("StrictMode subscriptions and unstarted bindings do not duplicate work", async ({
  page,
}) => {
  await open(page, "?strict");
  await count(page, 1);
  expect(await page.evaluate(() => window.sharedReads.bindingsAreEmpty())).toBe(
    true,
  );
  await count(page, 1);
  await resolve(page, 0, "strict");
  await expect(page.getByTestId("b-data")).toHaveText("strict");
});

test("a captured hook callback cannot invalidate a remaining subscriber of its former query", async ({
  page,
}) => {
  await open(page);
  await count(page, 1);
  await resolve(page, 0, "old scope");
  await page.getByRole("button", { name: "save-a", exact: true }).click();
  await page.getByRole("button", { name: "provider-a", exact: true }).click();
  await count(page, 2);
  await page.evaluate(() => window.sharedReads.saved?.());
  await count(page, 2);
  await resolve(page, 1, "new scope");
  await expect(page.getByTestId("a-data")).toHaveText("new scope");
  await expect(page.getByTestId("b-data")).toHaveText("old scope");
});

test("recovery invalidates active reads without publishing old results or errors", async ({
  page,
}) => {
  await open(page);
  await count(page, 1);
  await resolve(page, 0, "confirmed");
  await page.clock.fastForward(10000);
  await count(page, 2);
  await page.evaluate(() => window.sharedReads.recover());
  await page.evaluate(() =>
    window.sharedReads.calls[1].reject(Error("old connection")),
  );
  await count(page, 3);
  await page.clock.runFor(100);
  await expect(page.getByTestId("a-data")).toHaveText("confirmed");
  await expect(page.getByTestId("a-error")).toBeEmpty();
  await resolve(page, 2, "new connection");
  await expect(page.getByTestId("b-data")).toHaveText("new connection");
});

test("a zero-subscriber pending read cannot be reused after recovery", async ({
  page,
}) => {
  await open(page, "?direct");
  await page.evaluate(() => {
    window.sharedReads.stop = window.sharedReads.direct.subscribe(() => {});
  });
  await count(page, 1);
  await page.evaluate(() => {
    window.sharedReads.stop?.();
    window.sharedReads.recover();
    window.sharedReads.stop = window.sharedReads.direct.subscribe(() => {});
  });
  await resolve(page, 0, "old orphan");
  await count(page, 2);
  expect(
    await page.evaluate(() => window.sharedReads.direct.getSnapshot().data),
  ).toBeNull();
  await resolve(page, 1, "new connection");
  await expect
    .poll(() =>
      page.evaluate(() => window.sharedReads.direct.getSnapshot().data?.label),
    )
    .toBe("new connection");
});

test("last-subscriber remount during publication starts a read after the settling request", async ({
  page,
}) => {
  await open(page, "?direct");
  await page.evaluate(() => {
    window.sharedReads.stop = window.sharedReads.direct.subscribe(() => {
      if (!window.sharedReads.direct.getSnapshot().data) return;
      window.sharedReads.stop?.();
      window.sharedReads.replacement = window.sharedReads.direct.subscribe(
        () => {},
      );
    });
  });
  await count(page, 1);
  await resolve(page, 0, "released during publication");
  await count(page, 2);
  expect(
    await page.evaluate(() => window.sharedReads.direct.getSnapshot().data),
  ).toBeNull();
  await resolve(page, 1, "replacement");
  await expect
    .poll(() =>
      page.evaluate(() => window.sharedReads.direct.getSnapshot().data?.label),
    )
    .toBe("replacement");
});

for (const terminal of ["data", "error"] as const) {
  test(`a new view cannot inherit pending ${terminal} after the last old reader leaves`, async ({
    page,
  }) => {
    await open(page, "?direct");
    await page.evaluate(() => {
      window.sharedReads.stop = window.sharedReads.direct.subscribe(() => {});
    });
    await count(page, 1);
    await page.evaluate(() => {
      window.sharedReads.stop?.();
      window.sharedReads.replacement = window.sharedReads.direct.subscribe(
        () => {},
      );
    });
    if (terminal === "data") await resolve(page, 0, "departed view");
    else
      await page.evaluate(() =>
        window.sharedReads.calls[0].reject(Error("departed failure")),
      );
    await count(page, 2);
    expect(
      await page.evaluate(() => {
        const snapshot = window.sharedReads.direct.getSnapshot();
        return { data: snapshot.data, error: snapshot.error };
      }),
    ).toEqual({ data: null, error: null });
    await resolve(page, 1, "current view");
    await expect
      .poll(() =>
        page.evaluate(
          () => window.sharedReads.direct.getSnapshot().data?.label,
        ),
      )
      .toBe("current view");
  });
}

test("domain notifications refresh one active query and stop when it has no readers", async ({
  page,
}) => {
  await open(page);
  await count(page, 1);
  await resolve(page, 0, "confirmed");
  await page.evaluate(() =>
    window.dispatchEvent(new Event("fixture-read-changed")),
  );
  await count(page, 2);
  await resolve(page, 1, "changed");
  await page.getByRole("button", { name: "toggle-a", exact: true }).click();
  await page.getByRole("button", { name: "toggle-b", exact: true }).click();
  await page.evaluate(() =>
    window.dispatchEvent(new Event("fixture-read-changed")),
  );
  await page.clock.fastForward(10000);
  await count(page, 2);
});

test("one-shot resources revalidate on recovery without polling or refocus reads", async ({
  page,
}) => {
  await open(page, "?oneshot");
  await count(page, 1);
  await resolve(page, 0, "first");
  await expect(page.getByTestId("a-data")).toHaveText("first");
  await page.clock.fastForward(60000);
  await page.evaluate(() =>
    document.dispatchEvent(new Event("visibilitychange")),
  );
  await count(page, 1);
  await page.evaluate(() => window.sharedReads.recover());
  await count(page, 2);
  await resolve(page, 1, "reconnected");
  await expect(page.getByTestId("b-data")).toHaveText("reconnected");
});

test("a completed background read publishes a fresh wrapper when the loader reuses its payload", async ({
  page,
}) => {
  await open(page);
  await count(page, 1);
  await page.evaluate(() => {
    const payload = { label: "first" };
    window.sharedReads.retained = payload;
    window.sharedReads.calls[0].resolve(payload);
  });
  await expect(page.getByTestId("a-data")).toHaveText("first");
  await page.clock.fastForward(10000);
  await count(page, 2);
  await page.evaluate(() => {
    const payload = window.sharedReads.retained!;
    payload.label = "second";
    window.sharedReads.calls[1].resolve(payload);
  });
  await expect(page.getByTestId("a-data")).toHaveText("second");
  await expect(page.getByTestId("b-data")).toHaveText("second");
});
