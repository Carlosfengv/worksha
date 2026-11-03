import assert from "node:assert/strict";
import { messageGroups } from "../src/features/observation/message-groups.ts";

const at = (n) => new Date(Date.UTC(2026, 9, 24, 12, 0, n)).toISOString();
const native = (id, start, session = "opencode:s") => ({
  id, sessionId: session, messageId: id, messageStartedAt: at(start),
  eventAt: at(start + 1), userInput: { text: "repeat", at: at(start) },
});
const request = (id, start, meta = null, session = "opencode:s") => ({
  request: { id, sessionId: session, startedAt: at(start), activityMessage: meta },
});
const natives = [native("u1", 0), native("u2", 10), native("u3", 20)];
let groups = messageGroups([request("a", 1), request("b", 2), request("c", 11), request("d", 21)], natives);
assert.equal(groups.length, 3);
assert(groups.every((m) => !m.uncertain));
assert.deepEqual(groups.map((m) => m.gateways.map((g) => g.request.id)), [["a", "b"], ["c"], ["d"]]);
const meta = { id: "fingerprint-u1", startedAt: at(1) };
groups = messageGroups([request("first", 1, meta), request("late-tool", 25, meta)], natives);
assert.deepEqual(groups.find((m) => m.id.endsWith(":u1")).gateways.map((g) => g.request.id), ["first", "late-tool"]);
assert.equal(groups.find((m) => m.id.endsWith(":u3")).gateways.length, 0);
groups = messageGroups([request("a", 1), request("b", 2)], []);
assert.equal(groups.length, 2);
assert(groups.every((m) => m.gateways.length === 1 && m.uncertain));
groups = messageGroups([request("a", 1, meta), request("b", 2, meta), request("c", 21, {id:"next", startedAt:at(20)})], []);
assert.equal(groups.length, 2);
groups = messageGroups([request("a", 1), request("b", 1, null, "opencode:other")], [native("u1", 0), native("u1", 0, "opencode:other")]);
assert.equal(groups.length, 2);
assert(groups.every((m) => m.gateways.length === 1));
console.log("通过：同 Session 不同消息及重复输入分行、工具续调归回原消息、跨 Session 隔离、缺消息证据不合并整条 Session。");
