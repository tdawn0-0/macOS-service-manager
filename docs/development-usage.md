# 本地开发环境：使用与验证

## 当前使用方式

```sh
go tool mygo dev
go build -o build/devmgr ./cmd/devmgr
build/devmgr help
```

打开 Development，四个页面均通过图形表单配置，无需编辑 JSON：

- Projects：左侧项目列表，右侧总览、服务、环境和日志。选择目录导入、编辑项目名称和目录；添加和编辑服务、选择已安装运行时、填写参数、工作目录、环境变量、依赖、健康检查和重启策略。勾选数据库后保存。底部固定“Save project”，一次保存当前服务和环境字段；菜单栏也可启停项目和打开网站。
- Software：PHP 使用 shivammathur/php tap，PostgreSQL 使用 Homebrew core；先安装 Apple Silicon Homebrew。PHP 可在页面点击 Add shivammathur/php repository。选择系列或精确版本，Review package 解析为精确 keg 版本（含 revision），再点击 Install with Homebrew。其他软件仍下载校验后的归档。已安装版本绑定项目草稿，保存后生效。
- Databases：选择实例或创建实例，选择引擎和已安装版本、端口、持久化；启停重启、查看/复制连接、单独复制密码、打开默认外部客户端、建库、创建用户、备份、选择备份文件恢复到新数据库、移除实例管理记录。
- HTTPS：选择独立或项目站点，填写域名、上游、HTTPS 端口；支持重命名、移动所属项目、停用和删除。保存后用 Apply proxy 应用规则；可配置本地域名、信任或移除本地 CA、执行诊断。

编辑服务时，程序及每个参数分别填写在独立行，以保留带空格的参数。环境变量使用 NAME=value 的普通表单输入。保存前需停止运行中的项目；站点规则可单独保存和应用。

图形操作显示阶段；App 归档下载显示字节进度。下载、缓存验证、归档解包与 Homebrew 命令支持上下文取消，失败不注册半成品包。Homebrew 是共享安装，取消后已完成的 keg 或依赖由 Homebrew 保留，不承诺回滚系统安装。系统授权框的取消由 macOS 处理。

## 当前软件支持状态

本项目仅支持 Apple Silicon Mac（arm64），不维护 Intel App 或 Intel 软件包。

目标首版软件为 PHP、Node.js、MySQL、PostgreSQL、Redis、MongoDB。软件选择器已按该范围展示，各引擎的验证状态如下（当前实机为 Apple Silicon）：

| 软件 | 当前已验证的实现 |
| --- | --- |
| Node.js | 上游精确版本下载、校验、项目版本选择和 launchctl 托管 |
| PostgreSQL | Homebrew core postgresql@17 / @18 / @16；实例仍使用 App 的独立数据目录和端口 |
| PHP | shivammathur/php tap；支持选择 8.5 / 8.4 / 8.3 等系列，解析当前补丁版本 |
| MySQL | 官方 8.4.4 包；独立实例、用户、数据库、备份恢复已实测 |
| Redis | Redis Stack 中的 Redis 7.4.7 服务；认证、启停和持久化已实测，不使用 Valkey 替代 |
| MongoDB | 官方 8.0.5 包；认证实例、用户、数据库、原始 BSON、索引、集合校验规则和视图的备份恢复已实测 |

旧的 Go 和 Valkey 包管理后端仍保留兼容。目录只解析 arm64 下载；自定义清单和启动路径拒绝 Intel-only 包及可执行文件。PHP/PostgreSQL 不再保留 Lerd/Postgres.app 的安装、解包或迁移逻辑；项目尚未发布，旧测试数据直接清理。软件包携带各自许可证，开源 App 不改变这些组件的授权条件。

当前 App 使用 Go 1.27.3 构建，实际最低系统为 macOS 13，安装包已据此声明；此前 macOS 12 声明与 Mach-O 要求不符。用户已于 2026-11-07 明确采用 macOS 13+，替代原兼容目标；[Go 官方要求](https://go.dev/doc/go1.27#darwin)与当前 Mach-O 一致。当前目录中的 MySQL 包要求 macOS 15+，Redis 要求 14+，MongoDB 要求 13+；PHP/PostgreSQL 的系统及依赖支持由 Homebrew formula/bottle 决定。安装时检查包的最低系统版本。这些是工具链和包清单要求，不能等同于已在所有系统版本实测通过。

用户随后指定：以后只验收电脑已安装的当前 macOS 版本。本轮实际环境是 macOS 27.3 arm64，不要求另备 macOS 13 机器或虚拟机；文档保留“最低要求”与“实际测试环境”的区别。

## 数据与权限

默认存储目录为 ~/Library/Application Support/ServiceManager/Development。MSM_DEV_HOME 可指定隔离测试目录。App 管理归档包、实例配置和数据；Homebrew 管理 PHP/PostgreSQL 的共享 Cellar 安装与依赖；launchctl 管理常驻进程。移除管理记录与卸载包不会删除数据库数据。安装包来源和摘要保存在内部清单，用户无需编辑这些文件。

HTTPS 只监听 IPv4 回环地址。配置 hosts 和系统 CA 信任会显示 macOS 授权对话框；CA 信任允许本工具签发受本机信任的证书。当前已通过用户钥匙串信任授权和 Chrome 默认 443 HTTPS 实机验证（当前浏览器验收范围仅 Chrome）。低端口由 launchd 创建监听，代理进程以当前用户运行。首次创建或变更系统端口任务需要管理员授权；路由变更复用已有任务。详见验证记录。

## 验证

```sh
go test ./...
go test -race ./...
go vet ./...
go build -o /tmp/msm-devmgr ./cmd/devmgr
python3 scripts/verify-development.py
MSM_DEV_SHOTS=/tmp/msm-dev-shots go test -run TestDevelopmentPanes .
go tool mygo build
```

现有真实服务脚本验证 Node、Go、PostgreSQL、Valkey 和使用指定 CA 的 HTTPS。它不覆盖新目标中的 PHP/MySQL/Redis/MongoDB，也不修改 hosts 或系统 CA 信任。MSM_SMOKE_BINARY 可指定 CLI，MSM_SMOKE_CACHE 可复用重新校验的下载缓存。

新增引擎集成验证：`MSM_SIX_INTEGRATION_ROOT=<隔离的包目录> go test -v ./internal/devplatform -run TestAdditionalEnginesIntegration -count=1`。先通过 Software 页面或 CLI 安装对应软件包。

首版目标保持进行中。环境本机覆盖已补上并通过双工作副本 UI 验证；剩余恢复场景仍需补齐，逐项状态见 [完成核对](development-completion-audit.md)。发布前还需 Apple Silicon 上的最低系统实机验证、干净机器安装、Developer ID 签名公证，以及许可证来源确认。现有凭据使用受限文件。

项目总览的 Export environment 生成 schema 2 共享声明和版本锁。共享层使用端口引用，移除命令中的本机路径、秘密参数和环境值。私有设置仍保存在 App 目录。导入仅预览和登记；点击 Prepare imported environment 才下载目录中的精确版本并初始化数据库。锁内的任意下载 URL 和可执行路径不会被用于安装；同架构摘要必须匹配应用目录。历史其他架构的环境锁保留解析兼容，但本产品只解析和安装 arm64 可用包。每个工作副本有独立实例 ID、端口和新凭据，不复制数据和账户。导入为服务及数据库分配空闲端口，替换已识别的命令、PORT / *_PORT 环境、就绪与上游引用，并避免已用域名。HTTPS 监听优先复用本机已有配置。固定在程序内部或未识别参数中的端口仍需自行配置。未知命令值以本机参数补填；Complete local setup 带到对应表单，补齐后才能启动。外部上游需要在 HTTPS 中审阅保存后才启用。

项目 Logs 页面可读取服务日志、手动轮转和开关自动轮转。普通用户 launch agent 每 60 秒执行一次有限时维护任务，不运行第二套进程监管器。每份归档最多 5 MiB，保留两份；检查间隔内日志可继续增长。copy/truncate 的瞬间可能遗漏并发输出，不能用作审计日志或数据库恢复日志。关闭 App 后维护仍运行，关闭开关会解除该任务。

项目登录启动使用私有目录中的稳定入口，不依赖原 App 或构建目录的位置。运行中的项目需先停止再修改环境；未保存的表单不会被“启动”或“导出”按钮悄悄忽略。

MongoDB 逻辑备份支持最多 256 MiB 原始文档数据，读取备份文件上限 512 MiB；不备份管理用户和系统集合，不承诺跨集合时间点一致性。恢复只写新数据库。恢复中途失败可能保留部分新库，需要检查结果后换新库名重试。

UI 测试命令与界面设计说明见 [界面设计与端到端测试](development-ui.md)。

Redis 的备份/恢复使用数据库编号 0–15；恢复目标必须为空。项目设置支持登录自启动、环境文件引用；导出的共享配置关闭本机自启；绝对环境文件通过本机表单重新选择，不复制文件内容。

默认 HTTPS 端口为 443，项目网站按钮省略 :443。独立 HTTPS 页面提供打开网站、重启和停止代理的按钮。当前用户安装的 CA 信任用于本机浏览器；其他设备仍需自行配置解析与信任。

HTTPS 的 Customize proxy settings 提供连接超时和响应头超时，单位为秒，范围 0–3600，0 表示关闭该超时。默认值为 10/30 秒。响应头超时不截断已开始的 SSE 或 WebSocket。Host 默认保留本地域名，也可选择上游地址；转发头重建为原始 HTTPS 请求信息，不信任客户端伪造的转发值。上游超时、拒绝连接和证书校验失败分别显示错误页。损坏的路由文件不会替换内存中的上一份有效配置。

数据库详情显示关联项目。项目启动后通过一个有限时 launchd 标记记录本次登录会话中的归属，不运行额外监管进程；只有数据库的项目也能正确持有共享实例。停止一个项目不会停止另一个活动项目仍使用的实例；未启动项目不会误停单独启动的实例。项目和独立数据库均可通过按钮或 CLI 重启。默认客户端通过无密码 URI 打开，密码需要用户显式复制，任务结果中不显示密码。

测试管理员操作可用 build/devmgr proxy-verify 合并为一次授权，验证停止、端口释放、恢复与重启。build/devmgr proxy-stop 移除本应用的网络监听任务；站点配置和 CA 数据保留。测试授权失效时 macOS 仍可能要求重新认证，应用不改变永久免密规则。

导入时读取 .nvmrc、.node-version、package.json engines/volta、.php-version 和 composer.json PHP 声明，展示来源。不同精确版本的冲突会提示；范围不会被假装解析成精确版本。只发现声明不会自动下载或执行 npm/composer 脚本；仅在存在 dev/start 脚本时建议 npm 命令。总览可重新扫描，运行时选择不覆盖版本声明文件。Node 支持手输上游精确版本；不存在的版本明确失败，不静默换版本。

顶部 Diagnostics 或 HTTPS 的 Diagnose HTTPS 打开分层报告。DNS 必须只返回代理实际监听的 127.0.0.1，其他回环别名、IPv6 或外部地址不会被当作成功。解析超时单独标记为未完成的警告，不直接判断 hosts 配置错误。检查进程/就绪、数据库端口、CA 有效期与 macOS 信任、上游 TCP/TLS，以及以系统信任校验的本地 HTTPS。归档软件包按已记录索引检查完整文件树；Homebrew 包仅核对收据的 tap、版本与必需可执行文件。数据库认证和普通上游 HTTP 响应不在该报告的检测范围，页面和结果会说明。诊断只读取状态，不应用 hosts、修改信任或启停服务。

Copy redacted report 复制匿名 JSON；Export redacted report 在私有 reports 目录写出 0600 文件并在 Finder 显示。资源名、域名与证书标识替换为本次报告的匿名编号，不导出环境、命令、日志、数据库密码、私钥或本地路径。CLI 对应 devmgr doctor、doctor --json、doctor --export。报告标记检查时间，修复后需要重新运行。

Services 提供 Readiness timeout（1–3600 秒，旧配置默认 30 秒），Restart throttle（1–3600 秒，旧配置默认 10 秒）。就绪地址支持 process、tcp://host:port 和 HTTP(S)。启动需要主进程持续运行后才检查就绪；快速退出会失败，另一个进程恰好响应同一地址也不会让退出的服务通过。空检查只确认进程启动，状态为 Running；显式检查通过为 Healthy，失败但进程仍在为 Not ready。process 检查不代表应用端口已就绪。

工作区每 5 秒在 UI 活跃时刷新状态，手动 Refresh 也会立即检查。编辑期间状态刷新保留草稿；较早的查询不能覆盖后来发起的启停操作。观测超时显示 Unknown，不能把它理解为已停止。服务详情展示 PID、启动次数、最近退出码和可解析的退出信号；连续启动次数较多时提示查看日志。CLI status 新增 runtime 状态信息，原有配置字段保持可读取。

自动重启默认关闭。开启后由 launchd KeepAlive 和 ThrottleInterval 执行；节流限制短时间内的重复启动，不承诺精确重试次数或指数退避。停止项目会解除相应任务，不继续重启。[Apple launchd 手册](https://github.com/apple-oss-distributions/launchd/blob/main/man/launchd.plist.5)说明了该节流语义。

关闭窗口继续保留服务。菜单栏 Quit、系统退出菜单或 CmdQ 进入退出选择：Cancel quit 取消；Keep services and quit 保留运行状态；Stop services and quit 按预览停止此存储目录管理的项目、数据库和日志维护任务。共享数据库在相关项目停止后释放，独立受管实例也会停止。外部进程、其他目录的实例和其他系统服务不会被接管。

退出范围包含所有已配置项目与数据库，不限于预览时正在运行的资源；点击前配置若被其他客户端改动，会要求重新预览。Stop local HTTPS proxy 默认勾选，可取消以保留监听。停止系统 HTTPS 监听可能需要 macOS 管理员授权；不改变信任、hosts、数据、登录自启动偏好或项目配置。保留服务退出不执行停止操作。

有正在执行的操作时需要等待或取消，未保存编辑会提示丢弃。停止过程可取消；失败或管理员授权取消时保持 App 打开，明确提示部分资源可能已经停止，允许重新预览后重试，或保留剩余服务退出。下次打开 App 会按原日志维护偏好恢复维护任务；原先启用的项目登录启动偏好仍生效。

Software 的每个已安装版本提供 Check files、Repair 和 Reveal package folder。新安装在校验归档后记录文件内容 SHA-256、访问权限、文件类型、相对符号链接和目录条目；索引摘要保存在独立的本机状态中。检查显示 Verified、Needs repair 或 Not recorded，并标记检查时间。检查结果是该次快照，修改文件后需要重查；不跟随包外链接，不读取非普通索引文件。旧安装不会被现场扫描后自动视为可信基准，需要从已验证归档修复后建立索引。

Repair 重新准备已登记的同一来源、版本、架构和校验值，重新校验缓存或下载，不执行安装脚本。先停止使用它的项目、数据库和 CLI 命令；仍登记的 job 或无法确定的进程状态会阻止替换，配置中保留的已停止引用可以继续使用同一版本。CLI exec 持有包级共享租约，修复及卸载使用独占租约。

macOS 以同卷目录交换完成替换，原目录在状态提交成功前保留。取消或状态保存失败会恢复原目录及索引；下载失败不改动安装目录。文件系统不支持安全交换时拒绝替换。数据目录与安装目录分离，修复不改动数据库数据、项目环境或系统默认运行时。异常的安装路径、无法安全处理的索引类型和清理失败会明确提示查看，保留待检查文件。

CLI 对应 devmgr check-package <package-id> 与 devmgr repair <package-id>。Needs repair 使检查命令返回非零；Not recorded 明确标记没有可信基准。完整性范围不包括时间戳、扩展属性或对能改写整个本机状态的同一用户构建安全边界。


共享环境导出会短暂生成 .devmanager-export.json 恢复记录，正常完成后自动移除。记录权限为 0600，只保存脱敏后的新声明、版本锁和旧文件摘要，不保存旧声明正文；无需编辑它。准备阶段、两个保存阶段和操作编号会显示在任务进度中。

若导出被取消或 App/CLI 中途退出，导入会先提示未完成的操作。在导入页或项目总览点击 Review export recovery，核对当时的项目快照、版本和文件状态。Complete interrupted export 继续保存同一份快照，不重新执行项目代码。若期间另有文件改动，恢复不会覆盖；可选择 Discard export record (keep files) 保留两个文件的现状，再从注册项目 Export environment。丢弃记录不回滚文件，也不会使不匹配的文件变得有效。

新导出在声明中记录版本锁摘要；App/CLI 导入会拒绝摘要不符的文件组合。两个文件仍逐个原子替换；App 的导入、导出和恢复通过同一目录身份的跨进程锁协调，未提交的操作不能被导入。schema 1 和早期没有摘要的 schema 2 保留兼容，不追溯保证旧文件的配对一致性。

可选 CLI 使用 export-recovery <directory> 查看阶段和 Fingerprint，再将该 Fingerprint 传给 export-recover <directory> <fingerprint> 或 export-discard <directory> <fingerprint>；文件改动后必须重新查看。


项目移动后，保留原管理记录。先 Stop project，再在 Finder 移动源码。进入项目 Overview，Choose working folder 选择新位置，或在 Working folder 填入新路径。Save project 会列出要调整的路径，核对后点击 Apply folder change。App 只更新管理配置，不搬移或删除源码。项目 ID、数据库实例关联、私有数据目录、HTTPS 域名和端口保留。目录缺失时会提示定位，Start / Restart 暂时禁用。

迁移自动调整指向旧项目目录内部的绝对执行路径、普通绝对参数、--flag=/absolute/path、路径型本机参数，以及绝对环境文件引用。相对路径和项目外的引用保持原值；秘密参数及环境值不自动猜测，含旧目录时提示到 Services / Environment 审阅。源目录下的文件由用户移动，新目录必须存在。登录启动器的 WorkingDirectory 同步更新；状态写入失败时恢复原启动器配置。配置或目标目录变化后，必须重新预览；应用时会再次确认项目已停止。

Add project HTTPS site 可从项目生成域名和上游建议。首次无既有监听时建议 443；已有监听时复用其端口。纯中文名称且目录也无法生成 ASCII 名称时使用稳定项目 ID 作为后备名称。域名有冲突时添加序号；这些只是待保存的表单，不会自动改 hosts、信任证书或启动代理。

可选 CLI：project-folder <project-id> <new-directory> 查看预览和 Fingerprint；project-folder-apply <project-id> <new-directory> <fingerprint> 应用同一份预览。不需要编辑项目配置 JSON。


遇到端口占用时，在 HTTPS 或 Databases 点击 Check port。检查只针对 IPv4 TCP 回环地址；显示可见进程元数据，不读取完整命令行、环境或尝试停止外部进程。In use 不表示该进程一定属于本 App，也不表示运行中的受管实例有问题；观察时间和实际启停错误需要一起看。进程信息不足时明确保留未知状态，不触发管理员授权。

Find available port 建议一个高位端口，避开当前监听及本 App 的已知配置。建议不保留套接字；其他进程仍可能随后使用它，启动时会再次检查。HTTPS 修改后先 Save HTTPS site，再 Apply proxy。新增监听预检失败时保留原已部署站点；应用代理后验证本机 CA 的 TLS 握手，而不是只判断 TCP 连通。更换已有系统监听仍可能需要用户完成 macOS 授权。

已有数据库修改 Database port 后点击 Save database port。先停止实例及关联项目；App 保留数据、凭据、用户和版本，更新 Redis 配置、MySQL 客户端配置及关联工作副本的私有环境锁。再次 Prepare 不会恢复旧端口。应用自身的连接环境需要用户在 Environment 中更新，App 不覆盖仓库 .env。服务页的就绪端口观察不自动修改任意启动脚本。

若端点变更被中断，停止相关资源后点击 Repair saved port settings。它按当前已保存的实例端口修复可识别的端点配置及私有锁，不更换密码或删除数据；之后检查端口并重新启动。有歧义、多个端口指令或不同实例身份的配置会拒绝自动修复。正常保存失败或取消会回滚已写文件；进程中断后可重复执行修复。

CLI 对应 port <number>、available-port、db-port <instance-id> <number>、db-port-repair <instance-id>。这些命令与 GUI 使用相同检查和保存逻辑，无需编辑 JSON。


### 本地 CA 状态与修复

在 HTTPS 点击 Local certificates，再点击 Refresh certificate status。查看不会创建 CA 或修改钥匙串。页面显示公开指纹、期限、材料匹配和 macOS 验证结果。Trust 为证书验证结果；过期证书验证失败不表示钥匙串中一定没有旧信任记录。私钥不会显示或复制。

CA 到期或私钥损坏时，点击 Prepare replacement CA。核对新指纹，再点击 Trust prepared CA；macOS 如需授权，须在本机完成。取消授权不会替换当前 CA。验证信任通过后先 Stop HTTPS proxy，再 Activate prepared CA，最后 Apply proxy。系统 443 的停止或重新安装仍可能需要管理员授权。站点、hosts 和数据库配置保留；App 不承诺跨多次系统操作只输入一次密码。

App 将每代证书和私钥保存在独立受限目录，使用原子指针切换成对材料。新材料权限为 0600，目录为 0700。旧版普通文件继续可读；只有显式切换才迁移。不会因材料缺失或损坏而静默更换已有 CA。激活提交后若 App 退出，重新打开页面可通过 Finish CA activation 完成记录清理，继续使用已经切换的同一代。

已保存 CA 可 Trust saved CA 后 Use saved CA；恢复要求材料有效、信任验证通过且代理停止。准备中的替换须先完成或丢弃。Discard prepared CA 只移除待激活选择，保留材料供移除已授信任。Remove saved CA trust 针对显示的精确证书，不影响当前 CA；但同一证书被本机其他环境使用时也会受影响。旧信任不会在切换时自动删除。CA 指针损坏、特殊文件或有歧义的存储拒绝自动覆盖。

可选 CLI 与 GUI 共用控制层：ca-info、ca-prepare、ca-trust <revision>、ca-activate <revision>、ca-discard <revision>、ca-use <id> <fingerprint> <revision>、ca-saved-trust <id> <fingerprint>、ca-saved-untrust <id> <fingerprint>。revision 和公开指纹来自最新 ca-info；状态改变后须重新检查。所有配置均可通过 GUI 完成，无需编辑内部文件。


### 任务记录与中断处理

顶部 Tasks 查看 App/CLI 共用的本机任务记录。软件安装/修复、项目启停/重启、项目有限时命令、数据库初始化/启停、备份和恢复记录操作 ID、阶段、时间和结果。记录与跨进程操作租约分开保存。调用者异常退出且没有最终结果时显示 Interrupted；App 不会推定后台 launchd 服务也已退出。观察不停止任何进程。

运行中的任务由其调用者取消，其他窗口或 CLI 不能重试或清理记录。等待另一个配置修改时，显示 Waiting 阶段；取消等待不会执行后续修改。当前配置修改采用工作区状态锁串行排队，软件包另外持有包级租约。

Retry operation 重新检查当前配置、运行状态和健康，创建新的关联记录；软件重新解析的身份及摘要必须与原请求一致。不会根据旧 PID 停进程，也不会自动执行未知命令。任意命令不保存参数且禁止自动重放，需要在项目页显式重新执行。初始化和恢复只能 Review resource settings：新实例使用新 ID，恢复使用新库或 Redis 空编号目标；旧目标和未注册目录保持原状，须检查后单独处理。App 不自动删除孤立数据或终止未知的有限时子进程。

备份先写 .partial 文件，工具成功且写入同步后才发布为 .dump。中断输出不得当作成功备份；已知 .partial 文件不能直接恢复。重试备份写入新文件，不覆盖之前的部分输出；任务记录的输出在非 Succeeded 状态下仍须检查。Dismiss record (keep all files) 只删除该记录，不删除任何资源或输出。

任务记录为本机 0600 文件，不包含命令参数、环境、密码、原始错误输出或下载 URL；不会导出到共享声明或匿名诊断。可选 CLI：tasks、task-retry <id> <revision>、task-dismiss <id> <revision>。revision 来自最新 tasks 快照；旧预览和仍被调用者持有的操作都会拒绝。


### 数据库数据删除

Remove instance (keep data files) 仍只移除管理记录并保留目录。要删除仍受管理的实例数据，先备份需要的数据库，停止实例，并在项目配置解除所有关联。随后点击 Review permanent data deletion，核对实际路径、条目数、逻辑体积和影响，再输入完整实例 ID，点击 Permanently delete instance data。此操作删除该实例的全部数据库、配置和凭据，并移除实例记录；安装的软件、源码与该目录之外的备份保留。不会自动创建备份，也不提供逐库擦除或覆写磁盘的安全擦除保证。

预览只检查目录元数据，不读出数据库内容或密码。执行时重新核对实例、引用、job 停止状态、目录身份和元数据摘要。目录、已知文件或设置有改动时需要重新预览；符号链接不会被跟随到目录之外。特殊文件、外来所有者、跨文件系统树或无法安全移动的目录会拒绝自动处理，不提权修复。

删除先保存恢复记录，将目录移动到 App 的受限暂存区，再提交实例移除，最后擦除暂存内容。提交失败会尝试将目录原样移回。若 App/CLI 在移动后退出，Review pending data deletions 会显示下一步：移除未提交时 Restore original data；已提交时重新核对剩余内容、输入实例 ID 后 Finish permanent deletion。已经开始擦除时，取消只停止剩余工作，不能恢复已擦除文件。App 重开不自动继续破坏性操作。

恢复不覆盖已存在的其他目录。若原 ID 已供新实例使用，继续旧删除只作用于旧暂存目录，不会停止或删除新实例。删除记录、暂存区和操作租约由 App 管理；无需编辑内部文件。已有“移除记录后保留”的目录和未注册初始化目录不在此入口自动清理范围内。

可选 CLI：db-delete-preview <instance-id>；db-delete-data <instance-id> <fingerprint> <confirm-id>；db-deletions；db-deletion-finish <deletion-id> <revision> <confirm-instance-id>；db-deletion-restore <deletion-id> <revision>。指纹或 revision 必须来自最新预览。


### 后台软件任务与并发占用

Download and install、Use installed version 和 Repair 在后台执行，Software 显示每个版本的阶段、结果和取消入口。Package tasks 可返回任务；结果可单独关闭，不移除软件或持久化任务历史。下载期间可以切换项目、输入配置并保存。不同版本使用独立租约，准备完成后以短暂配置锁提交最新状态。

同一版本的替换、卸载或另一次安装遇到独占占用时会返回具体包身份；检查及 CLI exec 的只读租约可等待，并可取消等待。不要按历史 PID 停其他调用者；在实际发起任务的 App/CLI 完成或取消后重试。Review package tasks 提供共同记录。数据库初始化也已独立准备；建库、用户管理、备份和恢复已按实例独立执行；独立数据库启停也已移出工作区锁；项目生命周期也已使用项目、实例和版本租约，独立项目可并行；代理发布和整体退出仍串行协调。

修复在准备前及发布前都检查运行引用。准备期间若有项目或数据库开始使用原版本，发布会拒绝并保留原安装；新解包目录按事务结果清理，回滚失败时保留原目录供检查。取消一个任务不会取消另一个版本，校验通过的缓存可继续复用。

安装完成不自动保存项目，也不覆盖后来选择的运行时。目标服务及其选择保持原值时会填写草稿；若用户已作其他选择，可在 Services 选择已安装版本。完成刷新保留未保存草稿；不同调用者编辑同一项目时仍需用户审阅当前保存值和草稿，不承诺自动合并任意配置。

退出时如仍有后台软件或初始化任务，先等待或在退出对话框 Cancel background tasks。取消后重新读取实际范围，方可保留服务退出或停止受管服务。此规则只保护正常 App 退出，不声称系统强制终止后后台有限时子进程都会自动回收。

### 数据库后台初始化与中断恢复

Create instance 在后台准备独立数据目录。Database tasks 可查看阶段并分别取消。其他项目仍可配置和保存；独立实例可并行准备。同一实例 ID 或端口被准备任务占用时，其他调用者会收到具体资源的占用提示。安装包在准备期间受共享租约保护，不能替换或卸载。

初始化成功并确认临时 launchd 任务停止后，App 重新读取当前配置，再将准备数据移到实例目录并登记。不覆盖已存在的目录。失败、取消或进程退出会保留准备数据和记录；App 重开不会自动重放初始化。

Databases → Unfinished initializations 提供明确恢复操作。仍在运行的调用者需要先完成或取消。未登记数据可 Stop and archive data；仅停止记录且配置摘要匹配的初始化任务，再将数据保留到 Development/retained-initializations 下的独立目录。已登记实例可 Finish registration；只移除初始化记录，数据库保持。归档不是逻辑备份，也没有直接恢复或自动启动入口；需要新建实例时重新配置即可。

可选 CLI：db-initializations；db-initialization-resolve <preparation-id> <revision>。revision 必须来自最新快照。归档结果输出保留目录。它们不操作未知进程，不按历史 PID 判断归属，也不改变系统证书或管理员权限设置。


归档移动后意外退出时，Unfinished initializations 会依据原目录身份显示 Archived。点击 Finish archive 仅清理已完成的准备记录，归档文件和同 ID 的新实例保持。记录清理失败会显示错误，可重新刷新后再试；归档身份不符时不自动处理。App 不根据目录名称推断数据归属。


### 数据库后台数据操作

Create database、Create database user、Back up database 和 Restore into new database 在后台按实例执行。Database tasks 可返回进度和结果，每个实例可 Cancel database operation。其他项目和独立实例仍可配置。一个实例同一时间执行一项数据或生命周期操作；同一实例的启停、改端口、数据删除和项目启停请求会收到占用原因。退出停止范围包含忙碌实例时也会拒绝，等待实际调用者完成或取消后重新预览。

备份只在工具完成、输出同步后发布 .dump，取消时不发布部分输出。恢复仍要求新目标；取消不回滚已经写入的目标内容，失败后应检查该目标并重新选择空的新目标。调用者强制退出后的 worker 由下文 Unfinished database tasks 显式审阅，不自动重放建库或恢复。


### 实例后台启停

实例启动、停止、重启和移除记录按实例后台执行。独立项目可继续保存；任务运行时列表显示实际操作阶段，状态轮询不会提前覆盖它。完成后重新读取状态。取消不能撤销已经发生的停止或启动，请查看完成后的状态再操作；移除记录仍保留数据目录。

正在启停的实例有排他资源租约，其他调用者的同实例操作、关联项目启停以及绑定该实例的配置保存会返回占用原因。保存独立项目不受其就绪等待或停止等待阻挡。停止/重启前若无法确认关联项目已经停止，会拒绝；停止后无法确认任务已卸载时，不移除记录或报告成功。


### 项目后台任务与资源占用

Start / Stop / Restart、Prepare imported environment 和 Remove project 在后台按项目运行。Project tasks 显示进度，Cancel project operation 可单独取消，退出时可取消全部后台任务。独立项目可以同时启动，其他项目表单仍可保存。编辑中的草稿在完成刷新后保持；当前任务所属项目需要先完成或取消后再保存。

项目操作持有项目和关联实例的租约，启动时也保护选定的软件版本。同一项目或共享实例发生冲突时，App/CLI 返回具体占用原因。取消及失败只回收本次启动的资源，既有共享实例保持；清理无法确认完成时会报告错误。移除项目仍保留源码和数据库数据。项目就绪后应用代理时会重新读取当前站点，保留其他调用者在等待期间保存的设置；代理发布和退出范围处理仍串行协调。

受控项目启动遇到正在修复的版本时，会等待该版本的共享租约，并允许取消等待；独立配置仍可保存。修复发布前还会复查绕过租约的旧调用者或外来注册任务，发现实际运行引用时保留原安装并拒绝发布。

### 调用者退出后的数据库任务

备份、恢复、建库和建用户由 App 自己的有限时 launchd worker 执行。关闭调用它的终端或调用者异常退出，不代表工具已经停止。Databases → **Unfinished database tasks** 可以查看实际作业状态、目标和备份路径。

- 调用者仍在：在原任务中取消，或等待完成；另一个窗口不能替它收尾。
- 调用者退出但 worker 运行：刷新等待，或显式点击 **Stop database task**。
- worker 已完成：查看结果后点击 **Finish database task**，释放实例占用。
- 归属或状态无法确认：停止按钮不可用，保留文件供检查；不会按历史 PID 杀进程。

审阅不会自动重做恢复、建库或建用户，也不会撤销已经写入目标的内容。停止/收尾保留数据库和备份文件；`.partial` 不能作为可恢复备份。已完成但未审阅的记录仍会阻止同实例启停、端口/数据修改与软件替换，其他实例和项目可以继续操作。

worker 最长运行 30 分钟。正常调用者取消时会停止它自己的 worker；没有新系统授权。用户名和密码只通过私有输入文件传递（目录 0700、文件 0600），不放进作业参数或任务历史。完成收尾会清除该任务的私有输入与记录。CLI 的 `db-work` 和 `db-work-resolve <id> <revision>` 使用相同控制层；普通用户使用上述界面即可。


当前交付为 Developer ID 正式签名的 Apple Silicon 安装包，位于 build/notice-review/darwin-arm64；具体文件与摘要见验证记录末尾。新包补齐图标/Go 许可声明，并统一 macOS 13 最低要求。build/darwin-arm64 及 build/project-command-review 中此前交付的文件保留原字节。用户本轮选择暂缓 Apple 公证。系统 CA 信任与 Chrome 原生安全状态已经实际授权验证。项目登录配置和状态保存失败时会回滚；移除项目的状态提交失败也恢复此前登录文件与代理路由。证书清理支持信任设置已移除后的精确证书删除重试。

### 后台安装项目依赖与命令恢复

保存项目后，点击 **Install project dependencies**，以项目保存的运行时和环境执行 `npm ci`。该命令会运行仓库声明的生命周期脚本，需要先确认项目可信。导入项目本身不执行它。安装在 Project tasks 中显示进度和取消入口；其他独立项目仍可保存，未保存草稿保持。

`npm ci` 和 CLI `exec` 都由有限时 launchd worker 执行，最长 30 分钟。普通取消会停止自己的 worker；取消保留命令已经写入的源码、依赖和其他文件，不提供回滚。输出最多 64 MiB；界面仅展示末尾 16 KiB，CLI 返回完整受限输出。参数通过私有输入文件传递，保留原始参数字节，不放入 plist/argv 或操作历史；私有目录 0700、文件 0600。

调用者异常退出后，进入 Projects → **Unfinished project commands**：

- 调用者仍在时，在原任务取消或等待，不能从另一窗口收尾。
- worker 仍在运行时，刷新等待或点击 **Stop project command**。
- worker 已完成时，检查结果后点击 **Finish project command**。
- 归属改变或状态未知时，停止入口禁用，保留文件供检查。

**Reveal private command files** 打开经过目录身份核对的任务文件夹。输入和输出可能包含秘密，不会自动复制进操作历史。收尾仅清除该任务的私有文件和占用记录，保留命令对项目的写入，不自动再次执行。未审阅记录继续保护项目、关联数据库和软件版本；独立资源仍可使用。CLI `project-commands` 与 `project-command-resolve <id> <revision>` 提供相同能力，revision 取最新快照。

PHP/PostgreSQL 不使用 Homebrew 默认服务，仍由 App 为每个实例生成独立 launchd 任务。精确 Cellar 路径缺失时会报错，不跟随 opt 软链接改用其他版本。Homebrew 不保证历史补丁包可长期安装。工作区的 Remove from workspace 只移除注册记录，保留共享 keg；修复需停止使用方后通过 brew reinstall <formula> 完成。
