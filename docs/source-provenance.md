# 源码与资产来源核对

核对日期：2026-11-07。工作区基线为 `9f2c9f9`；没有提交、推送或发布新版本。范围为 Go/MyGo 主项目、此次新增业务代码、App 内的资源和依赖声明。未跟踪的 `ServiceManager/` Swift 原型不进入构建，也未修改。

| 材料 | 当前来源证据 | 分发处理 |
| --- | --- | --- |
| 原有 Go 主项目 | Git 历史作者为 J.yeu / Jyeu，远程为 tdawn0-0/macOS-service-manager；保留原历史 | 项目自有源码采用顶层 Apache-2.0 LICENSE/NOTICE |
| 新增业务层、CLI、图形表单和测试 | 当前工作区 internal/devplatform、internal/devcli、cmd、view_*.go 与对应测试；通过 Go 模块引用外部库，无复制的 vendor 目录 | 项目源码纳入上述许可，外部模块保留其许可 |
| Lucide 内嵌 SVG | icons.go 已有来源注释，最早引入于 20f1b66；不是此次创造的图标 | 保留完整上游 ISC 和 Feather 衍生图标 MIT 声明，见 assets/notices/lucide |
| App / 菜单栏 / 引导图标 | assets/icons 下 SVG 及 scripts/icons.sh 的生成映射；f0a2a50 同时登记 SVG、生成脚本与 PNG，现有文件保持 | 随项目保留，不把 Lucide 声明误作这些自有 SVG 的来源证明 |
| 新增开发工作区截图 | development_visual_test.go 的固定 designFixture；本轮重新渲染的全部 252 张 PNG 与 docs/images/development 逐字节一致 | 两种尺寸、两种主题、63 个视图；manifest.json 记录渲染输入和图片 SHA-256，不包含本机项目内容 |
| Go 模块 | go.mod/go.sum、go list 的 darwin/arm64 依赖图和本机模块缓存 | 15 个模块的原始声明文件及各 SHA-256 纳入 licenses/manifest.json |
| Go 工具链 | go env GOVERSION / GOROOT；当前 Homebrew 分发的 LICENSE 位于同一公式前缀，PATENTS 在 GOROOT | 两者原样纳入；缺少 LICENSE 必须报错，不能仅凭 PATENTS 判定完整 |
| 下载的运行时和数据库 | 软件目录中的来源、固定摘要及原始归档 | 不内嵌于 DMG，不改变其许可；见 third-party-software.md |

Lucide 许可证快照固定到 `e715245d62667c800e7f54c94b1b023692e900a3`，SHA-256 为 `b495047bd93a9b06913511076f504daba17d5bbeb3e0650f3bb53a4220329c57`。完整上游文件包含 ISC 及 Feather 派生图标的 MIT 段落。旧 SVG 几何的具体上游版本未写入原 Git 历史，因此不把许可证快照提交号当作图标几何的版本号。

收集器离线运行，只读取已下载的模块和已核对的资产许可证。资产源文件在 assets/notices 内固定上游提交、URL、摘要和使用位置；生成到 licenses/third-party-assets。声明缺失、摘要变化或使用位置无效都会阻止检查通过。CI、发布工作流和本地打包均执行生成器测试及清单核对。

当前生成目录包含 50 个文件，连同顶层 LICENSE / NOTICE，共有 52 个分发声明文件。旧签名包仅有 50 个，保留其原字节作为历史产物；新增构建补齐了遗漏的 Go LICENSE 和 Lucide 声明，不追溯声称旧包已经包含它们。
