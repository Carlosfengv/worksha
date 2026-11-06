import { expect, test } from "@playwright/test";
import type {} from "./requests-harness";

test("an exact Claude request returns to its original CLI task", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/requests.html?workbench=claude#/requests/request-001?range=all",
  );
  await page.getByRole("button", { name: "打开关联会话", exact: true }).click();
  await expect(page).toHaveURL(
    /#\/tasks\/00000000-0000-4000-8000-000000000003$/,
  );
  expect(await page.evaluate(() => window.requestWorkbenchSelections)).toEqual(
    [],
  );
});

for (const mode of ["claude-missing", "claude-offline", "claude-desktop"]) {
  test(`${mode}: keeps the exact native session history instead of an unrelated task`, async ({
    page,
  }) => {
    await page.goto(
      `/tests/frontend/requests.html?workbench=${mode}#/requests/request-001?range=all`,
    );
    await page
      .getByRole("button", { name: "打开关联会话", exact: true })
      .click();
    await expect(page).toHaveURL(
      /#\/sessions\/claude%3A00000000-0000-4000-8000-000000000002\?/,
    );
  });
}

test("a delayed Claude association preserves later navigation", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/requests.html?workbench=claude-slow#/requests/request-001?range=all",
  );
  await page.getByRole("button", { name: "打开关联会话", exact: true }).click();
  await page.evaluate(() => {
    location.hash = "/requests/request-002?range=all";
  });
  await page.waitForTimeout(350);
  await expect(page).toHaveURL(/#\/requests\/request-002\?range=all$/);
});

test("an exact OpenCode request association returns to the original workbench session", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/requests.html?workbench=current#/requests/request-001?range=all",
  );
  expect(await page.evaluate(() => window.requestWorkbenchSelections)).toEqual(
    [],
  );
  await page.getByRole("button", { name: "打开关联会话", exact: true }).click();
  await expect(page).toHaveURL(
    /#\/tasks\/ses_00000000000040008000000000000001\?tab=live$/,
  );
  expect(await page.evaluate(() => window.requestWorkbenchSelections)).toEqual([
    {
      kind: "select",
      connectionId: "current-connection",
      sessionId: "ses_00000000000040008000000000000001",
    },
  ]);
});

for (const mode of ["missing", "offline"]) {
  test(`${mode}: the same session's request history remains accessible without selecting another task`, async ({
    page,
  }) => {
    await page.goto(
      `/tests/frontend/requests.html?workbench=${mode}#/requests/request-001?range=all`,
    );
    await page
      .getByRole("button", { name: "打开关联会话", exact: true })
      .click();
    await expect(page).toHaveURL(
      /#\/sessions\/opencode%3Ases_00000000000040008000000000000001\?/,
    );
    expect(
      await page.evaluate(() => window.requestWorkbenchSelections),
    ).toEqual([]);
  });
}

test("a delayed task association cannot navigate away from a newly selected request", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/requests.html?workbench=slow#/requests/request-001?range=all",
  );
  await page.getByRole("button", { name: "打开关联会话", exact: true }).click();
  await page.evaluate(() => {
    location.hash = "/requests/request-002?range=all";
  });
  await page.waitForTimeout(350);
  await expect(page).toHaveURL(/#\/requests\/request-002\?range=all$/);
});
