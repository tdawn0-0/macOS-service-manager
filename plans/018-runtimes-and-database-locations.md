# Plan 018: Java、Go、uv Python 与多版本数据库数据目录

## Status

- Status: DONE（隔离工作树；原生安装与外部卷实机未验证）
- Baseline: `c4246ae`，隔离工作树 `/Users/jyeu/code/service-manager-015b`
- 用户已授权实施；本需求扩展并取代旧计划“五软件”的范围。仍无 Project、PHP、Site 管理；保留 HTTPS 规则。
- 旧 015 的签名/Chrome 实机验收仍未完成，不以本轮 mock 替代。

## Product contract

Software 新增 Java、Go、Python，与 Node/四数据库并列。支持可用版本、已安装精确版本、显式安装/卸载、默认与终端使用。Java 首版 Temurin JDK；Go 官方归档；Python 以 uv 为唯一安装/发现后端，尊重用户已有 uv 管理目录和解释器。数据库创建流程显示引擎、精确软件版本、端口、数据目录，可并存同一引擎不同版本的实例。

自定义数据库目录用于新实例；不自动接管非空目录或改变已有数据库主版本。当前需求不包含数据升级/迁移器。元数据、凭据、任务记录仍由应用管理；外部数据目录的归属必须明确，Remove 保留数据；首版外部目录禁用永久删除并显示准确路径/原因，应用默认内部目录保留现有可恢复永久删除。没有证据证明属于本实例的目录永不递归删除。

## Evidence and sources

基线 `catalog.go` 已有 Go Resolve 官方 JSON 和 SHA256，但 `AvailableVersions`/GUI 不显示 Go。`view_development.go` 软件列表只有 node/mysql/postgresql/redis/mongodb。`Instance` 仅 ID/Kind/Package/Port/Persistent；`instanceDirectory` 固定 Root/instances/id。数据库启动、初始化、恢复与删除都直接拼接该位置，必须整体审计调用者，不只添加输入框。

原实现示例：
```go
func (m *Manager) instanceDirectory(id string) string {
    if c := m.initialization; c != nil && c.record.Instance.ID == id { return c.dir }
    return filepath.Join(m.Root, "instances", id)
}
```

官方来源（执行前核对实际 CLI/JSON，不猜测输出结构）：
- https://docs.astral.sh/uv/guides/install-python/
- https://docs.astral.sh/uv/reference/cli/
- https://docs.astral.sh/uv/reference/storage/
- https://docs.astral.sh/uv/concepts/python-versions/
- https://adoptium.net/installation/archives/
- https://api.adoptium.net/
- https://go.dev/dl/?mode=json&include=all
- https://dbngin.com/ （仅参考多版本、多端口实例体验，不依赖其私有下载接口）

## Scope

允许 `internal/devplatform/*.go`、`internal/devcli/*.go`、根 `view*.go`、`menubar.go`、根 `*_test.go`、`scripts/*.py`、README.md、CONTRIBUTING.md、docs/product-scope.md、docs/resources-acceptance.md、docs/development-*.md、docs/images/development/*。不改系统服务 provider、签名凭据、依赖模块或用户真实数据。执行代理在新的隔离 worktree 中实施，基于 c4246ae，提交但不合并/推送。主工作树 plans 由审查者维护。

## Conventions

沿用 Manager 注入 Run/Home、资源租约、withState 原子事务、任务取消和 a.post 主线程回调。原生 MyGo UI，900×520 和1120×720明暗主题。默认/安装状态分开；不在渲染回调下载软件。测试 TempDir/httptest/fake runner，避免真实 HOME、数据库和 uv 仓库写入。

## Steps

### Step 1: 安装与可用版本后端

Java 从 Adoptium API 获取 macOS arm64 JDK 精确版本、archive URL、SHA256，复用安全解包，正确定位 Contents/Home/bin 与 JAVA_HOME。Go 复用官方版本索引及校验，不硬编码臆造哈希。可用目录失败显示可重试，已安装版本仍可使用。

Python 探测明确 uv 路径/版本并使用参数数组调用 uv python list/install/find/uninstall，优先机器可读输出，最低 CLI 能力不支持则明确提示。复用既有 uv 仓库，不移动解释器、不扫描删除 venv。不把 system Python 当 uv-managed 来卸载。安装须明确请求精确版本，不依赖 cwd 的 .python-version；缺 uv 提供可执行的安装入口（固定官方 release archive+checksum，不 curl|sh），或可选已有绝对路径。目录环境沿用 uv 标准；测试 override 到 TempDir。

**Verify**：新增 TestRuntimeCatalogs / TestUVPythonManagement；fake HTTP/Run 验证版本解析、哈希失败、取消、缺 uv、未知输出、两 Python 版本、只卸载选中 uv-managed 版本，无真实安装副作用；go test -race ./internal/devplatform -run '^(TestRuntimeCatalogs|TestUVPythonManagement)$' -count=1。

### Step 2: 版本使用与界面

Java/Go 支持已安装版本默认选择和可执行路径复制；显式启用终端，zsh 安全标记块与 symlink/并发保护沿用 Node。Java 同步 JAVA_HOME；Go 不全局强设 GOROOT/GOPATH。Python UI 明确由 uv 管理，默认操作采用官方可支持的 uv 语义，说明对 uv run、python 命令和已有终端的实际效果，不承诺覆盖 venv。保持 .python-version/uv 项目优先级；不引入项目注册或 uv sync/pip 管理。

**Verify**：新增 TestAdditionalRuntimeSelection 与 TestUIAdditionalRuntimes，验证不同版本、无匹配、不误改其它 runtime 默认、shell启用/取消、四尺寸主题；go test -race ./... -run '^(TestAdditionalRuntimeSelection|TestUIAdditionalRuntimes)$' -count=1。

### Step 3: 数据目录贯穿实例生命周期

增加明确的数据路径字段和统一 resolver，区分应用元数据与数据库数据；新建必须绝对、规范化路径，拒绝 symlink、非空已有数据、重叠/重复目录、应用元数据根、根目录或用户主目录。路径所有权标记/文件身份与实例 ID 绑定；同目录创建通过租约和事务防并发。初始化临时目录应在目标同一卷，失败/取消保留可恢复记录且不删除用户既有内容。停止/启动、备份/恢复、端口变更、重启、删除预览/恢复都使用同一规则；跨卷不能假设 rename 原子可用。不能仅改 startInstance。

显式选择已有的精确 package ID，列表显示引擎实际版本；至少用 PostgreSQL 两个已有支持版本验证并存，不声称所有引擎任意历史版本可安装。目录/数据初始化后不提供无迁移的原地改主版本；新增独立实例使用另一个版本。版本列表只列经过真实来源验证的支持版本，保留数据库依赖卸载保护。

**Verify**：新增 TestDatabaseCustomDataDirectory / TestDatabaseVersionIsolation / TestUIDatabaseLocations，涵盖两版本两目录两端口、四引擎路径参数、失败/取消/重启、Remove保留、内部永久删除准确预览、外部永久删除在副作用前拒绝且路径原因可见、未知内容保护、symlink替换、重复/嵌套路径、空格与跨卷行为；go test -race ./... -run '^(TestDatabaseCustomDataDirectory|TestDatabaseVersionIsolation|TestUIDatabaseLocations)$' -count=1。

### Step 4: 文档与完整验证

更新用户文档、软件数量假设及脚本/测试要求，不让旧五类断言误判。新增 runtime 可以独立 suite 覆盖，保留原五类真实 native gate；若扩展 required，必须父子事件实际执行，不把无缓存 SKIP算通过。明确本机不能执行的真实安装/系统信任边界。

**Verify**：go test ./... -count=1；go test -race ./... -count=1；go vet ./...；PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'；python3 scripts/collect-third-party-notices.py --check；git diff --check。生成并查看新界面四尺寸主题截图。测试名先 go test -list 确认存在，再用 Go JSON 事件核实 PASS，空跑不得过关。

## Done criteria

- Java/Go/Python 入口实际支持安装与多版本操作；Python 由 uv 管理，不另造解释器仓库。
- 数据库能选择精确软件版本和新建数据目录，两不同版本实例隔离；四引擎完整路径处理有测试。
- 取消/目录冲突/默认丢失可解释，卸载/删除不误删其他数据。
- 上述聚焦和完整检查通过；记录真实安装未验证项，不冒称 native通过。

## STOP conditions

依赖来源无法验证、需要非授权目录/凭据操作、跨卷或恢复实现无法保证数据保护时报告具体缺口，不绕过保护。不要用占位按钮、静默忽略字段或删测试满足验收。若需调整 Scope 先报具体路径；正常实施细节可自行决定并记录。

## Maintenance notes

uv 版本列表随 uv 发行更新，CLI输出需兼容能力检测。外部数据库目录是生命周期契约；未来迁移/升级必须单独设计。保留 Node 和HTTPS现有行为。

## Execution record

待填写实际提交、差异、命令、PASS/SKIP、截图和未验证边界。

实施细化：外部目录可能跨卷，现有永久删除事务在应用根内依赖 rename；首版明确保留外部数据，不实现跨卷递归删除。这是实现用户自定义目录需求时的数据保留边界，不阻塞运行/多版本使用。

执行完成：工作树 `/Users/jyeu/code/service-manager-018`，分支 `codex/018-runtimes-database-locations`，实现提交 `2f221b5`，最终 `96549ea`。新增 Java/Go/uv Python 与数据库数据目录；Python 采用选择既有 uv 路径的方案，不提供 uv bootstrap 下载。外部目录永久删除禁用，Remove 保留。完整 Go/race各502 PASS、30环境门控SKIP；七组新测试父项及32父子事件PASS、零SKIP；vet、mod verify、Python31项、notice、MyGo构建PASS；16张截图已查看。审查者复跑全部七组聚焦race、全包编译及diff check通过。真实运行时下载安装、PG18数据库、外部卷真实数据库恢复与签名Chrome尚未验证。所有代码在隔离工作树，主工作树源码未合并。

集成记录：按用户“合并 master”的明确指示，实现提交链已于本轮快进合入 master（`96549ea`）。此前工作树记录保留为历史；未验证的原生安装、外部卷与系统信任项目仍保持原状态。
