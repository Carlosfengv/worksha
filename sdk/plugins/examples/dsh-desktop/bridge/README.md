# Local Agent 的 DSH 只读连接

仅支持 DSH Desktop 0.2.2-rc.1。通过 DSH 官方 `sessionController.list` 和 `events` 读取已有会话、项目、最近 40 条消息范围及来源报告的响应和状态；不创建或恢复 Agent，不发送消息，不读取 DSH 的凭据、内部数据库或配置内容。

每 5 秒发布到 `~/.local/share/local-agent/dsh-desktop/facts.jsonl`，目录 0700、文件 0600。Local Agent 只有在用户选择这个确切文件并授权后才能读取。正文仅保留每个会话最近 40 条可识别消息，每条上限 4096 字符；未提供的用量保持未知。历史截断标记不完整。运行状态为 DSH 报告，不提供宿主确认的 TTFT 或 Token 实时速率。

通过 DSH 桌面安装所带的命令执行 `dsh plugin --profile desktop add /absolute/path/to/bridge`，再将 `worksha-dsh-readonly-bridge` 加入该 profile 的 bundle 选择。优先使用插件管理界面完成安装与启用；未开启配置热更新时需要正常重启 DSH。移除此 bundle 即停止发布，保留导出文件供核查；在 Local Agent 撤销来源权限会立即停止其读取。
