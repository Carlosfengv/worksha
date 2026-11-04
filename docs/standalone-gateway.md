# macOS 独立常驻与局域网网关

当前实现复用 `local-agent-service`，不依赖浏览器、Node.js、Vite 或桌面窗口常开。默认独立数据目录是 `~/Library/Application Support/app.localagent.gateway`，与现有桌面服务分开。首次启用需显式配置本机私有 IP 和固定端口，不自动开放系统防火墙，不修改系统网络或其他 Agent 的配置。

## 安装与管理

以下 `SERVICE` 指向应用包 `Contents/MacOS/local-agent-service`，或开发构建的 `target/debug/local-agent-service`。将地址替换为网关所在 Mac 的局域网私有地址，端口应独立且未被占用。

```sh
SERVICE='/Applications/Local Agent.app/Contents/MacOS/local-agent-service'
GATEWAY_DATA="$HOME/Library/Application Support/app.localagent.gateway"
"$SERVICE" gateway configure --data-dir "$GATEWAY_DATA" --listen 192.168.1.10:43210 --concurrency 16
"$SERVICE" gateway install --data-dir "$GATEWAY_DATA"
"$SERVICE" gateway status --data-dir "$GATEWAY_DATA"
```

`configure` 在服务运行中拒绝覆盖；私有 IP 变化后需先停止所属服务，再修改配置并启动。端口冲突直接失败，不抢占、杀进程或自动换端口。也可用 `gateway serve --data-dir ...` 前台运行同样的独立服务。

`install` 为此数据目录复制稳定的服务程序，创建当前用户的独立 LaunchAgent。标签由数据目录摘要生成；`RunAtLoad` 在用户登录后启动，失败退出由 launchd 重启。正常停止不会被立即重启。它是用户登录会话内的服务，用户注销和机器睡眠会影响可用性；钥匙串锁定时凭据操作明确失败。当前不是开机前无人登录的系统 LaunchDaemon。

```sh
"$SERVICE" gateway stop --data-dir "$GATEWAY_DATA"
"$SERVICE" gateway start --data-dir "$GATEWAY_DATA"
"$SERVICE" gateway uninstall --data-dir "$GATEWAY_DATA"
```

操作只使用该目录对应的 launchd 标签和经过核对的 plist，不枚举或停止其他网关。卸载保留配置、客户端和请求历史。更新程序时先停止并卸载此服务，再用新版本重新安装；当前没有不停机升级。

日志位于该目录的 `logs/service.log`、`logs/service-error.log`。运行身份与管理令牌保存在受保护的 `runtime/`；目录 0700、敏感文件 0600。不要分发管理令牌或 `service.json`。

## 桌面管理

桌面 App 仍通过本机管理接口访问服务。可在启动进程时设置 `LOCAL_AGENT_DATA_DIR` 为上述目录，连接已运行的同版本常驻服务；没有该变量时继续使用原桌面目录。服务没有启动时桌面提示错误，不另起临时副本。现有相同应用实例会被唤起，因此切换数据目录应在该应用实例退出后进行。

设置页显示常驻状态、局域网地址、并发及客户端列表。创建客户端时选择名称、Agent、Provider、一个已启用模型和并发数，原生保存对话框导出包含 `baseUrl`、`apiKey`、`clientId`、Agent 与模型的 JSON 文件。取消不创建客户端；文件保存失败会尝试撤销本次新密钥，撤销失败明确显示需处理的客户端 ID。密钥不返回 WebView。CLI 可一次授权多个模型。

```sh
"$SERVICE" gateway providers --data-dir "$GATEWAY_DATA"
"$SERVICE" gateway client-add --data-dir "$GATEWAY_DATA" --name '工作室笔记本' --agent claude --provider PROVIDER_ID --models MODEL_ID --concurrency 2
"$SERVICE" gateway client-revoke --data-dir "$GATEWAY_DATA" --id CLIENT_ID
```

`client-add` 仅在创建响应中显示一次下游密钥；`status` 不回显密钥。磁盘仅保存随机密钥的 SHA-256 摘要。每个客户端绑定一个已保存 Provider 和显式模型集合；以后新启用的模型不会自动获得授权，停用或删除的模型不再可用。撤销立即拒绝新请求，已开始的请求可继续完成。当前创建上限 128 个，已撤销身份保留历史，未实现轮换、删除和预算管理。

## Agent 接入

客户端支持自定义模型端点时，填写 `http://网关私有IP:端口/v1` 和自己的客户端密钥。当前为显式私有地址上的 HTTP，仅用于受控局域网；未提供 TLS、公网监听、浏览器 CORS 或远程管理功能。

- `claude`、`opencode` 客户端：`POST /v1/messages`，支持 Anthropic 的 `?beta=true`。Claude Code 可通过独立进程的 `ANTHROPIC_BASE_URL=http://IP:PORT`、`ANTHROPIC_AUTH_TOKEN=客户端密钥`、`ANTHROPIC_MODEL=授权模型` 接入；不需要向客户端提供上游 Provider 密钥。
- `codex` 客户端使用 `POST /v1/responses`。模型目录使用 `GET /v1/codex/models.json?client_version=<版本>`，接受与桌面配置适配器一致的版本范围，包括 0.163.x、0.164.0、0.165.0、0.166.0 和 0.166.0-alpha.5.1。兼容性通过离线协议回放核对，模型 ID 须在客户端授权范围内。
- `GET /v1/models` 仅返回该客户端仍有效的授权模型。
- Bearer 或 `x-api-key` 认证；管理密钥不能代替客户端密钥。Origin、错误 Host、未授权模型和不适用的协议会被拒绝。

当前不提供 Chat Completions/Gemini 原生下游入口，也不把 Antigravity App 的登录账号或原生记录转换为可共享 Provider。上游复用现有 Messages / Responses / Chat Completions 转发与转换规则。

## 并发与记录

本机转发继续使用原来的 64 个槽位；局域网使用另一组槽位（默认 16，可设 1–64），每客户端另设 1–16。额度不足立即 429，不建立无界队列。槽位跟随响应体生命周期，正常结束、失败、取消、背压超时都会释放。局域网活动不冒充本机 Agent 的配置切换占用。

这是入口并发隔离，进程 CPU、存储、网络及同一 Provider 的上游配额仍然共享；没有承诺上游限流或带宽层面的完全隔离。

成功认证并进入转发的请求沿用现有元数据记录，增加可信客户端 ID、名称和 TCP 对端 IP；这些值由服务端授权记录与连接生成，不信任客户端自报身份。Session 和逻辑请求 ID 按客户端命名空间隔离。未提供可靠 Session ID 时仍保持未知；不读取远端文件、原生会话或 MCP 历史。

请求列表、详情、名称/ID/IP 搜索和 CSV 导出保留客户端归属；保存请求时不保存凭据或消息正文。撤销与重启不会删除历史。

```sh
"$SERVICE" gateway requests --data-dir "$GATEWAY_DATA"
"$SERVICE" gateway requests --data-dir "$GATEWAY_DATA" --id REQUEST_ID
```

停止时沿用最长 10 秒转发排空及后续元数据提交预算。桌面退出不向带 `persistent` 身份的服务发送停止命令；明确停止／卸载服务才影响该服务自己的活动请求。

## 验证边界

`node scripts/test-lan-gateway.mjs --launchd` 使用全新临时目录、独立私有地址端口、临时来源目录及本机 HTTP 测试上游。覆盖真实 TCP 流式转发、协议回放、每客户端与总并发、双向本机/局域网隔离、撤销、取消、上游失败、搜索/导出、重启持久化和实际 launchd 安装/失败重启/停止/卸载。

真实模型调用和第二台局域网设备的防火墙、跨机连通需分别验证，同机私有 IP 测试不能替代。
