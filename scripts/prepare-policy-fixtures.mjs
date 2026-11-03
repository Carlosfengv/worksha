import {spawnSync} from "node:child_process";
import fs from "node:fs";
import path from "node:path";
const binary=path.resolve(process.env.PLUGIN_TEST_BINARY??"target/debug/local-agent-service");
const run=(cmd,args)=>{const r=spawnSync(cmd,args,{stdio:"inherit",env:{...process.env,CARGO_INCREMENTAL:"0"}});if(r.status!==0)throw Error(`${cmd} failed: ${r.status}`);};
const args=["build","--offline","--locked","--manifest-path","sdk/plugins/Cargo.toml","--release","--target","wasm32-unknown-unknown"];
run("cargo",[...args,"-p","pure-policy-plugin"]);
run(binary,["--plugin-sdk","componentize","sdk/plugins/target/wasm32-unknown-unknown/release/pure_policy_plugin.wasm","sdk/plugins/examples/pure-policy/package/plugin.wasm"]);
const root=".work/domain-d5-policy-fixtures";fs.mkdirSync(root,{recursive:true});
for(const feature of ["duplicate","foreign","stale","host-call","loop","memory","empty","configured"]){
 run("cargo",[...args,"-p","policy-hostile-fixture","--no-default-features","--features",feature]);
 run(binary,["--plugin-sdk","componentize","sdk/plugins/target/wasm32-unknown-unknown/release/policy_hostile_fixture.wasm",`${root}/${feature}.wasm`]);
}
console.log(JSON.stringify({complete:true,root,variants:8}));
