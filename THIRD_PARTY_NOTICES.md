# 第三方说明

## Agent 客户端图标

`public/agent-logos/` 中的 PNG 从对应客户端的应用图标提取，保留原图，用于识别 Agent 家族；客户端名称仍分别显示。

| 资源              | 原始资源                                                                        |
| ----------------- | ------------------------------------------------------------------------------- |
| `claude.png`      | Claude.app / Contents/Resources/electron.icns                                   |
| `codex.png`       | bundle ID 为 com.openai.codex 的 ChatGPT.app / Contents/Resources/electron.icns |
| `opencode.png`    | OpenCode.app / Contents/Resources/icon.icns                                     |
| `antigravity.png` | Antigravity.app / Contents/Resources/icon.icns                                  |

## Zeron UI

来源：[Carlosfengv/zeron-ui](https://github.com/Carlosfengv/zeron-ui)。通过官方 `zeron-ui@0.2.0-beta.19` CLI，从 `https://zeron-ui.vercel.app/r` 安装组件与语义主题。

实际文件与来源摘要保留在 `.zeron/install-state.json`，依赖版本保留在 `pnpm-lock.yaml`。本项目保留 Zeron 受管理源码，业务代码通过公开接口消费。

Zeron UI 使用 MIT License，Copyright (c) Zeron Design。完整许可证保留在 `licenses/zeron-ui-MIT.txt`。

Tauri、React、Tailwind CSS、SQLite 及其他依赖各自的许可随其软件包保留；发布前应根据最终锁文件生成完整依赖许可清单。

## TanStack Table

DataTable 使用 `@tanstack/react-table@8.21.3` 及同版 table-core，来源 [TanStack/table](https://github.com/TanStack/table)。MIT License，Copyright (c) 2016 Tanner Linsley。包内许可证原文保留在 `licenses/tanstack-table-MIT.txt`。

## Playwright（仅开发测试）

浏览器回归使用 `@playwright/test@1.64.1`，来源 [Microsoft/playwright](https://github.com/microsoft/playwright)，Apache-2.0。包内许可证原文保留在 `licenses/playwright-Apache-2.0.txt`。此测试依赖不进入桌面运行包；浏览器可执行文件另由测试安装步骤取得。
