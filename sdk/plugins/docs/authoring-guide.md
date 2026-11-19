# 创作本地插件

## 声明式规则包

从 [项目与会话看板](../examples/session-kanban/package/plugin.yaml)、[日志归档](../examples/gateway-log-archive/package/plugin.yaml) 或 [目录型 Agent](../examples/cursor-dir-agent/package/plugin.yaml) 复制规则。包源目录只需一个 `plugin.yaml`、`plugin.toml` 或 `plugin.jsonc`，顶层包含 `manifest` 和 `platform`。已有的 `manifest.json` 包可以直接添加同样受签名保护的 `platform.json`。

```sh
local-agent-service --plugin-sdk keygen ./publisher.key
local-agent-service --plugin-sdk pack ./package ./example.laplug ./publisher.key
local-agent-service --plugin-sdk check ./example.laplug
```

在桌面插件中心上传签名包，核对发布者公钥并启用。新包声明 `ui.platform@1`。包含集合或私有键值的包同时声明 `storage.plugin-data@1`。保持发布者私钥独立于包源目录。

YAML 支持缩进映射、列表、内联映射和列表、JSON 标量及注释。路径、以数字起始的 ID 和可能被识别为标量的文字可以加引号。JSONC 接受注释和尾随逗号。TOML 使用普通表与数组。包编译将规则规范化成 JSON，以同一套 Rust 契约校验。查看 [生成的契约](../../../contracts/plugins/platform)。

`collections` 定义字段类型和必填项。看板的 `groupBy` 指向必填枚举，列顺序来自该字段的 `options`。按项目和会话注入时，`filterByContext` 指向同种宿主实体的引用字段。导航插槽支持 `observation` 和 `supply`。引用字段仅存对象 ID，实体摘要在单独授权后按宿主租约读取。

目录型 Agent 声明 `source.directory.read`，使用 `sourceMapper` 的受限点路径从 JSON 或 JSONL 提取会话元数据。目录由用户在 macOS 文件夹对话框内选择。匹配相对路径，支持 `*` 和递归路径段 `**`。单文件上限 2 MiB，单次扫描 16 MiB，最多 256 个匹配文件。

`project` 读取来源报告的绝对路径，生成来源内的项目标识与名称。`createdAt`、`updatedAt` 和 `activity.occurredAt` 使用 RFC 3339 时间。`activity.phase` 接受 `running`、`thinking`、`generating`、`tool`、`idle` 和 `unknown`。前四种状态在新鲜来源报告的 30 秒内可显示为当前活动。目录来源无法观测网关计时或实时 Token 速率。

`conversation.messages` 定位单条会话记录中的消息数组，`conversation.role` 与 `conversation.text` 定位每条消息中的角色和正文。正文在打开会话时从同一个授权文件分页读取。文件变化、更换目录或撤销授权后需重新读取目录快照。

日志归档支持 JSONL、CSV 或 Wasm 转换，按天或小时写入授权目录。一次检查写入一个完成的切片文件，文件与游标可以安全重试。导出的模型、Agent 与请求标识使用摘要。规则过滤仍使用宿主记录里的原名称。

## Rust 和 TypeScript Wasm 组件

Rust 使用此仓库的 `wit-bindgen` 和 [WIT](../wit/plugin.wit) 生成 `Guest` 接口。[日志转换示例](../examples/gateway-log-archive/src/lib.rs) 使用 `define_plugin!` 实现生命周期。业务函数接收方法名和 JSON 文本，返回 JSON 文本或有限错误码。

```sh
pnpm prepare:service
pnpm prepare:plugins
local-agent-service --plugin-sdk pack \
  sdk/plugins/examples/gateway-log-archive/otel-package \
  ./otel-archive.laplug ./publisher.key
```

TypeScript 应使用能消费同一 WIT 的组件工具，在作者环境将实现编译成不导入 WASI 的 Wasm 组件。导出 `invoke(method: string, payload: string)`，初始化原样回显参数，准备、启用、健康检查、停用和释放返回 `{"ready":true}`。如果编译产物是带 WIT 元数据的核心 Wasm，先用 `local-agent-service --plugin-sdk componentize input.wasm package/plugin.wasm` 转成组件。打包后的应用无需 TypeScript 运行时。

声明 `wasm.render` 后，`platform.render` 可以返回本页已有的 `recordIds` 决定顺序和子集。声明 `wasm.action` 后，`platform.action` 返回 `accept` 及可选替代 `values`。授权后的对象句柄仅在当次调用内可由 `domain.read` 读取。存储写入在动作提交时与宿主变更一起校验和提交。带目录授权的组件可在该调用中通过 `source.list_files` 和 `source.read` 读取声明范围内的有限分块。

`sink.transform` 接收 `records`，每条记录返回一行文本。使用确定性转换，便于写入与游标提交之间发生中断后重试。此阶段没有宿主 I/O。

[复合供应商示例](../examples/unified-provider/package/plugin.yaml) 在一个签名包中声明预设、目录、协议和额度。启用后按需在插件中心和对应连接页面单独完成能力授权。其示例 URL 用于展示包格式。
