# Local Agent 外部插件 SDK

所有外部插件使用同一套清单、包检查器、签名格式与 worker 入口。当前 `manifestVersion` 为 1，宿主 API 为 `1.0.0`，WIT ABI 为 `local-agent:plugin@1.0.0`。按 `capabilities` 声明功能，按 `requires` 声明宿主特性或依赖，按 `permissions` 申请有限权限。

安装、信任发布者、启用、资源授权与业务操作相互独立。签名有效不代表作者身份已核实或资源已授权；未知能力、字段、权限和不兼容版本均拒绝。

## 示例与接入

| 示例 | 能力 | 接入说明 |
| --- | --- | --- |
| `aurora-agent` | 单个 JSONL 的会话来源 | 用户选定文件，手动刷新，按需读取正文 |
| `pulse-quota` | 额度解析与静态连接预设 | 绑定已有连接，由宿主请求固定 `/quota` |
| `provider-presets` | 无代码的连接预设 | [预设](docs/provider-presets.md) |
| `model-catalog` | 有限模型目录解析 | [目录与独立授权](docs/provider-models.md) |
| `gateway-report` | 宿主网关汇总视图 | 单独授权当前目录最近 24 小时汇总 |
| `envelope-protocol`、`json-config` | 协议适配与有限配置计划 | [扩展](docs/extensions.md) |
| `mcp-stdio`、`mcp-http` | 用户选定的 MCP 服务连接 | [扩展](docs/extensions.md) |
| `full-history` | 项目、会话、响应、活动等只读事实 | [完整历史来源](docs/history-source.md) |
| `pure-policy` | 无资源权限的有限候选排序 | [路由策略](docs/routing-policy.md) |
| `detail-helper`、`detail-report`、`object-summary` | 宿主渲染的详情内容 | [通用插槽](docs/contributions.md)、[对象摘要](docs/object-contributions.md) |

示例中的 `org.example` 不代表真实组织，`.invalid` 地址需自行替换。示例不连接真实账户或调用模型。协议、配置、来源、路由和网络权限按能力分别核查，不能通过混合清单扩大权限。

## 构建与打包

本目录是独立 Rust workspace，可复制到应用仓库外构建。需要 Rust 1.92.0 和 `wasm32-unknown-unknown` target；安装插件的用户不需要 Node、Rust 或浏览器。

```sh
rustup target add wasm32-unknown-unknown --toolchain 1.92.0
cargo +1.92.0 build --locked --release --target wasm32-unknown-unknown
```

`SERVICE` 指向应用随包提供的 `Local Agent.app/Contents/MacOS/local-agent-service` 绝对路径。以下以 Aurora 为例；输出使用新路径，私钥保存在包目录外。

```sh
"$SERVICE" --plugin-sdk componentize target/wasm32-unknown-unknown/release/aurora_agent_plugin.wasm /absolute/package/plugin.wasm
"$SERVICE" --plugin-sdk keygen /absolute/author.key
"$SERVICE" --plugin-sdk pack /absolute/package /absolute/aurora.laplug /absolute/author.key
"$SERVICE" --plugin-sdk check /absolute/aurora.laplug
"$SERVICE" --plugin-sdk schemas /absolute/contracts/plugins
```

所有能力都使用 `pack` 和 `check`。Wasm 示例需先 componentize，再放入对应包目录。开发用未签名包使用 `pack-development`，检查时显式加 `--development`；应用还必须显式开启插件开发模式。完整历史和纯路由策略始终要求签名。

插件 ID 必须位于发布者命名空间，例如 `com.vendor.product`。保留内置标识，不允许覆盖。JSONL 来源的产品 ID 使用 `<pluginId>:<localId>`。

## 包与签名

包为 ZIP：压缩最多 32 MiB、解压总量 128 MiB、最多 1000 个普通文件，Wasm 最多 32 MiB。只允许安全 ASCII 相对路径；拒绝目录项、链接、重复路径和大小写冲突。

`files.json` 精确列出所有载荷的 SHA-256，`signature.json` 提供 Ed25519 公钥与签名。签名消息为 `local-agent-plugin\n`、严格类型化清单经稳定 JSON 编码的字节、换行、文件索引的稳定 JSON。请使用 SDK 工具，避免默认字段或排序差异。完整归档的 digest 绑定检查、安装和启用身份；签名私钥不得放入包内。

清单、来源、扩展、对象声明与策略均通过同一检查器。不同能力可以持有独立资源与持久化仓储，但共享发布者信任和安装准入。更换版本、来源或授权使旧运行身份和结果失效；依赖不会共享授权。

## 运行边界

WIT world `plugin` 导出 `invoke(method, payload)`，导入有限的 `host.call(method, payload)`。payload 为 UTF-8 JSON。宿主与隔离 worker 通过匿名 stdin/stdout 的 NDJSON JSON-RPC 2.0 通信，每帧最多 1 MiB。

生命周期依次为 `plugin.initialize`、`plugin.prepare`、`plugin.activate`、`plugin.health`、业务调用、`plugin.deactivate`、`plugin.dispose`。initialize 回显宿主提供的 API、摘要、能力、epoch、grantRevision、contextId、预算和只读设置，其余阶段返回 `{"ready":true}`。预检和生命周期阶段没有资源反向调用权限。

Wasm 无 WASI、任意文件、网络、进程或凭据访问。来源只得到随机资源引用，网络由宿主注入凭据并核查同源、DNS、超时和私网授权。MCP stdio 是另行选择的本机程序，按当前用户权限运行。

额度、会话与目录调用按任务创建 worker；协议转换、配置与连接器遵循各自的有限生命周期。错误、未知、过期和确认无数据分别保留，不能把发现目录或安装成功显示为已验证连接。

## 契约维护

权威 Rust 类型生成 `contracts/plugins/` 与 `contracts/domain/` 的 JSON Schema，`scripts/plugin-bindings.mjs` 生成 TypeScript。契约目录按能力命名；WIT 副本与宿主保持一致。`pnpm check:plugins`、`pnpm check:domain` 和 Rust 测试检查漂移，不能只刷新快照绕过行为校验。

各领域可用 `schemas-extensions`、`schemas-sources`、`schemas-policy`、`schemas-domain` 等命令单独导出。授权、配置恢复和发布回执的边界见对应接入文档。DSH、Developer ID 与 Apple 公证未验证。
