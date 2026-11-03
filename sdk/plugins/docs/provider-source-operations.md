# Provider 来源操作（宿主专用）

此接口把目录来源和 Provider 保存放入同一个可核查操作。它属于桌面宿主，**不会传给 Wasm guest**；外部插件继续实现 `provider.models@1`，不获得凭据、SQL、IPC 或模型发送权限。

契约位置：`contracts/domain/provider-source-operation/`。相关契约分别为 provider-access、provider-connection 和 provider-source。

通过 `local-agent-service --plugin-sdk schemas-provider-source-operation <输出目录>` 导出三个 schema；`scripts/plugin-bindings.mjs --provider-source-operation` 核对 TypeScript 绑定。

## 请求与回执

新增受控命令 `perform_provider_source_operation({ input })`，服务路径为 `POST /api/provider-source-operations`。`input` 只支持两个固定动作：

- `{ kind: "commit", request }`：提交或核查同一编号；已存在的请求不会重新读取目录或发送模型请求。
- `{ kind: "query", request }`：只核查原结果。

`request` 包含：

- `operation`：原有五字段 Provider 操作，保持 `storeId / operationId / expectedSequence / kind / provider`。
- `source`：`{ kind: "builtin" }` 或 `{ kind: "external", catalog: Selection }`。
- `expectedSourceRevision`：最后一次来源状态的令牌；确认没有来源记录时为 `null`。它不替代 Provider 修订、数据库身份或外部授权。

返回原有 Provider Operation 回执；取消继续使用原有取消命令。手动 Save 不读取目录、钥匙串或模型，不生成验证成功记录。Connect 才执行原有受限验证。内置保留原格式识别；外部固定授权的基础协议，无隐式回退。已发送但结果未知时，不把未知当作已拒绝而换模型重发。

## 持久化与恢复

宿主保留独立来源记录和原 SQL 回执。手动保存时，在原 SQL 提交前暂存来源；确认回滚后才恢复原字节。中断后的来源只凭原回执解析，不能根据相似字段猜测成功。显式内置切换也走同一链路，不能分成“先改来源、再保存资料”两次写入。

新私有记录使用版本 2 和 `source-operation-v1` 格式标识；仍读取合法版本 1。未知版本、缺失完整性记录、令牌变化或记录不可读会拒绝操作，不解释成内置来源。原有五字段 SQL 回执与主数据库 schema 不变。

本地恢复键名保持不变；原生旧请求仍保存原字节结构，旧外部请求使用版本 2 信封，明确来源请求使用 `{ version: 3, request: operation, route: { source, expectedSourceRevision } }`。旧解析器保留不认识的信封并阻止接管。重试前重新读取并比对本地原请求与完整来源，不能用当前界面选择覆盖原选择。

## 授权边界

表单先核对已有来源，再显式选择、确认协议和授权。Draft 绑定在凭据准备后创建，只用于其精确范围。保存不把 Draft 授权升级为 Saved；已保存连接需要单独授权当前修订。插件停用、撤权、升级、数据库或后台连接变化时，旧结果不能作为当前结果发布。

原操作和已确认回执独立于插件是否可用。来源记忆不等于访问许可，也不等于模型已验证。关闭未确认操作保留原请求与必要授权，不能自动取消或撤权。

预设接口见 [统一供应商预设](provider-presets.md)。
