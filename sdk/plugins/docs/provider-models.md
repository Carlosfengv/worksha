# 外部模型目录

本接口约定有限模型目录的声明、授权和读取边界。Provider 保存、连接验证、配置与恢复遵循各自的独立契约。

## 声明与权限

使用 manifestVersion 1 / hostApi `^1.0.0` 与原 WIT ABI；旧包不需要改写。必须显式声明 capability `provider.models@1`、requires `host.provider-catalog@1`，并在签名包内包含 `provider-catalog.json`。见 `examples/model-catalog/package/` 的完整示例。

```json
{ "schemaVersion": 1, "baseProtocol": "responses", "path": "/catalog" }
```

`baseProtocol` 只允许 `messages`、`responses` 或 `chat-completions`。`path` 是至多 128 字节的固定后缀，每段只允许 ASCII 字母、数字、下划线和连字符；不允许空段、遍历、编码、查询、片段或 authority。必须恰好有一个 `connection.request` 权限，其 path 与声明完全相同。可同时声明静态预设与 UI 贡献，但不能混入来源、额度、报告、WASI、进程或任意网络权限。`provider.catalog@1` 仍仅表示静态预设，额度授权也不会授予模型目录读取权限。旧宿主应明确拒绝未知能力和 broker。

## 无权限的解析器与宿主职责

宿主调用 `provider.catalog.request`，输入使用共同 `CatalogContext` 的结构，但 `vendor`、`basePath` 始终为空，`officialCatalogRoot` 始终为 false，仅保留基础协议。插件只能返回签名后缀，例如 `{"path":"/catalog"}`。真实 Endpoint、前缀、凭据引用和认证头不进入 guest；宿主在已授权的同源 Endpoint 上拼接前缀，独立验证结果。

宿主 GET 后调用 `provider.catalog.parse`，传入 `{"snapshot":"宿主生成的本页标识","body":上游JSON}`。输出必须是严格 `Page`：原样返回本页 snapshot、共同 `CatalogModel` 数组和 `CatalogContinuation`。只报告来源声明；未知容量、工具支持等保留 null，明确 false 保留 false，不转成验证成功。宿主核对快照、字段、容量、密钥回显和分页，再通过与内置实现相同的 `ProviderAdapter` 消费结果。

**每次 request / parse 都使用独立、无状态的 worker。不得在两个调用之间依赖内存、全局变量或上一次 worker。** 两个方法中的 `Host.call` 均不开放；插件不能自己发送网络请求、读文件、读取凭据或修改 Provider。

网络由宿主管理：DNS 固定、禁止代理与重定向、私网另行同意、认证头由宿主注入。每次最多五页，每页最多 256 个候选，上游 JSON 最多 256 KiB；worker 仍受已有 Wasm 资源约束，单次网络等待最多 8 秒，整个读取最多 15 秒。游标由宿主编码到 `after_id`，重复或无效游标拒绝，达到有限页数返回截断标志。目录读取不执行模型调用。

## 绑定、撤销与恢复

安装、启用和查看元数据不会读取目录。用户明确授权指定插件 digest 与一个连接修订，或者指定尚未保存的完整草稿范围；私网另选同意。授权记录保存在宿主私有文件，不修改原 SQLite schema 或 Provider。绑定的公开状态不包含凭据引用。每次读取需要再次显式触发；失败不会静默使用原生目录。

停用、撤权、插件升级、服务重启、重新绑定，以及已保存连接修订变化，均使旧授权失效。在途请求会被取消或在发布前被拒绝。失效绑定保留可见，宿主可以在插件停用或升级后独立解除绑定；尚有绑定时不可卸载。重新授权不会偷偷更换 Key 或连接。草稿放弃时应显式解除其绑定。

当前编辑表单、自动连接和批量目录已接入独立来源与精确授权；草稿与已保存范围分别核查。静态预设接入见 [统一供应商预设](provider-presets.md)，其选择不产生目录读取权限。原 `fetch_provider_models` 等 96 个旧 IPC 继续使用原生语义，新命令为显式的 `query_provider_catalog` 和 `perform_provider_catalog`。

## 构建与契约导出

在 SDK 根目录使用锁文件构建可选示例；不需要应用 crate。`SERVICE` 指向当前试点 App 随包提供的服务绝对路径，输出使用新路径，签名私钥放在包目录外。

```sh
cargo +1.92.0 build --locked --release --target wasm32-unknown-unknown -p model-catalog-plugin
"$SERVICE" --plugin-sdk componentize target/wasm32-unknown-unknown/release/model_catalog_plugin.wasm /absolute/new/plugin.wasm
"$SERVICE" --plugin-sdk schemas-provider-access /absolute/new/provider-access
```

将 componentized 文件放到包目录的 `plugin.wasm` 后，使用统一的 `keygen` / `pack` / `check` 命令。新契约八份 schema 与 TypeScript 位于 `contracts/domain/provider-access/`，原契约目录保持；`pnpm check:domain` 与 Rust schema 测试核对它们。示例解析 `entries`、`more`、`cursor` 的自定义 JSON，不代表任何真实服务已接通。
