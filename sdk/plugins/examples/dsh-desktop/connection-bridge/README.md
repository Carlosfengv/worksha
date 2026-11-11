# DSH 模型连接桥接

适配 DSH 0.2.2-rc.1。这个伴随包与会话读取桥接分别启用。用 DSH 自带的 `dsh plugin --profile desktop add /absolute/path/to/connection-bridge` 安装伴随包，通过 DSH 插件面板启用。

在 Local Agent 的 Agents 列表中选择 DSH 的 Provider 或模型服务，授权并验证后预览默认模型变更。随附模型连接使用 `~/.local/share/local-agent/dsh-desktop`。使用自定义输出目录时，已有会话来源可通过所授权文件的同级目录连接。连接授权限本次 Local Agent 运行。撤销连接授权会停用该 Agent 已发布的本机入口，重新授权后需要准备新连接。

桥接 1.1 起，先在 DeepSeek Harness 登录官方账号，再在 Local Agent 的同一选择器选择「DeepSeek 官方账号」。模型列表来自 DSH 当前账号。确认切换后，DSH 将选中的账号模型用于新会话，账号与套餐由 DSH 管理。

切换范围是新会话的默认模型。桥接通过 `settingsController.mutate` 写入专用自定义 Provider，保存单用途本机网关令牌，然后通过 DSH 默认模型服务应用选择并回读。已有会话各自保留模型选择。已发布的入口在未撤销时可以继续服务引用它的会话。

- 上游 API Key 由 Local Agent 管理。DSH 只持有该绑定的本机入口令牌。
- 桥接在输出目录提供私有 Unix 套接字和配对令牌。套接字接受版本化的读状态、应用、恢复操作。它只登记对应绑定的回环网关地址和模型目录。
- 预览绑定目标版本、桥接实例、DSH 配置修订、默认选择和连接授权。确认前目标变化会使预览失效。
- 本次运行内可核对未知结果和恢复到预览前的选择。重启后可在 DSH 原生界面核对或调整配置。切换成功与实际请求是不同状态。
- 该接口不接收任务创建、消息发送或任意设置编辑。
