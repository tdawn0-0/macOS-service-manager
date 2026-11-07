# Plan 017: 为独立实例提供数据库和用户连接配置

> 执行者：本文件是独立交接计划。按步骤实施并验证；仅更新自己的状态记录。先读完 Scope 与 STOP conditions。计划作者本轮没有实施源码。
>
> 漂移检查：`git diff --stat 2ec48ab..HEAD -- 'internal/devplatform/model.go' 'internal/devplatform/connections.go' 'internal/devplatform/connections_test.go' 'internal/devplatform/connection_profiles.go' 'internal/devplatform/connection_profiles_test.go' 'internal/devplatform/database.go' 'internal/devplatform/database_port_change.go' 'internal/devplatform/instance_deletion.go' 'internal/devplatform/instance_deletion_test.go' 'view_development.go' 'view_database_connections.go' 'view_database_tasks.go' 'connection_profiles_ui_test.go'`，另运行 `git status --short`。本文件摘录描述规划基线。前置计划造成的预期变化须逐项记录在本文件“Execution record”，对照本文明确的前置契约继续；其他无法解释的漂移停止报告。不要为恢复摘录而撤销前置计划。

## Status

- Priority: P2
- Effort: M–L（包含测试的粗估，L 为多天）
- Risk: MED
- Depends on: 011、013 DONE；015-A 已定义 AC-CONNECTION
- Category: direction
- Planned at: commit `2ec48ab`, 2026-11-08
- Status: DONE（隔离工作树）

## Product contract

用户已决定取消注册项目：只管理软件版本、独立数据库实例和 HTTPS 反代。HTTPS 页面仅配置“域名 → 本地端口”规则，不设独立站点实体、详情页或生命周期。应用由用户在终端或 IDE 启动；App 不再管理源码目录、应用命令、依赖安装、编排、项目自启或环境导入导出。Node 按目录读取版本声明仍保留，不要求注册目录。保留现有系统服务管理为次要入口。

不做旧项目 schema、文件、任务或配置的兼容和迁移。开发验收使用新建隔离存储；这不授权清空用户真实数据库、卸载共享 Homebrew keg 或处理旧机器上的遗留进程。软件范围为 Node.js、MySQL、PostgreSQL、Redis、MongoDB 五类；PostgreSQL=Homebrew core，其余四类校验归档。删除 PHP 安装、版本选择、仓库添加和终端集成，不操作机器已有 PHP；macOS arm64、Go/MyGo 技术栈不变。界面主入口为 Software / Databases / HTTPS。尚未发布，任何新状态格式不需要迁移器，但各计划必须维持同一新格式并集成新增字段。

## Current state

以下为旧代码基线摘录，不是保留 PHP 或站点管理的目标要求。

`internal/devplatform/connections.go:35`：

```go
	scheme, user, path := "", "", "/"
	query := url.Values{}
	switch i.Kind {
	case "postgresql":
		scheme, user, path = "postgresql", "devmanager", "/postgres"
	case "mysql":
		scheme, user = "mysql", "root"
	case "mongodb":
		scheme, user, path = "mongodb", "devmanager", "/admin"
		query.Set("authSource", "admin")
		query.Set("directConnection", "true")
	case "redis", "valkey":
		scheme, user, path = "redis", "default", "/0"
	default:
		return ConnectionInfo{}, errors.New("unsupported database engine")
	}
	uri := url.URL{Scheme: scheme, User: url.User(user), Host: net.JoinHostPort("127.0.0.1", strconv.Itoa(i.Port)), Path: path, RawQuery: query.Encode()}
	return ConnectionInfo{URI: uri.String(), Username: user, CredentialFile: filepath.Join(m.Root, "instances", i.ID, "password")}, nil
```

`view_development.go:1383`：

```go
		devField(c, "Database name", &d.dbName, "app")
	}
	if d.dbKind != "redis" {
		if ui.Button(c, "Create database").AlignSelf(ui.Start).Disabled(d.busy || a.databaseTaskRunning(d.dbID)).Clicked() {
			id, name := d.dbID, d.dbName
			a.startDatabaseOperation(d.state.Instances[id], "create database", func(m *devplatform.Manager) (string, error) {
				return "Created database " + name, m.CreateDatabase(m.Context, id, name)
			})
		}
		devField(c, "Database user", &d.dbUser, "app_user")
		ui.Field(c, "New user password", func() { ui.TextInput(c, &d.dbPassword).Password() })
		if ui.Button(c, "Create database user").AlignSelf(ui.Start).Disabled(d.busy || a.databaseTaskRunning(d.dbID)).Clicked() {
			id, name, password, db := d.dbID, d.dbUser, d.dbPassword, d.dbName
			d.dbPassword = ""
			a.startDatabaseOperation(d.state.Instances[id], "create user", func(m *devplatform.Manager) (string, error) {
				return "Created user " + name, m.CreateUser(m.Context, id, name, password, db)
```

`internal/devplatform/database_other.go:112`：

```go
	case "mongodb":
		client, e := m.mongoClient(ctx, i, true)
		if e != nil {
			return e
		}
		defer client.Disconnect(context.Background())
		return client.Database(database).RunCommand(ctx, bson.D{{Key: "createUser", Value: name}, {Key: "pwd", Value: password}, {Key: "roles", Value: bson.A{"readWrite"}}}).Err()
```

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

- `internal/devplatform/model.go`
- `internal/devplatform/connections.go`
- `internal/devplatform/connections_test.go`
- `internal/devplatform/connection_profiles.go`
- `internal/devplatform/connection_profiles_test.go`
- `internal/devplatform/database.go`
- `internal/devplatform/database_port_change.go`
- `internal/devplatform/instance_deletion.go`
- `internal/devplatform/instance_deletion_test.go`
- `view_development.go`
- `view_database_connections.go`
- `view_database_tasks.go`
- `connection_profiles_ui_test.go`

每份计划允许更新自身的 Execution record 和 `plans/README.md` 自己的状态行。新文件只可在列出的命名范围内创建。不要修改其他计划的范围/状态。

## Git workflow

在执行者的隔离 checkout/worktree 实施，分支名 `codex/017-database-connection-profiles`；开始记下执行基线与工作区状态，不覆盖用户修改。提交风格沿用仓库 `feat:` / `refactor:` / `test:` / `docs:`。按逻辑单元交付可审查 diff；是否提交由执行任务授权决定，不自动合并、推送、创建 PR 或发布。

## Why this matters

用户创建业务数据库/账号后，现有 Copy/Open 仍返回实例管理员默认目标。本计划增加实例拥有的连接配置，让选择与复制结果一致。它不保存业务密码、不注入应用环境，也不创建项目替代实体。

## Target and boundaries

先冻结四引擎连接语义，再交付 Go/UI；不是只做离线原型。

- 新 `ConnectionProfile`：稳定 ID、InstanceID、Name、Database、Username、MongoAuthSource、RedisDB（仅相应引擎有效）。无 Password、CredentialFile、DSN raw text、任意 URI/query 字段。State 新增独立 map，旧项目 schema 仍不支持；在 011 新格式上按当前未发布字段自然增加，不新增迁移器。
- 存储归属实例，Host/Port/Kind 每次从当前实例解析，不复制缓存 endpoint。外部改端口后新 Copy 即使用新端口；实例移除/永久删除在同一状态事务清除其 profile。只删 profile 不触碰数据库/用户/数据文件。
- PostgreSQL/mysql：用户明确输入用户名与数据库；URL 使用 `net/url` 正确转义，不拼接任意查询。MongoDB 允许明确 authSource；App 在业务库里创建的账号，其默认 authSource 为该业务库，管理员预设仍为 admin。Redis 仅 0–15 和当前支持的 default 用户，不增加 ACL 管理。
- 管理员默认连接保留为单独的内置只读选项，不存成普通 profile。其 Copy admin password 继续显式操作。选择业务 profile 时不显示/不调用该 admin 密码复制按钮；GUI 提示客户端单独输入账号密码。
- Copy URI 与 Open client 使用完全同一生成函数和选中 profile；URI 永不含 password。Open 仍走系统默认 handler 的现有约定；没有默认客户端时显示可复制 URI，不自动安装客户端。
- 保存连接配置只保存元数据，不保证账号/数据库实际存在或有权限。创建数据库/用户成功后可显式“保存为连接配置”，不自动保存密码、不默默替换当前配置。取消/失败保持原 profile 和草稿。

**Out of scope**：密码管理器/钥匙串、自动读取客户端凭据、连接认证探测、SQL 编辑、数据库/用户枚举、应用环境变量、团队导出、外部数据库接入。

## Steps

### Step 1: 定义 profile 与四引擎 URI 契约

新增 `connection_profiles.go`，用 withState 处理 CRUD，验证 Instance 存在及类型对应字段。名称/用户/库限制长度并拒绝控制字符；不能仅用数据库创建接口的标识符正则代替 URI escaping。使用 typed 参数和有限 query keys：Mongo authSource/directConnection、其余无任意 query。profile ID 与显示名称分离。现有 ConnectionInfo(id) 保持管理员预设语义，新增选中 profile 的 resolver，并共享 engine URI builder。

**Verify**：新增 `TestConnectionProfiles` 于 `connection_profiles_test.go`；`go test ./internal/devplatform -list '^TestConnectionProfiles$'` 列出它；`go test -race ./internal/devplatform -run '^(TestConnectionProfiles|TestConnectionURIsExcludePasswordsAndSelectWorkingDefaults)$' -count=1 -v` → 四引擎、转义、Redis 0/15/越界、Mongo admin/业务库、类型字段不匹配均 PASS，URI parser 显示 Password 不存在。

### Step 2: 处理实例生命周期和状态一致性

端口变化无需改 profile 字段，resolver 从当前实例取值。移除记录和永久数据删除成功时在对应 state transaction 删除 profile；若删除在提交前取消/失败则 profile 保留，已提交后的 crash recovery 与实例状态一致。不得为了清 profile 而更改数据删除的 staging/指纹保护。缺失实例的 profile 不得生成 URI，给出 unavailable。

**Verify**：`go test -race ./internal/devplatform -run '^TestConnectionProfiles/lifecycle$' -count=1 -v` → port change、remove、delete before/after commit、cancel、stale instance 子用例 PASS；对删除动作使用临时目录和 mock job，仅 profile 删除绝不产生数据库命令。

### Step 3: 接入实例详情的连接选择器和编辑器

新增 `view_database_connections.go`：内置 Administrator connection 与实例自己的 profile 列表；Add/Edit/Delete profile 独立于实际 Create database/user。输入框按引擎显示；Mongo authSource 必须可见。Copy/Open preview 使用选中 profile；业务 profile 不可调用 DatabasePassword。创建业务库/用户后提供显式预填 profile 动作。后台成功结果携带发起时不可变的 InstanceID、Database、Username、MongoAuthSource 和 operation ID，只使用这些字段，绝不从完成时当前表单取值。单独建库时 Username 保持未选，不能猜账号。预填动作触发时重新验证原实例仍存在且 kind 匹配，并通过当前草稿保护；不自动导航或覆盖当前实例草稿。清空已有 password 输入并保证不进入成功结果/profile 状态。

新增 `TestUIConnectionProfiles` 于 `connection_profiles_ui_test.go`，mock clipboard/open/Run，验证切换实例不沿用前一个 profile；后台创建期间切实例/改字段、旧任务晚完成都不能污染当前表单，失败/取消不产生成功预填入口，原实例删除使预填不可用；草稿取消保留；管理员/业务密码操作边界；Delete profile 与 Delete data 明确不同。

**Verify**：`go test -race . -run '^TestUIConnectionProfiles$' -count=1 -v` → PASS，业务 profile 的 clipboard/open 捕获不含任何密码，Run 没有建库/删库动作。

### Step 4: 验证布局、持久化和真实客户端边界

UI suite 的 `layouts` 子用例覆盖长名字、用户名/库转义、Mongo额外字段、Redis编号，四种尺寸/主题输出 `MSM_RESOURCE_SHOTS`。保存/重启 App 后选项恢复，管理员默认仍可用；缺失 profile 不按同名模糊匹配。普通应用仅承诺给出正确 URI，不宣称客户端已认证成功；015-B 用隔离账号/数据库验证 URI 与实际创建语义相符，密码另传给测试客户端。

**Verify**：`MSM_RESOURCE_SHOTS=build/resource-shots/017 go test . -run '^TestUIConnectionProfiles/layouts$' -count=1` → 四种布局截图与 hit-test 通过；`go test -race ./internal/devplatform -run '^TestConnectionProfiles$' -count=1 -v` → metadata round-trip 与 secret sentinel 不落盘；`go test ./... -run '^$'`、`go vet ./...`、`git diff --check` → exit 0。

## Test plan

按 `connections_test.go` 的解析后断言风格验证字段，不用完整字符串快照替代 URI 语义。新增 profile suite 下含 `uri`、`validation`、`lifecycle`、`persistence`；UI suite 下含 `selection`、`credentials`、`drafts`、`layouts`。为 Mongo 业务账号设置 authSource=创建账号的数据库，不能从当前 admin URI 直接换 path 后保留 admin。

## Done criteria

- [ ] 新两个 suite 与既有默认连接测试实际执行，零 SKIP，race PASS。
- [ ] 管理员预设与业务 profile 明确分开；业务 profile 不能复制 admin 密码。
- [ ] metadata round-trip 无密码；endpoint 始终来自实时实例；删除 profile 不操作数据库。
- [ ] 实例移除/删除的 profile 清理与 state transaction 一致，错误与取消保留正确状态。
- [ ] 四种截图已人工检查；编译/vet/diff 均 exit 0；真实认证未验证部分移交 015-B 明确记账。

## Maintenance notes

将来支持保存业务密码需要独立凭据存储方案，不能在此 struct 加 Password。连接配置不反向管理数据库账号；外部删除账号后配置仍只是地址元数据。新增引擎选项必须更新有限字段验证与 URI 测试。

## Non-vacuous verification gate

以下是 Done 的必需补充门槛。`plans/check-test-events.py` 已随计划提供，只读取 stdin 的 Go JSON 事件；拒绝缺失完整测试名、SKIP、FAIL、重复或不完整包事件。不要只凭 exit 0 或顶层 -list 判断子用例通过。先执行 `set -o pipefail`，确保管道前后任一失败都失败。

```sh
set -o pipefail
go test -json -race ./internal/devplatform -run '^TestConnectionProfiles$' -count=1 | python3 plans/check-test-events.py --package macos-service-manager/internal/devplatform --require TestConnectionProfiles --require TestConnectionProfiles/uri --require TestConnectionProfiles/validation --require TestConnectionProfiles/lifecycle --require TestConnectionProfiles/persistence
MSM_RESOURCE_SHOTS=build/resource-shots/017 go test -json -race . -run '^TestUIConnectionProfiles$' -count=1 | python3 plans/check-test-events.py --package macos-service-manager --require TestUIConnectionProfiles --require TestUIConnectionProfiles/layouts --shot-dir build/resource-shots/017
```

预期每条命令 exit 0 并输出 `PASS: … required test results verified`。
layouts 必须于本次运行生成精确四个 PNG：`light-900x520.png`、`dark-900x520.png`、`light-1120x720.png`、`dark-1120x720.png`，置于上述目录，1×像素无额外阴影边框。校验器核对存在、PNG尺寸和修改时间不早于包 start；仍需人工查看布局。不得复用旧图充当本次产物。

## STOP conditions

- 非预期代码漂移、前置契约不成立，或需要修改 Scope 外路径：先报告具体差异并修订计划，不能自行扩范围。
- 同一验证失败两次合理修复后仍未通过：留下命令和失败原因，不以删除断言、增加 SKIP 或降低 required 清单通过。
- 新能力要求兼容旧项目、迁移真实数据、接管外部应用进程或加入项目替代实体：停止；这些与批准方向冲突。
- 准备环境需要替换本机 Homebrew 安装、操作真实 hosts/钥匙串、注销/重启：不把这些当成普通测试前置；使用隔离测试并单列实机未验证项。

## Execution record

执行者填写：实际起点 SHA、前置计划契约核对、文件差异、运行命令、测试名/子用例 PASS 与 SKIP、原生环境、截图路径、未验证边界。未执行不写 PASS；只有全部 Done criteria 达成才标 DONE。

017 执行记录：隔离工作树 `/Users/jyeu/code/service-manager-017`，提交 `d76d545`（基于 016 `8991776`）。九个 Scope 文件；Go 全量/race、vet、聚焦 race 与四张截图门槛 PASS。末尾仅布局测试扩展后重跑聚焦 race 门槛；原生实机 NOT RUN，留给 015-B。

集成记录：按用户“合并 master”的明确指示，实现提交链已于本轮快进合入 master（`96549ea`）。此前工作树记录保留为历史；未验证的原生安装、外部卷与系统信任项目仍保持原状态。
