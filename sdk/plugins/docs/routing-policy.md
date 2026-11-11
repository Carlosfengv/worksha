# 纯路由排序契约

`contracts/domain/routing-policy/` 定义有限候选排序。内置主备排序与外部纯策略通过相同领域接口执行；外部策略使用统一插件格式，独立持有启用、模型版本绑定和发布意图。

## 宿主与排序器

宿主先固定已发布服务版本，排除禁用或不满足既有能力约束的模型，再构造 `RankRequest`。最多 32 项，沿用原模型服务容量；服务、版本和候选均使用现有 UUID 身份，保留原字符串表示，不以模型名或 Endpoint 替代身份。

请求只包含服务及版本身份、允许的首选 / 默认模型、已过滤的配置顺序，以及候选 ID、已发布能力摘要和健康状态。不含 Provider 对象、Endpoint、Key 或凭据引用、价格、请求正文、工具参数、Session、SQLite、超时 / 重试 / 并发预算。

内置排序器 `BuiltinPriority` 实现 `RoutingPolicy`：首选模型在前；之后按原配置顺序，未配置顺序时采用默认和发布顺序；精确去重。禁用、不兼容和不存在的项不会通过排序重新出现。实际 `Store::service_dispatch` 消费排序结果后才生成原路由目标和原 Dispatch，不只注册一个未使用接口。

原序列化 `model_services::policy::RoutingPolicy` 是已有服务配置，名称虽相同，职责不同。它的字段、默认值、JSON、版本快照及主库格式保持；新增领域 trait 没有替换该持久类型。

## 输入与结果核查

`RankRequest.identity()` 固定全部脱敏输入。`RankResult` 只含该摘要、允许候选的排列或子集，以及固定原因码；不能增加候选、重复候选、跨服务版本或以旧健康快照回复。多余字段拒绝。该结果不拥有发送权限，也不能修改 maxAttempts、回退开关、Endpoint、Key、模型能力或价格。

内置路径的健康状态为 `unknown`，因为原排序阶段不读取冷却和运行容量；没有将未观察状态写成 available。宿主仍在每次发送前按原流程核查真实冷却、并发、每分钟限制、期限、同轮归属和发送状态。关闭策略或已固定轮次继续保持原路径，不增加网络或元数据读取。

已有 SQL pending / committed / failed、重启后的未知请求保护、发送归属、尝试计数、有限回退、流输出后不重发、用量与错误裁决均留在原宿主流程。排序不执行文件、数据库、网络或插件生命周期操作。

## 外部策略的执行边界

外部排序使用 `kind: routing-policy`，必须明确启用并绑定具体模型服务版本。策略与来源能力不能混用，也不会因安装来源插件自动获得排序权限。

外部 worker 调用必须在 Store 锁与 SQL 事务之外进行，结果仍由宿主复核。绑定固定服务版本、签名包、授权修订和用户选择的失败处理；首次发送前也只有用户明确选择的内置降级才能使用。发送、输出或结果不明后不迁移请求或自动重发。

服务发布与新绑定仓储采用可恢复的 pending 意图及原操作编号；核查原发布结果后完成收尾，仅尚未开始提交的意图可取消。不能宣称两个 SQLite 库原子提交，不能为了接入排序而重做未知发布或改变原服务 JSON。

SDK 导出：`local-agent-service --plugin-sdk schemas-routing-policy <directory>`。完整链路见 [Provider 接入](provider-agent-chain.md)；DSH、真实 Keychain 专项、Developer ID 与正式公证保持未验证。

## 纯策略包

`contracts/plugins/routing/` 定义策略能力与管理操作。清单固定 `kind: routing-policy`、`hostApi: ^1.0.0`、`domain.routing-policy@1` 与 `host.pure-policy@1`，`permissions` 必须为空，不填写 `source`。使用统一清单、签名域、打包检查器与 worker 入口。

示例在 `sdk/plugins/examples/pure-policy`。通过 `--plugin-sdk componentize` 将 core Wasm 转为组件，使用统一的 `pack` 打包、`check` 检查。`schemas-policy` 导出策略能力的 Schema，`node scripts/plugin-bindings.mjs --policy` 核对 TypeScript。

worker 接收 policy.rank 的 RankInvocation：request 加宿主计算的 requestIdentity。结果必须回显摘要，只能返回许可 ID 的排列或子集，reason 固定 extension-order。示例反转候选顺序。空子集表示本轮无候选，不会追加被省略候选。Available 只表示现有冷却记录没有活动限制，不代表一次新的连接验证。

worker 没有 WASI、环境、目录、网络、凭据或 broker 授权。沿用 128 MiB 内存、燃料和 RSS 限制，每次生命周期由宿主限制三秒；初始化、预检、激活、健康、排序、停用和释放均在独立进程中执行。metadata list 不运行 guest。默认未绑定服务的发送路径不读取策略库、不启动 worker。

## 版本、授权与失败

策略数据保存在新增 routing-policy.sqlite3；签名包与暂存文件共享 plugin-sources 的全局物理容量门禁。旧插件与策略共同受 64 包、256 MiB、同 ID 三版本安装限制约束。纯策略与旧来源不能共用同一 ID。

绑定固定 storeId、serviceId、serviceVersionId、包摘要、宿主实例、激活 epoch 和发布者授权版本。绑定修订使用不透明 UUID 做 CAS，保留历史版本行；新服务版本、插件升级或回退不会隐式改写原绑定。重启不会恢复运行授权，需要明确启用并重新确认绑定。先启用服务既有“优先级与回退”，再选择策略；该开关关闭而外部绑定仍保留时，新的发送被拒绝，必须明确解除绑定。

用户选择 reject 或 builtin。Builtin 降级只发生在首次发送前；已有版本行沿用该版本的失败选择，不能从较新版本借用降级权限。第一次发送后不再运行排序器。每次发送继续由原服务进行候选能力、冷却、限流、预算与持久发送归属核查；授权变化阻止后续尝试，不重新排序或扩大预算。已开始输出的流不迁移目标。

## 发布与恢复

发布意图先写独立仓储，再以原 OperationRequest.operationId 调用既有模型服务发布接口。成功后从原主库回执固定服务版本并提交策略绑定；跨两个 SQLite 不宣称原子事务。所需标记先持久化，标记与仓储不一致时阻止该范围的新发送。

query-publish 只读取原回执并完成绑定，不重放模型服务发布。operation_not_found 不等于未执行。结果不明时保留 pending；相同 publish 再次到达也只返回原意图。只有尚未开始提交的 prepared 意图可取消。原始请求保存在意图中，供人工核查原操作编号；未确认状态不得通过新编号自动重发。发布结果确认前，该服务新的策略请求与其他发布被阻止，既有在途请求保留旧版本执行归属。

`resume-publish` 是用户明确确认后的继续操作，只接收 `storeId` 与原 `operationId`，不接收替换请求。宿主先核查原回执，再使用意图中固定的原编号和原请求进入既有幂等发布器；已有回执直接返回，已取消操作不会继续。并发继续操作仍由原主库事务核对同一编号。继续操作沿用准备时的授权快照，不重新授权插件；查询和相同 publish 的重放始终保持只读核查语义。原生界面显示原服务及编号，确认后才继续提交，错误保留在确认窗口内。

pending 一直保留到绑定与完成记录均已持久化，最后先清理仓储中的 pending，再同步所需标记。中断时旧标记继续阻止新请求，核查只修复该原编号的收尾，不能覆盖之后的绑定。prepared 取消在策略库中同时提交取消记录和范围状态，再同步标记；只有主库未查到对应原请求回执时才允许取消。

发布意图保存准备时的宿主实例、激活代次与发布者授权修订。重启后的核查沿用这份原始授权快照，不能用当前授权重新绑定；返回结果按当前授权重新显示状态。明确启用插件之后，仍需重新确认服务版本绑定。待核查时，原模型服务操作入口仅允许相同原编号和完全相同请求内容，不能借用原编号发布其他内容。

启动会有界、只读地对照策略仓储中的范围记录。必需标记丢失或不一致时显示该服务状态未知，不把仍存在的绑定当作未绑定；可识别的损坏只影响对应服务。SDK 可使用已知范围修订明确解除非 pending 绑定；未知提交、损坏的不可识别记录、双重元数据丢失与满额恢复仍需保留证据并核查，不能自动丢弃。

## 界面与验证

模型服务页提供本机完整选包、公钥与摘要确认、安装、启停、撤权、回退、卸载、当前版本绑定、发布新版本并绑定、明确失败选择与原结果查询。新增 perform_routing_policy(input)，原 124 个 IPC 名称与参数保留。

离线验证使用受管临时目录、MemoryVault 和环回模拟上游。macOS 也可用签名开发包验证钥匙串重启链路。将 `LOCAL_AGENT_TEST_SERVICE_BINARY` 指向包内服务，在 Claude Code CLI 空闲时运行 `pnpm test:external-routing-policy --system-keychain`。这组测试通过隔离配置生成凭据助手，检查仅启用插件时仍拒绝发送、显式重新绑定后原入口恢复发送，以及本次测试凭据的回收。

原生选包、键盘、滚动、最小窗口与拖拽须在打包窗口核验。DSH、Developer ID 和正式公证仍待验证。

## 仓储与界面边界补充

新增字节计数使用十进制字符串，旧 DTO 保持。复核预览最多 32 份，十分钟过期；每次检查包最多回收八份已完成或过期预览，仅删除摘要仍匹配的自有暂存文件。已安装包和改动过的文件不会被自动清理。服务范围注册最多 256 项，满额在写入新范围前拒绝，不把正常运行中的注册数量推到重启预算以上。

expectedBindingRevision 使用目录 scopes 中对应范围的 revision；历史版本绑定的 revision 用于在途身份，取消未提交发布后两者可以不同。unavailableServiceIds 表示无法解码但能识别服务身份的范围标记，界面必须显示未知并暂停该范围绑定操作，不能显示“未绑定”或“已授权”。

原始请求可从策略意图查询，SDK 调用者可按原操作编号核查回执。满额发布意图、范围注册和异常孤立标记的长期恢复尚未完整支持。宿主保留未知记录，等待明确核查。

排序输入与内置契约保持同一语义：candidates 按服务发布顺序，configuredOrder 保留过滤后的用户配置顺序，首选和默认单独传递。不能把内置策略排序后的列表重新命名为上述字段。失败轮次重试继续使用已捕获的服务版本及其绑定，不从较新的选择借用权限或失败处理。

排序与授权复核消耗原服务的整轮等待预算。三秒 guest 上限与剩余服务预算同时生效，排序耗时不会给后续模型发送追加时间。外部策略发送前拒绝的网关响应含固定原因码；这不修改原生无外部绑定时的错误响应。

## 宿主范围恢复契约

范围恢复使用独立的 `contracts/domain/routing-policy-recovery/`，不扩充旧纯策略 Action / Reply，也不开放新的 guest 方法或 WIT 权限。宿主 SDK 工具 `--plugin-sdk schemas-routing-policy-recovery <目录>` 导出四份 schema；TypeScript 由 `scripts/plugin-bindings.mjs --policy-recovery` 校核。

受控宿主入口为 `perform_routing_policy_recovery(input)` / `POST /api/routing-policy-recovery`。操作仅有 `list {storeId}`、`inspect {scope}`、`resolve {reference, mode}`；scope 固定为 storeId / serviceId，reference 由宿主核查签发，mode 仅为 `{kind:"native"}` 或 `{kind:"pending-original"}`。禁止未知字段、调用方路径、SQL、force、原请求替换及权限选择。

引用最多 32 个，每个范围只保留最新核查，60 秒有效。确认前重新比对数据库文件身份、当前服务、原控制与标记、绑定、原发布和核心回执；每批元数据读取上限 8 MiB，序列化校核上限 2 MiB，超限明确拒绝。服务标识沿用已有的原始 UUID 拼写，宿主新签发的引用与修订仍使用规范 UUID。修复收据只备份原控制和标记及证据摘要，最多 256 条；原绑定、发布和主库历史不复制到收据、不自动删除。已提交收据可按原 reference 完成相同标记同步，但证据或后续选择改变即拒绝。

`scope_recovery_unknown` 带 `outcomeUnknown:true`。调用方应保留原 reference，显示明确核查动作；不得自动重试、续权、提交发布或补发模型请求。完整操作流程及不能恢复的证据类型见 [范围核查与恢复](routing-policy-recovery.md)。旧 IPC 和旧包契约继续保持。
