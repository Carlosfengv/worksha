# Local Agent

本地 AI Agent 网关与可观测性桌面应用。统一管理模型连接，查看会话与调用，追踪用量、费用和运行状态。

Local Agent 使用 Tauri 2 和 Rust，界面与本地服务随 macOS 应用包分发。应用运行时无需 Node.js、Vite 或外部浏览器，业务数据保存在本机 SQLite 中。

当前版本为 **0.1.8**，处于开发与验收阶段，尚未完成完整功能及正式发布验收。内容按天到期、完整归档恢复和部分客户端能力仍未完成。

## 功能概览

| 功能           | 已接入能力                                                                  |
| -------------- | --------------------------------------------------------------------------- |
| 任务工作台     | OpenCode / Claude Code CLI 项目任务、流式对话、审批、中止、续聊与交接       |
| Agent 与会话   | 原生来源授权、实时活动、会话检索、分页对话、工具与 MCP 记录，以及跨页面定位 |
| 项目与请求     | 项目关联、文件与 Git 信息、请求筛选、调用详情、诊断和 CSV 导出              |
| Provider 管理  | 供应商与自定义连接、模型目录、连接验证、凭据管理及本机配置导入              |
| 配置与恢复     | 切换预览、加密备份、原配置恢复、操作回执与未知结果核查                      |
| 模型服务与路由 | 跨 Provider 模型组合、固定入口、动态路由、版本发布与策略恢复                |
| 用量与账户     | Token、计时、费用，以及各来源提供的套餐、额度和余额信息                     |
| 插件扩展       | 本地签名包、独立授权、Wasm 执行、完整只读来源与详情扩展                     |
| 常驻网关       | 可选的用户级常驻服务、局域网模型入口和独立客户端授权                        |

不同 Agent、版本和来源提供的能力不同。安装、授权、连接成功和模型验证分别表达；缺失、过期或无法核验的数据保留明确状态。

任务看板新建的 OpenCode 任务使用官方默认本机服务，能在连接同一服务的 OpenCode Desktop 搜索、查看和继续。App 保存会话绑定以便重开恢复，连接和模型不可用时显示原因。OpenCode 从 2.0.0 起提供这一连接机制，兼容性按实际接口判断。已有 App 会话继续使用原存储位置，旧版共享服务仍支持手动连接。

OpenCode 共享任务会显示模型入口是否经过当前 App 网关。指向其他 App 网关的本机入口会阻止创建和发送，保留原模型；直连外部服务会提示记录范围。任务可按原生会话 ID 查看模型请求，请求详情也可打开看板已知的原会话。

新建的 Claude Code 任务把原生记录保存在所选用户配置目录，支持 CLI 按会话 ID 恢复，工作台回读后续消息并检查原生客户端占用。创建和续聊前校验网关归属，模型请求按原生会话 UUID 关联任务。接续入口提供保留配置目录、项目、会话和模型的 CLI 命令，并分别展示 CLI 与 Claude Desktop 的网关状态。Desktop 使用官方第三方推理 Profile 接入网关；当前 API / 网关会话通过复制上下文交接到 Desktop，不支持运行中 Desktop 会话接管。

## 快速开始

### 环境要求

| 工具    | 版本或要求                                                                         |
| ------- | ---------------------------------------------------------------------------------- |
| 系统    | macOS，安装 Xcode Command Line Tools                                               |
| Node.js | 24.19.0，以 [.nvmrc](.nvmrc) 为准                                                  |
| pnpm    | 11.8.0，以 [package.json](package.json) 为准                                       |
| Rust    | 1.92.0，包含 rustfmt 和 Clippy，以 [rust-toolchain.toml](rust-toolchain.toml) 为准 |

### 启动开发窗口

在仓库根目录运行：

```sh
pnpm install --frozen-lockfile
pnpm tauri dev
```

开发命令自动编译本地服务并启动原生窗口，界面支持热更新。`pnpm dev` 和 `pnpm preview` 仅辅助界面开发，浏览器环境不开放本机能力。

### 打包应用

```sh
pnpm tauri build --debug --bundles app
```

该命令自动构建随包服务和前端资源，生成 macOS 开发包：

```text
target/debug/bundle/macos/Local Agent.app
```

双击应用包即可启动。Developer ID 签名、Apple 公证和正式更新渠道仍需单独完成与验收。原生测试结束后按 [测试 App 回收流程](docs/test-app-cleanup.md) 清理本次产物。

## 开发检查

首次 Rust 桌面检查前先准备本地服务；首次浏览器回归前安装 Chromium：

```sh
pnpm prepare:service
pnpm exec playwright install chromium
```

运行主要检查：

```sh
pnpm check
pnpm test:display
pnpm test:frontend
pnpm check:rust
pnpm test:rust
pnpm test:service
pnpm build
```

`pnpm check` 包含类型、ESLint、Zeron 文件完整性、语义样式、依赖边界、IPC 契约、生成绑定和格式检查。Rust 检查包含格式与严格 Clippy；`test:rust` 覆盖完整 workspace，`test:service` 验证独立进程及持久化行为。

插件回归另需生成 Wasm 夹具，在准备服务后执行：

```sh
rustup target add wasm32-unknown-unknown
pnpm prepare:plugins
pnpm test:plugins
pnpm test:plugin-cache-capacity
pnpm test:domain-isolation
```

策略故障夹具由 `pnpm prepare:policy-fixtures` 生成。专项命令见 `package.json`，自动检查范围以 `.github/workflows/ci.yml` 为准。测试使用临时数据与模拟上游，原生交互须在实际打包窗口验证。开发约定见 [AGENTS.md](AGENTS.md)。

## 技术与结构

前端采用 React 19、TypeScript、Vite、Tailwind CSS 4 和 Zeron UI；桌面采用 Tauri 2；后台采用 Rust、Tokio、Axum 和 SQLite WAL。

| 路径                            | 职责                                           |
| ------------------------------- | ---------------------------------------------- |
| `src/app/`                      | 应用组合、路由、外壳和初始化                   |
| `src/features/`                 | 领域页面、业务组件、状态、IPC 与类型           |
| `src/shared/`                   | 共享界面、Hook 与桌面通信                      |
| `src/components/ui/`、`.zeron/` | Zeron 官方组件与安装记录                       |
| `src-tauri/`                    | 原生生命周期、受控命令与本地服务通信           |
| `crates/local-agent-core/`      | 可独立测试的业务规则、来源解析、存储与配置事务 |
| `crates/local-agent-service/`   | 本地 HTTP 服务、网关、插件宿主与后台任务       |
| `contracts/`                    | 版本化协议、schema 与生成类型                  |
| `sdk/plugins/`                  | 插件 SDK、示例与隔离测试夹具源码               |
| `resources/`                    | Agent、Provider 与错误契约等随包目录           |
| `tests/`、`scripts/`            | 回归测试、校验与构建工具                       |

前端通过受控 Tauri 命令访问本地服务，凭据由后台管理。来源读取、配置变更和插件授权遵循各自的确认流程。测试输出放在 `.work/`；依赖、构建包、缓存和生成的 Wasm 不进入版本库。

## 许可与安全

项目许可见 [LICENSE](LICENSE)。第三方依赖与资源许可见 [第三方说明](THIRD_PARTY_NOTICES.md)、`licenses/` 和 `public/licenses/`。

安全问题的报告要求与渠道现状见 [SECURITY](SECURITY.md)。
