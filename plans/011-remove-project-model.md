# Plan 011: 移除项目、PHP 和站点模型，建立独立资源基础

> 执行者：本文件是独立交接计划。按步骤实施并验证；仅更新自己的状态记录。先读完 Scope 与 STOP conditions。计划作者本轮没有实施源码。
>
> 漂移检查：`git diff --stat 2ec48ab..HEAD -- 'internal/devplatform/*.go' 'internal/devcli/*.go' 'main.go' 'app.go' 'menubar.go' 'view*.go' '*_test.go' 'scripts/prepare-native-ui-verification.py' 'scripts/test_prepare_native_ui_verification.py' 'scripts/verify-development.py' 'scripts/verify-native-release.py' 'scripts/test_verify_native_release.py'`，另运行 `git status --short`。本文件摘录描述规划基线。前置计划造成的预期变化须逐项记录在本文件“Execution record”，对照本文明确的前置契约继续；其他无法解释的漂移停止报告。不要为恢复摘录而撤销前置计划。

## Status

- Priority: P1
- Effort: L（包含测试的粗估，L 为多天）
- Risk: HIGH
- Depends on: 015-A 的验收契约已记录；不等待 015-B
- Category: direction
- Planned at: commit `2ec48ab`, 2026-11-08
- Status: DONE（隔离工作树）

## Product contract

用户已决定取消注册项目：只管理软件版本、独立数据库实例和 HTTPS 反代。HTTPS 页面仅配置“域名 → 本地端口”规则，不设独立站点实体、详情页或生命周期。应用由用户在终端或 IDE 启动；App 不再管理源码目录、应用命令、依赖安装、编排、项目自启或环境导入导出。Node 按目录读取版本声明仍保留，不要求注册目录。保留现有系统服务管理为次要入口。

不做旧项目 schema、文件、任务或配置的兼容和迁移。开发验收使用新建隔离存储；这不授权清空用户真实数据库、卸载共享 Homebrew keg 或处理旧机器上的遗留进程。软件范围为 Node.js、MySQL、PostgreSQL、Redis、MongoDB 五类；PostgreSQL=Homebrew core，其余四类校验归档。删除 PHP 安装、版本选择、仓库添加和终端集成，不操作机器已有 PHP；macOS arm64、Go/MyGo 技术栈不变。界面主入口为 Software / Databases / HTTPS。尚未发布，任何新状态格式不需要迁移器，但各计划必须维持同一新格式并集成新增字段。

## Current state

以下为旧代码基线摘录，不是保留 PHP 或站点管理的目标要求。

`internal/devplatform/model.go:147`：

```go
type Project struct {
	RuntimeHints      []RuntimeHint `json:"runtimeHints,omitempty"`
	DetectionWarnings []string      `json:"detectionWarnings,omitempty"`
	Autostart         bool          `json:"autostart,omitempty"`
	SchemaVersion     int           `json:"schemaVersion"`
	ID                string        `json:"id"`
	Name              string        `json:"name"`
	Directory         string        `json:"directory"`
	Services          []Service     `json:"services"`
	Instances         []string      `json:"instances,omitempty"`
	Sites             []Site        `json:"sites,omitempty"`
}
```

`internal/devplatform/model.go:193`：

```go
type State struct {
	PackageIntegrity    map[string]string   `json:"packageIntegrity,omitempty"`
	LogRotationDisabled bool                `json:"logRotationDisabled,omitempty"`
	NodeDefault         string              `json:"nodeDefault,omitempty"`
	SchemaVersion       int                 `json:"schemaVersion"`
	Packages            map[string]Package  `json:"packages"`
	Projects            map[string]Project  `json:"projects"`
	Instances           map[string]Instance `json:"instances"`
	Sites               map[string]Site     `json:"sites"`
}

func emptyState() State {
	return State{SchemaVersion: Schema, Packages: map[string]Package{}, Projects: map[string]Project{}, Instances: map[string]Instance{}, Sites: map[string]Site{}}
```

`internal/devcli/cli.go:21`：

```go
var internalCommands = map[string]bool{
	"serve":           true, // the HTTPS proxy
	"maintain":        true, // log rotation
	"up":              true, // a project that starts at login
	"command-worker":  true, // project commands that outlive the window
	"database-worker": true, // database work that outlives the window
	"node":            true, // the Node.js version for a directory, for the shell setup
}
```

`internal/devplatform/engine.go:73`：

```go
	// Reuse the existing plist validation and atomic job writer.
	spec := launchd.Spec{Label: m.label(id), Args: args, WorkingDir: dir, Env: env, StdOut: filepath.Join(m.Root, "logs", id+".log"), StdErr: filepath.Join(m.Root, "logs", id+".err.log"), RunAtLoad: true, KeepAlive: restart}
	if throttle > 0 {
		spec.ThrottleInterval = &throttle
	}
	path, e := launchd.Save(filepath.Join(m.Root, "jobs"), spec, existing)
```

PHP 删除依据：基线 `internal/devplatform/catalog.go:58` 将 PHP/PostgreSQL 一起交给 Homebrew；`view_development.go:658` 软件菜单仍含 PHP。它们需要在本计划中同步收缩：

```go
case "php", "postgresql":
    return resolveHomebrew(ctx, name, version, runCommand)
```

`model.go` 的 Package.Validate 也接受 PHP Homebrew 包，因此只隐藏菜单不能完成删除。保留 PostgreSQL 共享实现，拒绝 PHP 包入口。

## Conventions

- 领域行为在 `internal/devplatform`，GUI 在根目录 `view*.go`。主线程修改 UI 状态，后台工作以 `a.post` 回传；沿用现有任务取消、按实例/包的租约和短状态事务，不新增通用任务框架。
- 持久化沿用 `model.go` 的 `withState` / `withStateTransaction` 与原子写入，私有配置权限 0600。现有范例：

```go
return m.withState(ctx, func(s *State) error {
    s.Sites[site.ID] = site
    if e := validateSites(allSites(s)); e != nil {
        return e
    }
    return nil
})
```

摘录来自基线 `internal/devplatform/proxy.go:90`，仅示范事务模式；其中 Site/Sites 由 011 删除。新操作应同样先验证再提交，不绕过状态锁。

- 界面沿用原生 MyGo 控件、稳定 accessibility 名称、900×520 与 1120×720 两种尺寸及明暗主题。`docs/development-ui.md:10` 原规则：“按钮和字段的可访问名称保持稳定”。状态相关警告可见，解释放到相关操作附近。保留草稿；不用已取消的 Project 类型实现草稿保护。
- 测试结构参考 `development_e2e_test.go` 的 `devUIHarness`：`t.TempDir()` 根、注入 `Manager.Home/Program/Run`、捕获 clipboard/open、禁止自动管理员调用。它的默认 Run 会执行真实命令：离线新增用例必须明确替换 Run，不能误认为整个 harness 都是 mock。后端参考 `platform_test.go` 的 testManager。

## Commands you will need

以下为仓库已有命令；本轮只核对定义，未运行构建或测试。后文标“新增”的测试名是要实现的验收契约，当前不存在。先 `go test -list '^测试名$' <包>` 确认列出该名称，再运行；`[no tests to run]`、SKIP 都不算通过。

| 用途 | 命令 | 成功条件 |
|---|---|---|
| 编译所有测试 | `go test ./... -run '^$'` | exit 0，仅编译，不代表功能已验证 |
| 静态检查 | `go vet ./...` | exit 0 |
| 完整回归 | `go test ./...` | 隔离 macOS 会话中 exit 0，逐项记录 SKIP |
| 竞态检查 | `go test -race ./...` | 隔离 macOS 会话中 exit 0，无竞态 |
| Python 工具测试 | `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'` | exit 0 |
| 本机构建 | `go tool mygo build -sign - -skip-notarize` | exit 0，产物在 build/；不发布 |
| 补丁检查 | `git diff --check` | exit 0 |

完整 Go 测试存在真实 launchd/数据库用例，先运行本计划的无特权聚焦测试。原生软件门槛仅在一次性 GitHub-hosted macOS runner 用 `PYTHONDONTWRITEBYTECODE=1 python3 scripts/verify-native-release.py --run`，必须 required 用例全部 PASS、零 SKIP。不要在日常机器借验证顺便安装/替换软件。系统信任和 Chrome 的实机验收在 015 单独记账；私有测试 CA 不等于系统信任通过。

## Scope

只可修改下列路径；通配符是明确允许范围，不是整仓库授权。允许为前置计划已完成的等价拆分文件修订本节后使用，但必须先记录新路径及原因。

- `internal/devplatform/*.go`
- `internal/devcli/*.go`
- `main.go`
- `app.go`
- `menubar.go`
- `view*.go`
- `*_test.go`
- `scripts/prepare-native-ui-verification.py`
- `scripts/test_prepare_native_ui_verification.py`
- `scripts/verify-development.py`
- `scripts/verify-native-release.py`
- `scripts/test_verify_native_release.py`

每份计划允许更新自身的 Execution record 和 `plans/README.md` 自己的状态行。新文件只可在列出的命名范围内创建。不要修改其他计划的范围/状态。

## Git workflow

在执行者的隔离 checkout/worktree 实施，分支名 `codex/011-remove-project-model`；开始记下执行基线与工作区状态，不覆盖用户修改。提交风格沿用仓库 `feat:` / `refactor:` / `test:` / `docs:`。按逻辑单元交付可审查 diff；是否提交由执行任务授权决定，不自动合并、推送、创建 PR 或发布。

## Why this matters

只删 Projects 页面会留下后台执行、资源所有权、软件使用关系和退出流程。本计划一次完成领域删除及所有直接调用者的最小适配，交付可编译、可验证的独立资源基础。后续 012–014 增强体验，不承担修复本计划遗留编译错误的职责。

## Target and boundaries

1. 新 State 保留 Packages、Instances、ProxyRules、ProxyListenerPort、ProxyRulesRevision、NodeDefault、完整性与日志配置；移除 Project、项目 Service、Site/Sites 及所有项目关联字段。设新的状态 schema（建议 2）；旧 schema 明确报不支持，不自动转换、覆盖或清空。后续连接配置字段均在该新 schema 上增加，尚未发布阶段不制造逐版本迁移链。
2. 取消 `Import/SaveProject/Up/Down/RestartProject/Exec/ExportProject/PrepareEnvironment`、项目目录重定位、项目锁、项目 worker、项目登录 launcher、检测建议与项目图形编辑器。移除 `devmanager.json` / lock 的生产读写路径。
3. 保留数据库的初始化/备份/恢复/删除恢复、软件任务、代理、证书、域名配置、Node 目录版本解析、通用日志维护。`Service` 的健康检查结构如被数据库使用，提取为仅含进程/检查字段的内部探针，不能保留一个空壳项目服务模型。
4. 删除 `sites.go` 的单站点 CRUD 与 Site 类型，改为 `proxy_rules.go` 的整体规则集 API `ReplaceProxyRules(ctx, expectedRevision, listenerPort, rules)`。规则仅 Domain、LocalPort；将 listenerPort 与 rules 同时验证、在一个事务提交，并且只递增一次 revision；任何字段校验/持久化失败都不提交两者。规范化域名为唯一键，目标固定 `http://127.0.0.1:<LocalPort>`。没有站点 ID/name/owner/目录/命令/独立启停或 disabled 字段。HTTPS listener port 是全局代理设置（默认 443，测试可用空闲高端口）。新状态存 ProxyRules、ProxyListenerPort 与 ProxyRulesRevision，发布到 `proxy/rules.json`，记录格式固定为 `{schemaVersion: 1, listenerPort: N, rules: [{domain, localPort}]}`；rules 必须是数组，空集为 `[]`，拒绝未知字段。State revision 不写入用于等价比较的发布内容。不读写旧 sites.json。014 在此基础上完成单页规则表与整体保存/应用。
5. 本计划对 shutdown/菜单栏只做删除项目后的有效适配；保留实例停止与代理独立选择。013 增加完整菜单栏资源表现和新的专门回归。
6. 015-A 定义的新文档为产品目标；不沿用旧项目优先文档推导需求。删除 PHP catalog/解析/安装/修复/卸载注册入口与 AddPHPRepository，保留 PostgreSQL 的共享 Homebrew 逻辑。主界面固定五类；不顺带移除 Go/Valkey 底层实现。显式 manifest 也拒绝以 php/php-fpm 身份绕过删除；不扫描、卸载或迁移本机已有 PHP。

**Out of scope**：`internal/launchd`、`internal/service`、`internal/brew`、`internal/loginitem` 的业务语义；软件来源与许可、依赖版本、真实用户数据；Node 终端体验、HTTP 检查和连接配置的新功能。不得整文件删除仍承载共享 helper 的文件后复制未知实现。

## Steps

### Step 1: 建立删除与保留清单

在本文件记录搜索结果分类：独占项目文件、混合文件、共享 helper、旧测试、原生工具调用者。重点核对 `project_*.go`、`environment_lock.go`、`shared_environment.go`、`export_recovery.go`、`command_work.go`。`operations.go` 使用 `syncExportDirectory` 等导出文件内 helper，必须将仍需的通用文件同步/安全读取实现移动到 `storage.go`，保留测试语义。Node 的 `exactRuntimeVersion` / 版本文件读取也不能因删除检测器而丢失。

**Verify**：`rg -n 'Project|projectResourceKeys|commandWork|syncExportDirectory|readExportFile|exactRuntimeVersion' internal/devplatform internal/devcli view*.go menubar.go` → 命中的生产符号全部在清单中有删除或保留去向；退出 0 是盘点完成，不是删除已完成。`python3 -c "from pathlib import Path; p=Path('docs/resources-acceptance.md'); assert p.is_file() and 'AC-CORE' in p.read_text()"` → exit 0，015-A 已先定义契约。

### Step 2: 一次完成核心删除及编译调用者适配

先改新 State/读取契约，再删项目专属 API、内部入口 `up` 和 `command-worker` 及测试 harness 对应命令。保留 `serve`、`maintain`、`database-worker`、`node`。处理 `engine.go` 的共享 job writer、health/日志 helper 和数据库 worker；删除项目任务种类和重试分支。删除项目 references 检查时保留实例使用、包租约、job 归属、TCP 占用、删除指纹和未结束任务检查。将原 Site/Sites 与 NeedsReview 删除，统一使用 `State.ProxyRules`；不要把任意用户文本变成任务命令。

同步调整代理发布、hosts/证书、诊断、任务锁与 CLI 工具，全部从规则集生成路由，不能保留 Site 适配层。`serve` 读取 rules.json，保留 last-good、Host 校验、WebSocket/SSE/上传和超时处理；规则目标仅 HTTP loopback。旧的任意 HTTPS upstream 产品输入测试改为拒绝输入，将 `TestProxyHeaderTimeoutAndVerifiedHTTPSUpstream` 改为 `TestProxyHeaderTimeoutAndLocalPortRules`：保留原超时断言，增加 HTTPS URL/任意 upstream 输入拒绝用例，不再要求产品连接 HTTPS upstream；其它仍适用的底层 TLS 校验测试保留。

GUI 同步缩为三个入口，用有名称的 tab 常量替换散落的 0/1/2/3 映射，默认 Software；清除 project draft/binding/callback、项目任务菜单与结果。数据库草稿与整份 HTTPS 规则表草稿分别保存；现有系统服务 inspector/editor 保持原行为。`main.go` 的关闭窗口继续驻留和系统服务功能保持。

同步删除 `catalog.go` 的 PHP Resolve/AvailableVersions 分支、model/packages 校验的 PHP 接受路径、homebrew.go 的 PHP tap/formula/keg 分支及 UI 的仓库按钮。保留 PostgreSQL 精确 keg、来源/receipt 校验和不碰共享安装的规则。修改 `TestResourceOnlyCore`：PHP resolve、显式 manifest 和安装入口被拒绝且 runner 零调用；五类软件仍可列出。旧 PHP 专属测试改成拒绝测试，共享 Homebrew 测试改用 PostgreSQL fixture，不删除共有防护断言。Node 与数据库无需为此改来源。

**Verify**：`go test ./... -run '^$'`、`go vet ./...` → exit 0。新增 `TestResourceOnlyCore` 在 `internal/devplatform/resource_model_test.go`，运行 `go test ./internal/devplatform -run '^TestResourceOnlyCore$' -count=1 -v` → 新格式 round-trip、旧格式拒绝且字节未变、软件、实例和整份规则集独立保存通过。

### Step 3: 迁移保留能力的测试夹具，不保留旧产品接口

可删除只验证项目导入/编排/导出的用例；混合用例必须用独立实例与反代规则重写。将旧 `TestUISoftwareCatalogAllSix` 改为 `TestUISoftwareCatalogAllFive`，required 子项精确为 node/mysql/postgresql/redis/mongodb；仅删除 PHP 场景，保留其余五类真实检查。将 `TestUIRuntimeDeclarationsReviewInstallAndHTTPS` 重写并重命名为 `TestUIStandaloneRuntimeAndHTTPS`：测试拥有外部 Node 子进程，App 仅安装软件和反代。将项目草稿相关初始化用例改为 `TestUIDatabaseInitializationKeepsIndependentDrafts`。在 `verify-native-release.py` 和其 parser 测试同一修改中替换三个 required 名、五软件 MATRIX/VERSIONS；删除 PHP brew tap/preflight/receipt/准备步骤，仅保留 PostgreSQL Homebrew 路径。prepare/verify 脚本不得再安装或运行 PHP；不减少其余五类行为要求。

更新 prepare 脚本与 verify-development 的命令调用：不再调用项目 CRUD/up/exec；外部应用由测试 harness 以精确 binary 启动，记录只属测试的 PID/根并 finally 清理。其余脚本不得通过重新开放 shipped CLI 解决。为脚本新外部进程创建失败、取消及清理写无真实进程副作用的 Python 测试。

**Verify**：`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'` → exit 0；`go test -list '^(TestUIStandaloneRuntimeAndHTTPS|TestUIDatabaseInitializationKeepsIndependentDrafts|TestUISoftwareCatalogAllFive)$' .` → 三个名称都存在。实际原生执行列入 015-B，不将 list 或跳过当作通过。

### Step 4: 验证没有项目运行路径且保留资源独立工作

在 `resource_ui_test.go` 新建 `TestUIResourceOnlyNavigation`，在 `internal/devcli/resource_only_test.go` 新建 `TestInternalResourceOnlyCommands`。前者用纯 mock Run 检查三个入口、无 PHP/独立站点/导入/项目菜单、单一 HTTPS 配置页及数据库选择、切页不丢草稿；后者保证旧项目命令拒绝且剩余内部命令正常分派。系统服务回归用既有 mock tester。

**Verify**：`go test -race ./... -run '^(TestResourceOnlyCore|TestUIResourceOnlyNavigation|TestInternalResourceOnlyCommands|TestAllItemsListsEverySource|TestMenubarPanelTogglesServices)$' -count=1 -v` → 所有列出的新/旧测试实际 PASS，无 SKIP。`git diff --check` → exit 0。

## Test plan

新增上述三个顶层 suite；使用子用例覆盖新状态为空/三集合/不完整/旧 schema、规范化域名唯一且仅接受本地端口、同时改 listener port/规则成功一致提交或失败均不提交、数据库删除仍拒绝运行实例、卸载仍拒绝实例引用、Node 不注册目录也能选版本、取消不写状态。根 UI 使用 mock 后端，不执行真实 launchd。对旧测试建立“删除原因 / 保留行为 / 新测试名”映射，放在本计划 Execution record。

## Done criteria

- [ ] 上述编译、vet、聚焦 race、Python 命令通过，新增测试名称实际存在且执行。
- [ ] `rg -n 'type Project struct|map\[string\]Project|\.Projects\b|projectResourceKeys|RunProjectCommandWorker|runtimeTargetProject|type Site struct|map\[string\]Site|\.Sites\b|SaveSite|EditSite|RemoveSite' --glob '*.go' --glob '!**/*_test.go' .` → 无匹配（rg exit 1）；不能用全局字符串 project 禁令误删 Node 的目录语义。
- [ ] `rg -n 'devmanager\.json|devmanager\.lock\.json|command-worker|Import project|Start project' --glob '*.go' --glob '!**/*_test.go' .` → 无匹配。旧入口拒绝的负例仅在测试。
- [ ] PHP catalog/安装/仓库按钮已删除，PHP resolve/manifest/install 拒绝测试 PASS；不得执行 brew uninstall/untap 或改动本机 PHP。
- [ ] required 原生清单仍覆盖五类软件、独立运行时+HTTPS、数据库初始化，未降级跳过规则。
- [ ] 所有改动在 Scope 内；记录新状态 schema、规则集 API、tab 常量、共享 helper 新位置供下游使用。

## Maintenance notes

最容易误删的是项目文件中的共享存储/任务 helper，以及数据库操作的独立保护。审查必须看保留能力的替代测试，不能只看大量删除行数。011 完成后，014 不应再创建第二次模型迁移；015-B 验收不允许重新引入项目夹具。

## Non-vacuous verification gate

以下是 Done 的必需补充门槛。`plans/check-test-events.py` 已随计划提供，只读取 stdin 的 Go JSON 事件；拒绝缺失完整测试名、SKIP、FAIL、重复或不完整包事件。不要只凭 exit 0 或顶层 -list 判断子用例通过。先执行 `set -o pipefail`，确保管道前后任一失败都失败。

```sh
set -o pipefail
go test -json -race ./internal/devplatform -run '^TestResourceOnlyCore$' -count=1 | python3 plans/check-test-events.py --package macos-service-manager/internal/devplatform --require TestResourceOnlyCore
go test -json -race . -run '^TestUIResourceOnlyNavigation$' -count=1 | python3 plans/check-test-events.py --package macos-service-manager --require TestUIResourceOnlyNavigation
go test -json -race ./internal/devcli -run '^TestInternalResourceOnlyCommands$' -count=1 | python3 plans/check-test-events.py --package macos-service-manager/internal/devcli --require TestInternalResourceOnlyCommands
```

预期每条命令 exit 0 并输出 `PASS: … required test results verified`。

## STOP conditions

- 非预期代码漂移、前置契约不成立，或需要修改 Scope 外路径：先报告具体差异并修订计划，不能自行扩范围。
- 同一验证失败两次合理修复后仍未通过：留下命令和失败原因，不以删除断言、增加 SKIP 或降低 required 清单通过。
- 新能力要求兼容旧项目、迁移真实数据、接管外部应用进程或加入项目替代实体：停止；这些与批准方向冲突。
- 准备环境需要替换本机 Homebrew 安装、操作真实 hosts/钥匙串、注销/重启：不把这些当成普通测试前置；使用隔离测试并单列实机未验证项。

## Execution record

执行者填写：实际起点 SHA、前置计划契约核对、文件差异、运行命令、测试名/子用例 PASS 与 SKIP、原生环境、截图路径、未验证边界。未执行不写 PASS；只有全部 Done criteria 达成才标 DONE。

011 执行记录：隔离工作树 `/Users/jyeu/code/service-manager-011`，最终提交 `f987c0c`（父链包含 015-A `ad9f773`）。Go 全量/race、vet、Python 31 项、MyGo ad-hoc 构建、新增聚焦测试与编译通过；原生 runner 完整 `--run` 尚未执行，列入 015-B。主工作树未合并；最终差异为 126 文件，新增 1,636 行、删除 12,732 行。

集成记录：按用户“合并 master”的明确指示，实现提交链已于本轮快进合入 master（`96549ea`）。此前工作树记录保留为历史；未验证的原生安装、外部卷与系统信任项目仍保持原状态。
