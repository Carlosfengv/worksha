import assert from "node:assert/strict";
import { test } from "node:test";
import {
  activeHash,
  closeTabs,
  initialWorkspace,
  openHome,
  openResource,
  pinTab,
  pruneUnavailableTabs,
  resourceTab,
  setTabDirty,
} from "../src/app/workspace-tabs.ts";

const request = (id, search = "") =>
  resourceTab({ page: "requests", objectId: id }, `#/requests/${id}${search}`);

test("one preview slot is reused and existing resources keep their index and use the latest anchor", () => {
  let state = initialWorkspace;
  for (let i = 0; i < 20; i += 1) {
    state = openResource(state, request(String(i)));
    assert.equal(state.tabs.length, 1);
  }
  state = pinTab(state, "request:19");
  state = openResource(state, request("20"));
  state = openResource(state, request("19", "?anchor=tool"));
  assert.equal(activeHash(state), "#/requests/19?anchor=tool");
  assert.deepEqual(state.tabs.map(({ pinned }) => pinned), [true, false]);
});

test("closing selects the neighbor and Home restores its last collection query", () => {
  let state = openHome(initialWorkspace, "requests", "range=7");
  for (const id of ["a", "b", "c"]) state = openResource(state, request(id), true);
  state = openResource(state, request("b"));
  state = closeTabs(state, ["request:b"]);
  assert.equal(state.activeTabKey, "request:c");
  state = closeTabs(state, ["request:c"]);
  assert.equal(state.activeTabKey, "request:a");
  state = closeTabs(state, ["request:a"]);
  assert.equal(activeHash(state), "#/requests?range=7");
});

test("editing promotes a preview and capacity eviction preserves active and dirty tabs", () => {
  let state = openResource(initialWorkspace, request("draft"));
  state = setTabDirty(state, "request:draft", true);
  assert.equal(state.tabs[0].pinned, true);
  for (let i = 0; i < 12; i += 1) state = openResource(state, request(String(i)), true);
  assert.equal(state.tabs.length, 12);
  assert.equal(state.tabs[0].dirty, true);
  assert.equal(state.tabs.some(({ objectId }) => objectId === "0"), false);
  state = pruneUnavailableTabs(state, () => false);
  assert.deepEqual(state.tabs.map(({ objectId }) => objectId), ["11"]);
});
