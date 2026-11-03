# 对象详情贡献

本文件约定 `host.object-detail-slots@1` 及独立对象查询权限。通用详情插槽使用 `host.detail-slots@1`，两者按目标和权限分别核查。

## 声明及范围

示例目录为 `examples/object-summary/package`。首批使用 manifestVersion 1 的声明式包，能力仅 `ui.contribution@1`，权限仅 `ui.contribute` 与 `domain.query: object.summary@1`，必须显式要求 `host.object-detail-slots@1`。不运行 guest 或第三方 JavaScript。来源、协议、网络和配置写入由其他独立包及宿主授权负责，不能混入此贡献包。

`object-contributions.json` 是签名载荷的一部分，声明 `schemaVersion: 1` 和 `detailSlots`。每个插槽包含唯一 `id`、`target`、`placement`、`viewId`、`title`、`queryKind`、`fields` 和 `purpose`。首次仅支持 `queryKind: summary`，模板由宿主渲染。卡片与页签可共享一个报告 view；示例 7 个 view、13 个插槽沿用既有 8 个 view 上限。

| 目标            | 可申请的字段                                            | 含义                                                               |
| --------------- | ------------------------------------------------------- | ------------------------------------------------------------------ |
| agent / project | session-count                                           | 当前可读索引内已记录会话数；范围不完整时保留部分状态               |
| provider        | model-count、configuration                              | 已保存且启用的不同模型 ID 数和配置标记；不是实时连通或模型调用验证 |
| account         | quota-window-count                                      | 既有额度快照中记录的窗口数；不是剩余额度或可用券数量               |
| session         | input-tokens、output-tokens、total-tokens               | 来源已记录的会话用量；不补算未记录的总量                           |
| request         | 三类 Token、attempt-count、duration-ms、request-outcome | 固定请求单位内的已记录摘要；来源报告不变成宿主计时或尝试证据       |

来源缺失、未读取、未知、过期、无权限和确认零分开表示。计数和 Token 使用规范十进制字符串，不能经 JavaScript number 转换；毫秒允许保留已记录的小数。没有正文、标题、端点、文件路径、凭据、账户身份或任意字段查询权限。首批只读适配不表示全部第三方来源均支持全部目标，实际适配范围以宿主和已授权来源的能力为准。

## 宿主授权与引用

作者声明字段与用途；宿主在用户选择的真实对象详情内核对来源权限，展示对象类型、字段、用途及具体安装包版本后申请独立摘要授权。授权绑定完整插槽、签名包摘要、声明及父插件授权代次，不沿用原 `gateway.summary` 报表授权。

对象引用由宿主产生，固定当前对象、冻结范围、字段、插件版本和授权。读取仅接受原引用，不能替换对象、范围、单位或字段。引用不进入 URL；相同插槽切换对象会退休旧引用。引用最多 60 秒、宿主最多保留 128 个最新插槽上下文；关闭视图精确释放自身引用，不清除较新的引用。授权、后台、版本或快照变化后，旧响应被丢弃。

统一项目、会话和请求必须沿用原冻结查询，包括来源选择、筛选与请求单位，不能用默认查询扩大范围，也不能将当前页视为全部记录。失效时明确提示重新读取，不自动刷新额度、采集来源、执行模型或更改 Agent 配置。

`perform_object_contribution` 是宿主 UI 与原生边界内的有限动作接口，不是新增的 guest 任意对象查询 ABI。第三方贡献作者不能据此获取 AppState、原始资源或管理令牌。

## 可选依赖和旧宿主

原可选依赖继续保存在独立签名文件 `contributions.json`，使用 schemaVersion 1。示例仅把增强会话页签关联到既有 `org.example.detail-helper`；缺失、停用或版本不匹配时保留基础卡片和页签。依赖恢复不共享授权，不自动刷新数据。

旧宿主拒绝必需特性 `host.object-detail-slots@1`，不得仅忽略声明后把新包当作已支持。无此必需特性的旧包继续按原规则处理；旧包夹带的未知文件不获得对象摘要权限。

## 当前作者验证入口

当前 SDK 可导出 `--plugin-sdk schemas-object-summary` 与 `--plugin-sdk schemas-object-contributions`。宿主生成契约分别位于 `contracts/domain/object-summary/` 和 `contracts/plugins/objects/`。代码中的 schema、类型和签名声明都必须校验；不能只更新契约夹具来绕过兼容检查。

## 打包与校验

先确认使用支持 `host.object-detail-slots@1` 的服务作者工具；使用前核对服务实际声明的能力。可在受管目录使用既有命令：

```sh
local-agent-service --plugin-sdk keygen ./author.key
local-agent-service --plugin-sdk pack ./sdk/plugins/examples/object-summary/package ./objects.laplug ./author.key
local-agent-service --plugin-sdk check ./objects.laplug
```

私钥由作者保管，不放入插件包。安装、信任发布者、启用插件和具体插槽授权是独立步骤。示例不要求开发模式或 guest 执行权限。旧宿主的必需特性拒绝、新宿主的声明校验、安装后读取与原生页面应分别验证。

增强页签的引用同时绑定相关可选依赖的签名版本和启停代次。依赖停用再启用后，应创建新引用；已退休引用不会恢复。基础插槽的上下文不受无关依赖生命周期影响。
