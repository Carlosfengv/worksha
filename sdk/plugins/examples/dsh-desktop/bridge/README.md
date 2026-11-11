# Local Agent 的 DSH 只读连接

仅支持 DSH Desktop 0.2.2-rc.1。通过 DSH 官方 `sessionController.list` 和 `events` 读取已有会话、项目、最近 40 条消息范围及来源报告的响应和状态；不创建或恢复 Agent，不发送消息，不读取 DSH 的凭据、内部数据库或配置内容。

会话新增、移除、收到用户消息或运行状态变化时发布到 `~/.local/share/local-agent/dsh-desktop/facts.jsonl`，并每 5 秒更新一次。目录 0700、文件 0600。Worksha 在用户授权该文件后读取。确认统一展示后，重开 Worksha 时会核对并恢复同一文件的授权。正文仅保留每个会话最近 40 条可识别消息，每条上限 4096 字符。首页任务使用最近一条用户消息，会话列表保留原会话标题。

1.3 版桥接读取 DSH 的输出事件时间与服务商上报的用量。首个输出到达时可更新 TTFT。收到输出 Token 数后，可按首出之后的生成时长计算步骤均速，完成后还可显示输出 Token 数。缺少所需的时序或用量时，对应指标显示为未知。桥接只导出时间和用量，不附加原始流片段。

运行上下文在正文中单独显示。MCP 执行记录包含工具名、时间、可确认的执行结果与关联会话。参数、工具返回内容和 MCP 配置均不导出。

通过 DSH 桌面安装所带的命令执行 `dsh plugin --profile desktop add /absolute/path/to/bridge`，再将 `worksha-dsh-readonly-bridge` 加入该 profile 的 bundle 选择。优先使用插件管理界面完成安装与启用；未开启配置热更新时需要正常重启 DSH。移除此 bundle 即停止发布，保留导出文件供核查；在 Local Agent 撤销来源权限会立即停止其读取。
