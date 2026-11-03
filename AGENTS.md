# Local Agent 开发约定

## 技术与范围

- 桌面采用 Tauri 2，前端采用 React 19 / TypeScript / Vite / Tailwind CSS 4 / Zeron UI，后台采用 Rust / Tokio / Axum / SQLite WAL。
- 产品交付为 macOS 桌面 App，界面与本地服务随应用包分发；运行不依赖浏览器、Node.js 或 Vite。开发入口为 `pnpm tauri dev`，交付和功能验收以打包的原生窗口为准。浏览器页面仅可辅助检查布局，不提供 WebUI 产品或本机能力桥接。
- 项目用途、已接入能力和开发入口见 `README.md`，模块职责见 `docs/architecture.md`。具体功能范围以用户确认的需求为准；历史方案、原型、需求拆解、计划、实施进度和验收记录在仓库外保存，不随源码提交。
- 工程初始化和某一条链路通过，不代表全部功能完成。未实现或未验证的能力必须保留明确状态，不因移出计划文档缩减既定需求。

## Zeron UI

- 使用锁定版本的官方 `zeron-ui` CLI 和实际消费者别名安装组件。安装前查看目录、实际源码 / 类型及计划。
- `.zeron/install-state.json` 保存官方安装记录。`src/components/ui/`、安装记录中的共享 hooks / lib 和 `src/styles/globals.css` 为受管理区域；通过公开 API、插槽、业务组合和主题入口适配，不直接改组件内部样式或刷新基线以绕过检查。
- 业务样式使用 Zeron 语义颜色、字体、surface、状态与焦点变量。布局间距和圆角遵循 Tailwind 原生尺度；不建立第二套颜色和控件尺寸系统。
- 一个应用区域使用一套 AppShell，遵守布局直接子级和滚动归属。图文按钮通过公开图标参数组合。
- 交互型记录列表使用官方 DataTable；静态对账矩阵可用 Table。完整本地集合与后端游标分别处理，不对当前页冒充全量排序 / 分页；大候选字段用 Combobox。选型和公开 API 接入见 `docs/zeron-usage.md`。
- `src/app/` 负责应用组合；`src/features/<功能>/` 聚合页面、业务组件、必要 Hook、普通规则、`api.ts` 与 `types.ts`；`src/shared/` 不反向依赖 app / features。跨功能使用明确的领域 API / 类型 / 组件，不导入其他页面，不产生运行时循环。不要给每个基础组件增加无业务意义的包装层。
- 同时保留加载、错误、未知、过期、无权限和确认无数据的状态，不把未接入显示为已验证或已运行，不在正式界面增加演示控制。

## 数据与验证

- 前端通过受控 Tauri 命令访问本地服务；后台凭据不返回界面。管理接口验证令牌、Host 和 Origin。
- 发现本机目录不等于连接成功。读取会话、改写 Agent 配置、清理或兑换必须遵循用户授权的范围与确认流程。
- 测试使用临时目录和受管副本，不使用用户真实券、API 账户或原生会话做破坏性验证。
- 真实模型调用验收使用 Claude Code 和 OpenCode。除非用户重新明确指定，不启动 Codex CLI / TUI 或 Codex 桌面端的真实调用测试；Codex 相关逻辑使用离线夹具与回放检查。
- Node 版本见 `.nvmrc`，包管理器为 pnpm；Rust 版本见 `rust-toolchain.toml`。保持锁文件可复现。
- `pnpm check`：业务与测试 TypeScript、业务 ESLint、Zeron 官方文件一致性 / 语义样式、依赖边界、IPC 名称 / 参数 / 注册及业务格式检查。`pnpm format` 跳过受管理文件。第三方安装源码参与类型检查，业务层未使用变量等由 ESLint 检查。
- IPC 仅由功能 `api.ts` 经 `shared/lib/desktop-command.ts` 调用；现有命令契约夹具为 `tests/ipc-contract.json`。调整前评估兼容性，不能仅刷新夹具绕过检查；响应仍需 Rust 序列化和服务行为验证。
- 桌面 `commands/` 保持 IPC / 原生适配边界；`backend.rs` 持有 sidecar 与认证通信。service 的 routes / state / api / tasks 各司其职，core 保持可独立测试，不增加无需求的 crate 或抽象。
- 首次 Rust 桌面检查前先 `pnpm prepare:service`。`pnpm check:rust` 执行 fmt / 严格 Clippy，`pnpm test:rust` 验证完整 workspace；`pnpm test:frontend` 和 `pnpm test:display` 验证异步与展示边界。`pnpm test:backend` 验证核心存储与服务测试，`pnpm prepare:service` 后运行 `pnpm test:service` 验证独立进程和持久化。
- 插件测试先安装 `wasm32-unknown-unknown` 目标并执行 `pnpm prepare:plugins`，策略故障夹具另用 `pnpm prepare:policy-fixtures`。服务专项可用 `LOCAL_AGENT_TEST_SERVICE_BINARY` 指定独立 debug 副本，插件专项使用 `PLUGIN_TEST_BINARY`；构建与依赖该二进制的测试顺序执行，生产 release 服务拒绝内存凭据测试开关。
- `pnpm build` 验证前端；`pnpm tauri build --debug --bundles app` 验证 macOS 开发包。实际窗口、键盘、滚动与弹层仍须运行验证。
- 原生验收结束后必须正常退出本次测试实例，使用 `pnpm clean:test-apps --apply --min-age-minutes 0 --app <本次打包路径> --app <本次验收副本路径>` 回收本批 App 和对应系统登记，并将清理报告链接写入验收记录。只传本次拥有的确切路径；运行中 App 自动保护。保留用户数据、来源、截图、日志、文档及摘要，不能递归删除验收目录。长期保留的包须说明理由。完整流程见 `docs/test-app-cleanup.md`。

## 协作

保留用户现有文档和无关文件。子任务按用户提供的工具映射在主线程顺序执行，不主动分派子 Agent。
