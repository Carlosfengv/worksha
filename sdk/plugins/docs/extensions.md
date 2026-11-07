# 协议、配置与 MCP 扩展

安装者使用应用内设置，不需要 Node、Cargo 或浏览器。协议与配置 guest 是无 WASI 的 Wasm Component；MCP stdio 是用户另行选择的本机程序，以当前用户权限运行。

## 包与资源授权

使用统一的 `manifestVersion: 1`、`hostApi: "^1.0.0"` 与 WIT ABI `local-agent:plugin@1.0.0`。声明 `ui.contribution@1` 和下表的一项能力，权限固定为 `ui.contribute`，提供一个 settings 视图。不能混入 Agent、Provider 预设、私有设置或必需插件依赖。签名载荷 `extensions.json` 的 `schemaVersion` 为 1，只选择一项策略。

| 能力                      | 策略                                     | 额外授权                                                          |
| ------------------------- | ---------------------------------------- | ----------------------------------------------------------------- |
| `gateway.protocol@1`      | `protocol: {baseProtocol, path}`         | 具体已有 Provider；私网另选；完整请求 / 响应交给 guest 的明确确认 |
| `config.plan-provider@1`  | `config: {fields: [{pointer, value}]}`   | 原生选择一个 JSON 文件，生成预览后再次确认执行                    |
| `integration.connector@1` | `connector: "mcp-stdio"` 或 `"mcp-http"` | 原生选择连接 JSON，再确认启动；包不能指定程序或网址               |

策略中的其余两项为 null。包沿用 Ed25519、摘要索引、公钥固定、安装日志及统一生命周期；`extensions.json` 参与签名。新接口的 schema 使用 `--plugin-sdk schemas-extensions <目录>` 导出，权威源为宿主 `contracts/plugins/extensions/`。API 主版本与 WIT 通用 ABI 版本各自独立。

构建沿用 SDK 根目录的 `cargo build --locked --release --target wasm32-unknown-unknown`。新增两个 Rust 示例的输出名是 `envelope_protocol_plugin.wasm`、`json_config_plugin.wasm`，用随 App 的 `--plugin-sdk componentize` 转换，再分别打包 `examples/envelope-protocol/package` 和 `examples/json-config/package`。`mcp-stdio` / `mcp-http` 为声明式包，无 Wasm。首次安装仍须独立核对作者公钥。

## 协议调用

先完成 initialize → prepare → activate → health。每次模型请求新建一个 worker：

1. `protocol.open({stream, request})` 返回 `{bytes: number[]}`：转换后的上游 JSON 字节。输入已经由内置协议生成，包含完整工具、自由格式和命名空间字段，不包含宿主 HTTP 认证头。
2. 对每段上游字节调用 `protocol.feed({bytes})`，返回 `{bytes, complete:false}`。输出为所选 baseProtocol 的规范 SSE 或非流式 JSON 字节；guest 必须处理任意字节边界，包括 UTF-8 和多行事件。
3. EOF 时 `protocol.finish({})` 必须返回 `{bytes, complete:true}`。随后 deactivate / dispose。规范协议继续经原有解析、usage、结束状态、跨协议转换和客户端发送路径；不得虚构 usage 或成功结束。

baseProtocol 可为 `responses` / `messages` / `chat-completions`，须与 Provider 相同；随包示例是 Responses 的 NDJSON envelope，其他自定义适配器仍需自己的回放验收。固定 path 仅允许安全的绝对路径片段，并附加到该 Provider endpoint。网络、DNS 固定、凭据与并发由宿主持有；不跟随重定向，不使用系统代理。此能力只接入受管网关路由，未绑定的路径保持原行为。

单次输入 / 输出文档上限 256 KiB，每次输入分片 48 KiB；数字字节数组经过 JSON 编码后仍须满足 1 MiB RPC 帧及 worker 内部 512 KiB 输出限制，因此某些输出会先达到编码预算。上游累计 8 MiB；每次调用 10 秒，整个 worker 300 秒，fuel 100000000、CPU soft10 / hard11秒、线性内存128 MiB，worker RSS 每100ms采样、超过512 MiB终止。全局4个 worker、单插件串行，在途升级返回资源忙，流结束后可重试升级；停用 / 撤权和客户端断开会终止 worker，不自动重发模型请求。

原始凭据不传入 guest；上游回显宿主凭据（包括跨片和 JSON 转义）时拒绝继续交付 guest。此防护不将插件转换的用户内容变为可信内容。插件无法自行选择网络目标、HTTP 头或发起第二次网络请求。

绑定冻结 digest、授权版本和 Provider 版本。修改 Provider、重新授权、停用或升级后旧绑定拒绝请求，必须解除并重新授权。卸载前先解除绑定。`plugin-protocol-bindings.json` 单独原子保存，最多128项；扩展数据库损坏不阻止主服务或未绑定请求，绑定失败不会静默回落。

## 有限配置计划

`fields` 最多8个简单 JSON pointer，仅允许字母、数字、下划线和短横线的对象键；禁止路径转义、原型键和数组操作。`value` 为 endpoint / model / credential。`config.propose` 仅收到 `{edits, credentialPlaceholder:"${HOST_CREDENTIAL}"}`，返回完全相同的有限 `{edits}`。guest 不读取原文件、路径或原始密钥，宿主拒绝任何额外 / 不同的编辑。

原生选择的文件须为现有、当前用户持有、单链接、无符号链接的 JSON，最多256 KiB。预览无写入；执行重新核查文件身份 / 内容 / 模式、Provider 和授权，宿主仅在指定 credential 字段填入密钥。原字节由宿主用独立随机密钥加密备份；密钥由凭据保险库持有，确认执行后原子替换并回读。保留其他字段；写入凭据时将文件权限收紧为0600，其余计划保留原权限，恢复还原原始字节和权限。已应用 / 已恢复操作可幂等重试，状态不明明确显示待核查，不创建重复操作。

插件停用、撤销信任或卸载后，宿主备份与恢复入口独立可用。用户随后改动过的文件拒绝覆盖。扩展库保留最多64个计划，达到上限拒绝新增，不自动删除恢复证据。备份依赖同一主机凭据保险库；复制数据库本身不等于备份密钥也可用。扩展状态保存在 `plugin-extensions.sqlite3`，仓储损坏时保留错误和恢复证据。

## MCP 接入与观测

用户自行创建连接 JSON 后在原生窗口选择。stdio 例子：

```json
{ "transport": "stdio", "command": "/absolute/mcp-server", "args": [] }
```

HTTP 例子：

```json
{
  "transport": "http",
  "url": "https://example.test/mcp",
  "allowPrivateNetwork": false,
  "providerId": null
}
```

stdio 可执行文件须为绝对路径、当前用户或 root 持有、非组 / 全局可写的普通可执行文件；不经 shell，不继承环境，PATH 固定系统目录。选择时冻结文件摘要 / 身份，启动再复核。参数也由所选文件固定；若参数包含其他脚本，其运行属于用户授权的本机程序行为，并未成为 Wasm 沙箱。不要在连接 JSON 中放密钥或使用不可信程序。

HTTP 禁止重定向 / 系统代理，DNS 地址固定，私网需显式选择。可选 providerId 只允许同源 endpoint 的现有 Provider，宿主按次核对其版本并注入凭据。使用本机原生保存对话框导出有随机认证凭据的回环 MCP HTTP 入口；密钥不返回 WebView，只交给用户指定的可信客户端。

管理页不会主动执行 tools/call；启用入口不等于握手成功，目录未知不显示0。客户端 initialize、tools/list、通知不计为执行；外部客户端发起的 tools/call 记录结果分类、有效耗时、RPC ID 和连接身份，输入 / 输出正文不持久化，模型 Token 未知。重新使用已完成 RPC ID 是新调用；重复在途 ID 被拒绝。显式 cancelled 通知转发给上游并记录取消；超时不自动重试。断开网络本身不等价于撤回上游工具副作用。

stdio JSON-RPC 分行、Streamable HTTP POST/GET/DELETE、JSON / SSE、CRLF、Session / Version / Last-Event-ID、通知和客户端响应转发均经过宿主载体。stdio 单次等待30秒；帧1 MiB、在途64、发送队列32、通知环64。HTTP 工具请求等待有界，GET 通知流可持续；断线重连由客户端负责，不自动重复工具执行。数据库保留最近10000条元数据，界面分页展示最近100条。

外部 MCP 来源当前独立展示。没有可靠的 Agent / Session 关联证据时，不合并到原生 MCP 次数或 Token；跨来源统一身份与去重仍为 partial，不能把两份观测相加。stdio 通知缓冲溢出、HTTP 会话续传与不同服务实现仍需各自接入验证。

## 未开放能力与回退

清理与直接兑换的原执行路径尚未完成，因此外部执行保持 unsupported。外部 SQLite 来源、任意文件 / shell 权限、前端脚本、自动工具执行和插件市场未开放。

DSH Desktop 0.2.2-rc.1 使用[只读桥接插件](../examples/dsh-desktop/README.md)。DSH 自己加载适配器，以官方 Session Controller 发布本机快照；Local Agent 的来源包读取明确授权的文件。无需复制桌面认证 Cookie、读取内部数据库或给 Local Agent 捆绑 DSH / Node 运行时。依据：[Desktop](https://github.com/deepseek-ai/deepseek-harness/blob/master/apps/desktop/README.md)、[Session Controller](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/api/session-controller/README.md)。MCP 依据：[2025-11-25 传输规范](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)。

回退前停止 MCP、解除外部协议绑定、按需恢复配置，再正常退出应用；保留扩展文件及保险库。再次使用时需显式重新授权，不能通过删除仓储绕过格式检查。
