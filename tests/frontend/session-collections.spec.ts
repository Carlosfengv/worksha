import { expect, test } from "@playwright/test";
import {
  appendUniqueById,
  groupSessionsByProject,
} from "../../src/features/sessions/session-collections";

test("project grouping preserves order and hidden/unknown semantics without changing input", () => {
  const items = Object.freeze(
    [
      { id: "a", projectPath: "/one" },
      { id: "b", projectPath: "/private", projectHidden: true },
      { id: "c", projectPath: null },
      { id: "d", projectPath: "/one" },
      { id: "e", projectPath: "/other-private", projectHidden: true },
    ].map((item) => Object.freeze(item)),
  );
  const groups = groupSessionsByProject(items);
  expect([...groups.keys()]).toEqual(["/one", "已隐藏项目的会话", "项目未知"]);
  expect(groups.get("/one")).toEqual([items[0], items[3]]);
  expect(groups.get("已隐藏项目的会话")).toEqual([items[1], items[4]]);
  expect(groups.get("项目未知")).toEqual([items[2]]);
});

test("overlapping pages preserve existing event versions and reject duplicates within a new page", () => {
  const old = Object.freeze([{ id: "a", text: "original" }]);
  const incoming = Object.freeze([
    { id: "a", text: "later duplicate" },
    { id: "b", text: "first b" },
    { id: "b", text: "second b" },
    { id: "c", text: "c" },
  ]);
  const result = appendUniqueById(old, incoming);
  expect(result).toEqual([old[0], incoming[1], incoming[3]]);
  expect(result[0]).toBe(old[0]);
  expect(old).toHaveLength(1);
  expect(incoming).toHaveLength(4);
});

test("a large single-project collection retains every item and ordered page overlap", async ({}, info) => {
  const items = Array.from({ length: 20_000 }, (_, id) => ({
    id: String(id),
    projectPath: "/same-project",
  }));
  const started = performance.now();
  const groups = groupSessionsByProject(items);
  const merged = appendUniqueById(items.slice(0, 15_000), items.slice(10_000));
  const elapsedMs = performance.now() - started;
  expect(groups.size).toBe(1);
  expect(groups.get("/same-project")).toEqual(items);
  expect(merged).toEqual(items);
  await info.attach("collection-sample", {
    body: JSON.stringify({
      records: items.length,
      elapsedMs,
      scope: "grouping and merge only; excludes DOM and WebView memory",
    }),
    contentType: "application/json",
  });
});
