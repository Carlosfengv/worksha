# 宿主草稿目录读取

此接口仅供受控桌面命令使用，Wasm guest 的方法、权限和输入保持原样。既有 Provider、目录、来源、操作、取消与恢复契约不变；新增 `perform_provider_draft_catalog({ input })`，服务路径 `POST /api/provider-draft-catalog`，请求上限 32 KiB。

契约位于 `contracts/domain/provider-draft-catalog/`。通过随包服务的 `--plugin-sdk schemas-provider-draft-catalog <目录>` 导出四份 schema；`scripts/plugin-bindings.mjs --provider-draft-catalog` 核对生成类型，已纳入 `pnpm check`。

`input` 为 `{ kind: "read" | "check", request }`。`request` 包含：

- `origin`：已保存对象使用 `saved`、原 `SourceQuery` 和捕获的 `sourceRevision`；新建对象使用 `new`、`storeId` 与 `providerId`。新建身份只接受尚未配置、修订为 0 的默认对象或不存在的对象，并核查原来源文件和缺失标记，不能接管已保存连接。
- `draft`：当前编辑的 `endpoint`、`protocol`、`vendor` 和 `credentialRef`，不包含明文 Key。原连接仅用作归属及来源变化校验；网络、凭据与观测范围均取自此草稿。
- `source`：沿用宿主 `SourceChoice`。内置是严格的 `{ kind: "builtin" }`；外部需明确的插件、digest、bindingId 和精确 `Draft` target。Saved 授权不代替 Draft 授权，Draft 授权也不会自动延续至保存结果。

`check` 成功返回 JSON `null`；只核查数据库、原来源代次、对象修订及当前草稿授权，不访问 Vault、请求目录、启动 worker、授权或发送模型请求。读取成功返回原模型目录响应形状，保持 unknown / false、部分目录、时间与观测语义。

宿主在准入、读取凭据之后、分页请求与正文消费及结果交付时核查范围，并保留在途失效检查。已准入的请求可能仍在网络中；变化后停止交付及后续分页，不自动重发或切换来源。内置保持原 15 秒完整读取时限和公开错误映射；原外部目录的资源、网络与 worker 约束保持。

编辑器在自己的授权返回后才捕获本地权限代次，避免授权操作使本次读取自行失效。发布前再次 `check`；缓存只在相同对象、实际草稿范围、来源代次、权限代次和后台身份下有效。有数据时顺序复核元数据，每份显示许可最多 5 秒，采用单调时钟；迟到回复不能延续已到期的许可。隐藏窗口、本地权限变化、实际地址 / 凭据范围变化、更换后台或来源时撤下读取结果，已登记模型保留。

仅输入尚未提交的新 Key 时，原凭据引用尚未改变：隐藏「当前目录」确认信息，继续核查原范围；清空输入后，原范围仍有效才恢复显示。保存新 Key 保留当前仍有授权的候选模型 ID，但不将原目录标为新凭据的有效目录。模型 ID 列表和目录摘要均在异步准备之后重新读取有效缓存，不使用已经失效的渲染闭包。

本接口不改变原操作序号、SQL 回执或写入行为，不解决跨关闭 / 跨视图恢复后的草稿授权所有权收尾。后者仍须独立完成；不能根据绑定外观推测所有权并撤销其他操作的授权。
