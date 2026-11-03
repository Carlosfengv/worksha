import assert from "node:assert/strict";
import { command, DesktopError } from "../src/shared/lib/desktop-command.ts";

const structured = new DesktopError(JSON.stringify({ code: "conflict", message: "配置已变更" }));
assert.equal(structured.code, "conflict");
assert.equal(structured.message, "配置已变更");
assert.equal(new DesktopError({ code: "unauthorized", message: "授权已失效" }).code, "unauthorized");
const plain = new DesktopError("读取失败");
assert.equal(plain.code, "unavailable");
assert.equal(plain.message, "读取失败");
assert.equal(new DesktopError(null).code, "unavailable");
const uncertain = new DesktopError(JSON.stringify({ code: "transport_timeout", message: "待核查", outcomeUnknown: true }));
assert.equal(uncertain.outcomeUnknown, true);
assert.equal(structured.outcomeUnknown, false);
await assert.rejects(command("get_snapshot"), /桌面应用/);
console.log("通过：结构化错误码、原生文本失败、未知错误及浏览器原生能力边界。");
