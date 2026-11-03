# 可选依赖与通用详情插槽

此功能使用签名文件 `contributions.json` 和两个显式宿主特性，遵循统一插件清单与 API。

在 manifest 的 `requires` 中按需声明：

```json
["host.optional-dependencies@1", "host.detail-slots@1"]
```

未知特性必须明确拒绝；未声明的载荷不注册为贡献。基础插件及协议、配置和连接器可按各自清单限制声明，完整历史与纯路由策略不开放此声明。

## 声明

```json
{
  "schemaVersion": 1,
  "optionalDependencies": [
    {
      "pluginId": "org.example.detail-helper",
      "version": "^1.0",
      "views": ["enhanced"]
    }
  ],
  "detailSlots": [
    {
      "id": "agent-extra",
      "target": "agent",
      "objectIds": ["claude-code-cli"],
      "placement": "tab",
      "viewId": "enhanced",
      "title": "附加信息"
    }
  ]
}
```

文件最大 16 KiB，最多 16 个可选依赖、16 个插槽。未知字段拒绝；所有视图须已在 manifest 中声明。文件与其他载荷一起进入 `files.json` 摘要和 Ed25519 签名，不能安装后另放一份未签名声明。

## 可选依赖

依赖按插件 ID 和 SemVer 范围匹配**当前启用版本**，不按最高已安装版本匹配，不自动下载或启用依赖。`views` 列出依赖影响的既有视图，不得为空；可以影响多个视图，也可以由多个依赖共同限制一个视图。

缺失、停用、初始化 / 运行失败、签名损坏或版本不匹配时，仅指定视图不可用，消费者基础能力、启用状态、来源和报表授权保持。已有扩展页的直接查询也受相同检查，不能绕过详情 UI。依赖恢复后视图可重新读取自己的已有授权快照；不继承依赖的授权，不自动刷新、不执行模型或工具。

必需 `plugin:<id>` 规则保持：缺失阻止激活，停用反向停用依赖者。同一依赖不能同时声明为必需与可选；拒绝自身、重复项和无效版本范围。已存在的可选边参与循环检查，激活 / 回退的图发布互斥；竞争操作可返回既有 `resource_limit`，已经启用版本的幂等调用不受其他图发布影响。

这些声明控制视图增强的可用性，不是跨插件调用接口。guest 不获得依赖的实例、文件、原始凭据或宿主权限。插件的其他能力不随可选视图隐式停用。

## 详情插槽

`target` 为 `agent`、`provider` 或 `account`；`placement` 为 `card` 或 `tab`，三类详情均使用同一宿主组件。Agent / Provider 插槽位于既有详情主体的“扩展信息”；账户插槽跟随已确认选择的账户，不在未知账户时推断归属。

`objectIds` 为公开对象 ID 的精确列表，最多 32 个；单独的 `["*"]` 表示适用于该类对象的插件公共信息，不能与具体 ID 混用。不支持路径、表达式、脚本选择器或读取对象字段。目标 ID 仅在前端匹配声明，不传给 guest；插件数据仍来自它自己已获授权的视图，不能将通用插件信息当成当前对象的原生账单。

视图复用宿主的 report / sessions / quota / providers / settings 模板。插槽是只读展示：可读取已有快照和翻页，不自动采集，不执行正文读取、Provider 创建、配置计划、MCP 启动或其他动作。授权与刷新仍在独立扩展页明确操作。插槽不能提交 HTML、CSS、事件脚本、远程模块或原始本机命令。

附加页签通过 `pluginSlot=<target>:<pluginId>:<slotId>` 保存深链；设置该参数保留原路径及 `tab` 等查询参数。例如：

```text
#/agents/claude-code-cli?tab=quota&pluginSlot=agent:org.example.detail-report:agent-enhanced
```

插件停用 / 移除、对象不匹配或旧插槽不存在时显示不可用及“返回当前详情”。版本 / 授权快照不一致时不显示新旧混合结果；对象切换后丢弃迟到请求。单视图错误和声明错误不破坏原详情。

## 状态与工具

现有 `list_plugins` 的返回值增加可选的 `contributions` 目录，以及 `hostFeatures`；旧字段和参数保持。目录按 `pluginId + digest` 区分版本，包含依赖状态、相关视图、插槽和单包错误。它不代表模型调用成功；依赖可用只表示对应版本已启用并可校验。

新 schema 的权威源是宿主 `contracts/plugins/contributions/`。SDK 分发时位于 `contracts/contributions/`。导出与检查：

```sh
"$SERVICE" --plugin-sdk schemas-contributions /absolute/contracts
"$SERVICE" --plugin-sdk pack examples/detail-report/package /absolute/report.laplug /absolute/publisher.key
"$SERVICE" --plugin-sdk pack examples/detail-helper/package /absolute/helper.laplug /absolute/publisher.key
"$SERVICE" --plugin-sdk check /absolute/report.laplug
```

两个样例均为声明式，不依赖作者另编译 Wasm。report 样例覆盖三类详情的卡片与页签；没有 helper 时基础摘要仍可用，增强摘要降级。安装后仍须单独启用并授权报表。没有模拟调用或修改原生来源。

## 回退与保留边界

同数据降回 0.1.2 时，新特性包保持安装记录但明确不可用，原有功能与旧包可继续使用。回到 0.1.3 后，需要明确重新启用这些失败版本并按原规则重新授权；不会悄悄恢复旧授权。核心 schema 2 和两个插件元数据 schema 1 均不迁移。

DSH、Developer ID / 公证继续未验证。复杂 UI、跨源统一身份 / 去重、外部 SQLite、通用增量、清理 / 兑换和完整留存清理不因本次插槽而开放。
