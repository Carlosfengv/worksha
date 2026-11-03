# 原生测试 App 的回收

每批原生验收结束后回收本批完整 App，包括 Tauri 打包目录中的产物和 `.work` / `output` 中的测试副本。验收文档、截图、日志、配置、来源夹具与用户数据继续保留。清理工具记录包身份、版本和主程序 / service 的 SHA-256，使删除后的验收记录仍可追溯。

## 每批验收的结束步骤

1. 开始前记录用户正在运行的 App / service 路径和 PID；测试继续使用独立身份与临时来源 / 数据目录。顺序验收优先复用固定测试名称和标识；同时进行的验收使用各自身份。不能为减少图标而共用正式实例身份或取消数据隔离。
2. 验收结束后先保存结果、截图、签名检查、主程序 / service 摘要，以及未验证事项。正常退出本批测试 App，等待本批 sidecar 结束。不要退出用户实例。
3. 对本批拥有的两个确切路径运行预览，核对报告中的 `remove` 与 `preserve`，再执行回收。`--app` 可重复；没有创建副本时只传打包路径。

```sh
pnpm clean:test-apps --min-age-minutes 0 \
  --app 'target/debug/bundle/macos/Local Agent Validation.app' \
  --app '.work/my-validation/Local Agent Validation.app'

pnpm clean:test-apps --apply --min-age-minutes 0 \
  --app 'target/debug/bundle/macos/Local Agent Validation.app' \
  --app '.work/my-validation/Local Agent Validation.app'
```

4. 在验收记录中链接命令返回的 JSON 报告。核对 `errors`、`remainingTargetRegistrations` 为空，`removed` 包含本批路径。若 `skipped` 或 `preserve` 中的本批路径仍在运行，结束自己的测试进程后重新执行；不能把跳过写成清理完成。
5. 确认原用户 App / service 继续运行。临时数据按本次授权范围单独回收，本工具不会删除任何 App 外的数据。需要保留完整 App 时，在验收记录中写明路径、原因和保留期限；共享批量回收使用 `--keep <确切路径>`。

失败、中断或只完成构建的验收也须回收本批产物并保存清理结果。工具不强制终止进程；本批进程尚未结束时如实记录保护状态，结束后补做回收。

## 历史积累的批量处理

```sh
pnpm clean:test-apps
pnpm clean:test-apps --apply
```

默认只预览；`--apply` 才会删除。默认保护最近 60 分钟内创建或变更的包，避免碰到其他正在进行的构建 / 验收。本批已结束的精确路径才使用 `--min-age-minutes 0`，不要在并行验收时用该选项做全量清理。

扫描范围限定为本仓库的 `target/debug/bundle/macos`、`target/release/bundle/macos`、`.work` 和 `output`。仅识别 `app.localagent.*` 且具有有效 Info.plist、桌面可执行文件及包内 service 的 `.app`；不跟随符号链接，不删除父目录或 App 外文件。系统进程列表查询失败时停止删除。每个包删除前再次检查进程与文件指纹，保护新启动或被构建更新的包，包括从包内直接运行的 sidecar。

先删除已确认的测试包，再按确切路径逐项注销系统登记；批量模式也注销已不存在的 Local Agent 包路径。最终复核若发现异步索引再次登记了已删除的目标路径，仅对这些路径补注销一次，仍有残留则返回失败并如实记录。现存的仓库外 App 不处理。不会重置 Launch Services 数据库、重建全盘 Spotlight 索引或重启 Dock；当前运行包的系统登记会复核并在需要时恢复。搜索界面若暂时保留旧结果，等待系统索引更新后重新打开搜索。

Spotlight 搜索缓存与应用登记分别核对。若文件和应用登记已清除，但 `mdfind` 仍返回缺失路径，单独记录缓存残影；不能写成搜索结果已全部清除。索引只读或系统目录选择器不可用时保留该限制，可参考 [Apple 的局部索引重建步骤](https://support.apple.com/en-us/102321)，不要为消除一个残影而自动重建全盘索引或改变用户既有搜索设置。

报告默认写到 `.work/test-app-cleanup/<UTC时间>.json`，记录候选、保护原因、摘要、已删除、已注销、跳过、错误及完成后的剩余包 / 目标登记。可用 `--report <新文件路径>` 指定位置；拒绝覆盖已有报告或写到 App 内。历史报告引用的 App 路径可能已被回收，应结合清理报告读取，不能据此认为历史验收未发生。

清理逻辑回归使用受管临时目录和伪造的系统响应：`pnpm test:cleanup-test-apps`，不会注销真实应用或接触用户数据。
