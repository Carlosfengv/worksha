import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Keep the JIT runtime exception on the sidecar. Tauri's global entitlements
// option would also grant it to the desktop executable, which does not run guests.
if(process.platform!=="darwin")throw Error("此签名入口仅适用于 macOS");
const args=process.argv.slice(2);
const index=args.indexOf("--app");if(index<0||!args[index+1])throw Error("需要 --app /absolute/Local Agent.app");
const app=path.resolve(args[index+1]);
const identityIndex=args.indexOf("--identity");
const identity=identityIndex<0?"-":args[identityIndex+1];
if(!identity||identity.startsWith("--"))throw Error("签名身份无效");
if(!app.endsWith(".app")||!fs.statSync(app).isDirectory())throw Error("请选择已构建的 App 包");
const entitlement=fileURLToPath(new URL("../src-tauri/plugin-worker.entitlements.plist",import.meta.url));
const run=(args)=>{const result=spawnSync("/usr/bin/codesign",args,{stdio:"inherit"});if(result.status!==0)throw Error("App 签名或校验失败");};
const base=["--force","--sign",identity,"--options","runtime",...(identity==="-"?[]:["--timestamp"])];
run([...base,"--entitlements",entitlement,path.join(app,"Contents/MacOS/local-agent-service")]);
// Explicit empty entitlements also remove any stale global JIT exception.
run([...base,"--entitlements",fileURLToPath(new URL("../src-tauri/desktop.entitlements.plist",import.meta.url)),app]);
run(["--verify","--deep","--strict","--verbose=2",app]);
console.log("服务独立执行权限、主窗口默认权限及 App 完整签名校验通过；Apple 公证需另行完成。");
