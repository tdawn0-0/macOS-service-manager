# 第三方软件与包来源

App 按需下载发行包，不把 Homebrew 当作安装前提。下表来自当前 `internal/devplatform/catalog.go` 的固定目录，实际授权文件以所下载归档内的 LICENSE/NOTICE 为准。此表记录来源，不改变组件许可。

| 软件 | 当前目录版本 | 发行来源 | 目录记录的许可 |
| --- | --- | --- | --- |
| PHP | 8.5.12 | [Lerd PHP 发行](https://github.com/lerd-env/php/releases/tag/php-8.5.12) | PHP-3.01 及包内组件许可 |
| Node.js | 22.14.0、20.19.0 等精确版本 | [Node.js 官方归档](https://nodejs.org/dist/) | MIT 及包内组件许可 |
| MySQL | 8.4.4 | [MySQL 官方归档](https://cdn.mysql.com/archives/mysql-8.4/) | GPL-2.0 及包内组件许可 |
| PostgreSQL | Postgres.app 17-2.9.6 | [Postgres.app 发行](https://github.com/PostgresApp/PostgresApp/releases/tag/v2.9.6) | PostgreSQL 及包内组件许可 |
| Redis | Redis 7.4.7 / Stack 7.4.0-v8 发行包 | [Redis 官方包目录](https://packages.redis.io/redis-stack/) | RSALv2/SSPLv1 及包内组件许可 |
| MongoDB | 8.0.5 | [MongoDB 官方下载源](https://fastdl.mongodb.org/osx/) | SSPL-1.0 |

Redis 实例直接使用发行包中的 redis-server，不用 Valkey 替代，也不自动启用 Stack 模块。Go 与 Valkey 的旧适配保留兼容。

应用本身直接依赖 MyGo、purego、MongoDB Go Driver 和 howett.net/plist；精确版本及传递依赖见 go.mod/go.sum。项目自有源码已按计划采用顶层 Apache-2.0 LICENSE/NOTICE。标准文本来自 [Apache 官方](https://www.apache.org/licenses/LICENSE-2.0.txt)，原作者在 Git 历史中保留为 J.yeu/Jyeu。外部组件保持原许可；新增源码或资产的来源仍须在发布前审阅。

每个已安装包保存来源描述和摘要。新增版本须更新目录、校验归档摘要、检查动态库与迁移路径，并在目标 Mac 上运行。产品仅支持 Apple Silicon（arm64），不提供 Intel 专用包。上游通用归档须包含 arm64；保留其原始校验和及归属。


App 随包包含 LICENSE、NOTICE 和 licenses 目录。`scripts/collect-third-party-notices.py` 从 Go 缓存收集 macOS arm64 的 App/CLI 生产依赖模块提供的许可、归属及 NOTICE 文件，附 Go 工具链许可，写出版本、模块摘要和文件 SHA-256；不会改变组件许可。每个模块内的全部相关文件均保留，部分文件涉及未链接的可选或测试资产，不声称清单中的每项都会进入可执行文件。

`icons.go` 沿用的 Lucide SVG 路径保留上游 ISC 及 Feather 衍生图标的 MIT 声明。完整许可证来自 [Lucide 固定提交](https://github.com/lucide-icons/lucide/blob/e715245d62667c800e7f54c94b1b023692e900a3/LICENSE)，原样保存在 assets/notices/lucide，生成时核对固定摘要并纳入 licenses/third-party-assets/lucide/LICENSE。该提交固定的是许可证快照；旧图标几何的具体上游版本没有记录，不把两者等同。

Go LICENSE 必须存在；PATENTS 不能替代它。对于 Homebrew 分发，收集器也会检查同一公式前缀的 LICENSE，不要求最终用户安装 Homebrew。当前清单为 15 个模块、50 个生成文件，加顶层 LICENSE/NOTICE 共 52 个随包声明文件。详见 [来源核对](source-provenance.md)。

更新依赖后先 `go mod download`，再运行生成脚本并审阅差异。`--check` 只核对，模块网络访问关闭；缺少缓存或存在替换模块会拒绝。发布脚本和 CI 均检查清单。第三方开发运行时按需下载，不包含在此 App 的 Go 依赖清单中，其原始归档许可仍需保留。
