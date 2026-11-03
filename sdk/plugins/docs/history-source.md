# 完整只读历史来源

使用统一插件清单、签名域与 API；`source` 描述来源和产品，不能与路由策略、网络、配置或 UI 能力混用。完整示例见 `examples/full-history/package/manifest.json`。

必须声明 `domain.agent-directory@1`、`domain.session-source@1`，可选声明项目、响应、活动与 MCP 事实能力；`requires` 固定为 `host.source-broker@1` 和 `host.domain-facts@1`，权限仅为所选单个历史 JSONL 的 `source.content.read`。产品标识在插件命名空间中唯一；项目路径仅作为报告证据，不授予本机访问权限。

## 构建与安装

在 SDK 根目录构建组件，私钥放在包目录外。`SERVICE` 指向当前应用随包提供的服务绝对路径。

```sh
cargo +1.92.0 build --locked --release --target wasm32-unknown-unknown -p full-history-plugin
"$SERVICE" --plugin-sdk componentize target/wasm32-unknown-unknown/release/full_history_plugin.wasm /absolute/package/plugin.wasm
"$SERVICE" --plugin-sdk keygen /absolute/author.key
"$SERVICE" --plugin-sdk pack /absolute/package /absolute/history.laplug /absolute/author.key
"$SERVICE" --plugin-sdk check /absolute/history.laplug
"$SERVICE" --plugin-sdk schemas-sources /absolute/contracts/sources
```

来源包必须签名。公钥需经作者独立渠道核对，签名有效不代表已核实组织身份、资源已授权或业务行为已验证。完整来源管理使用能力专属入口；安装、启用、资源授权、统一展示与元数据保存分别确认。启用仅进行无资源预检，目录和普通查询不执行 guest。

## 读取与失效

统一 worker 使用 `initialize`、`domain.scan`、`domain.read-conversation` 白名单；不开放 WASI、网络、凭据、SQL、进程或模型发送。来源以随机引用读取，扫描结果按 `contracts/domain/facts/` 的宿主规则核查。只有完整扫描成功才替换投影，失败保留上一完整代次并标记失败。正文按需分页读取，不持久化。

资源授权仅限本次运行。重开后保留包与启用意图，需重新启用、选择来源并授权展示。可选保存只保留已授权元数据，过期与读取失败保持明确状态。更换文件、版本或授权使旧游标、正文引用和冻结快照失效；重新授权不会使旧结果恢复可读。

完整来源仓储为 `plugin-sources/` 和 `plugin-source-host.sqlite3`。不同能力共享发布者信任及全局安装容量，但独立持有资源和运行状态；撤销信任影响该发布者的所有包。包合计最多 64 个、压缩容量 256 MiB，同一插件 ID 最多三个发布版本。未知文件和用户原始来源保留，不能通过删库修复容量或身份错误。

## 统一查询与导出

统一查询消费已发布且单独授权的事实，不自动刷新来源或执行 guest。响应查询按明确范围和统计单位分页；外部报告不能提供宿主实测计时、费用或精确网关关联，未知值保持未知。冻结快照继续校验授权与采集代次。

统一范围 CSV 使用宿主安全保存链路；系统弹窗选择目录，路径不进入插件。导出契约为 `contracts/domain/request-export/`，可用 `schemas-request-export` 导出。DSH、Developer ID 与正式公证仍未验证。
