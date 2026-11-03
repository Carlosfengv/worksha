import { expect, test } from "@playwright/test";
import { agentName } from "../../src/features/providers/display";
import { configAgentName, agentProducts } from "../../src/shared/types/agent";
import {
  formatDateTime,
  formatCalendarDate,
} from "../../src/shared/lib/date-time";

test("known configuration names use the directory and unknown agents retain their identity", () => {
  expect(agentName("claude")).toBe("Claude Code CLI");
  expect(agentName("codex")).toBe("Codex");
  expect(agentName("codex-desktop")).toBe("Codex 桌面端");
  expect(agentName("future-agent")).toBe("future-agent");
  expect(agentName("constructor")).toBe("constructor");
  expect(
    configAgentName("shared-config", [
      { id: "a", name: "A", family: "one", configId: "shared-config" },
      { id: "b", name: "B", family: "two", configId: "shared-config" },
    ]),
  ).toBe("shared-config");
  expect(
    configAgentName("new-config", [
      ...agentProducts,
      {
        id: "new-client",
        name: "新增客户端",
        family: "new-family",
        configId: "new-config",
      },
    ]),
  ).toBe("新增客户端");
});
test("date precision shares one zone while calendar dates do not invent a time", () => {
  const original = process.env.TZ;
  try {
    process.env.TZ = "America/Los_Angeles";
    expect(formatDateTime("2026-10-29T23:05:06Z")).toBe("10/30 07:05");
    expect(formatDateTime("2026-10-29T23:05:06Z", "full")).toBe(
      "2026/10/30 07:05:06",
    );
    expect(formatCalendarDate("2026-10-30")).toBe("2026/10/30");
    expect(formatDateTime("2026-10-30")).toBe("2026/10/30");
    expect(formatDateTime(0, "full")).toBe("1970/01/01 08:00:00");
    for (const value of [
      null,
      undefined,
      "",
      "bad",
      "0",
      "2026-02-30",
      "2026-02-30T00:00:00Z",
      "2026-10-30T24:00:00Z",
      "2026-10-30T00:00:00",
    ])
      expect(formatDateTime(value)).toBe("时间未知");
    expect(formatDateTime("2024-02-29T23:05:06.123456789+08:00", "full")).toBe(
      "2024/02/29 23:05:06",
    );
  } finally {
    if (original === undefined) delete process.env.TZ;
    else process.env.TZ = original;
  }
});

test.describe("display policy in a different system locale and timezone", () => {
  test.use({ locale: "en-US", timezoneId: "America/Los_Angeles" });
  test("settings diagnostics keep the same Chinese Shanghai timestamp as the main UI", async ({
    page,
  }) => {
    await page.goto("/tests/frontend/tables.html?mode=shell#/settings");
    await page
      .getByRole("button", { name: "最近诊断记录", exact: true })
      .click();
    await expect(
      page.getByLabel("最近后台连接诊断", { exact: true }),
    ).toContainText("2026/10/30 08:00:00");
  });
});
