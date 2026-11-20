# 第三方软件与包来源

PostgreSQL 通过 Apple Silicon Homebrew core 的版本公式管理。Java 使用 JetBrains JDK 列表，按其 `install_folder_name` 保存在 `~/Library/Java/JavaVirtualMachines`，Python 使用现有 uv 的安装目录。其他归档软件由 App 下载并校验，按版本保存在 `~/Library/Application Support/Devbase/Development/packages`；下载缓存保存在 `~/Library/Caches/Devbase/Development/downloads`。Homebrew 软件保留在 Cellar，App 记录精确 keg 版本（含 revision），不跟随 `opt` 软链接切换版本。下表记录当前目录来源；实际授权文件以安装包内的 LICENSE/NOTICE 为准，此表不改变组件许可。

| 软件 | 当前目录版本 | 发行来源 | 目录记录的许可 |
| --- | --- | --- | --- |
| Java | JetBrains 列表中的 macOS arm64 非预览发行版 | [JetBrains JDK 列表](https://download.jetbrains.com/jdk/feed/v1/jdks.json)，下载链接指向相应发行方 | 各发行版许可不同，以源站条款及包内许可为准 |
| Python | uv 目录提供的 macOS arm64 版本 | [uv Python 管理](https://docs.astral.sh/uv/guides/install-python/) | 以解释器发行包内许可为准 |
| Node.js | 22.14.0、20.19.0 等精确版本 | [Node.js 官方归档](https://nodejs.org/dist/) | MIT 及包内组件许可 |
| MySQL | 8.4.13、8.4.4 | [MySQL 官方下载](https://dev.mysql.com/downloads/mysql/) | GPL-2.0 及包内组件许可 |
| PostgreSQL | 17、18、16 系列，可输入精确版本 | [Homebrew core](https://formulae.brew.sh/formula/postgresql@17)；如 `postgresql@17` | PostgreSQL 及包内组件许可 |
| Redis | Redis 8.2（Homebrew `redis@8.2`） | [Homebrew core](https://formulae.brew.sh/formula/redis@8.2) | AGPL-3.0-only AND BSD-2-Clause AND BSL-1.0 AND MIT AND (CC0-1.0 OR BSD-2-Clause) |
| MongoDB | 8.0.38、8.0.5 | [MongoDB 官方下载源](https://fastdl.mongodb.org/osx/) | SSPL-1.0 |

MySQL 和 MongoDB 归档包在目录中声明最低 macOS 26（`app/crates/devcore/src/software/catalog.rs` 的 `min_macos`）。App 只验证并支持最新的两版 macOS（目前为 26 和 27），更旧的系统不在支持范围内。

Redis 实例直接使用 Homebrew keg 中的 redis-server，不用 Valkey 替代。PHP 不再由应用管理。

应用本身直接依赖 nsh、hyper/rustls、serde、plist 等 Rust crates；精确版本及传递依赖见 `app/Cargo.lock` 与 `app/crates/*/Cargo.lock`。项目自有源码已按计划采用顶层 Apache-2.0 LICENSE/NOTICE。标准文本来自 [Apache 官方](https://www.apache.org/licenses/LICENSE-2.0.txt)，原作者在 Git 历史中保留为 J.yeu/Jyeu。外部组件保持原许可；新增源码或资产的来源仍须在发布前审阅。

每个已安装包保存来源描述和摘要。Homebrew 包的 SHA-256 是 formula 定义的摘要，不是 bottle 摘要；bottle 与依赖由 Homebrew 校验管理。App 检查安装收据的 tap、版本与必需可执行文件，不声称完成 Homebrew 全文件树校验。工作区移除只删除注册记录，保留共享 keg；修复需停止使用方后执行 `brew reinstall <formula>`。Homebrew 不保证历史补丁版本长期可安装，精确版本不可用或 formula 改变时会报错，不静默改装其他版本。项目尚未发布，已移除旧 Lerd/Postgres.app 安装实现，不保留兼容或数据迁移路径；旧验证安装与测试数据清理后重新建立。

新增归档版本须更新目录、校验摘要、检查动态库与迁移路径，并在目标 Mac 上运行。产品仅支持 Apple Silicon（arm64），不提供 Intel 专用包。上游通用归档须包含 arm64；保留其原始校验和及归属。


App 随包包含 LICENSE、NOTICE 和 licenses 目录。`scripts/collect-third-party-notices.py` 从已 fetch 的 crates.io 缓存离线收集 macOS arm64 的 App/CLI 生产 crates 提供的许可、归属及 NOTICE 文件，写出版本、声明的 SPDX 许可表达式和文件 SHA-256；不会改变组件许可。未随包发布许可文件的 crate 以其 Cargo.toml 声明的许可归属。每个 crate 内的全部相关文件均保留，部分文件涉及未链接的可选或测试资产，不声称清单中的每项都会进入可执行文件。

`app/src/components/icons.rs` 沿用的 Lucide SVG 路径保留上游 ISC 及 Feather 衍生图标的 MIT 声明。完整许可证来自 [Lucide 固定提交](https://github.com/lucide-icons/lucide/blob/e715245d62667c800e7f54c94b1b023692e900a3/LICENSE)，原样保存在 assets/notices/lucide，生成时核对固定摘要并纳入 licenses/third-party-assets/lucide/LICENSE。该提交固定的是许可证快照；旧图标几何的具体上游版本没有记录，不把两者等同。

每个 crate 必须声明许可表达式或随包提供许可文件；两者皆无时收集器拒绝生成。清单条目和摘要以 `licenses/manifest.json` 为准，`licenses/README.md` 由同一脚本生成。

更新依赖后先在 `app/` 及 `app/crates/` 各 workspace 运行 `cargo fetch --locked`，再运行生成脚本并审阅差异。`--check` 只核对，Cargo 网络访问关闭；缺少缓存或存在非 crates.io 来源会拒绝。发布脚本和 CI 均检查清单。第三方开发运行时按需下载，不包含在此 App 的 Cargo 依赖清单中，其原始归档许可仍需保留。
