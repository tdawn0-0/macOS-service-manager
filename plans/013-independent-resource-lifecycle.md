# Plan 013: 以数据库实例和 HTTPS 代理为单位控制后台生命周期

> 执行者：本文件是独立交接计划。按步骤实施并验证；仅更新自己的状态记录。先读完 Scope 与 STOP conditions。计划作者本轮没有实施源码。
>
> 漂移检查：`git diff --stat 2ec48ab..HEAD -- 'menubar.go' 'view_quit.go' 'view_development.go' 'view_database_tasks.go' 'view_package_tasks.go' 'view_status.go' 'view_diagnostics.go' 'view_resource_lifecycle.go' 'internal/devplatform/shutdown.go' 'internal/devplatform/shutdown_test.go' 'internal/devplatform/diagnostics.go' 'internal/devplatform/database.go' 'internal/devplatform/database_port_change.go' 'internal/devplatform/instance_deletion.go' 'internal/devplatform/resource_locks.go' 'internal/devplatform/resource_lifecycle_test.go' 'resource_lifecycle_ui_test.go' 'quit_e2e_test.go' 'main_test.go'`，另运行 `git status --short`。本文件摘录描述规划基线。前置计划造成的预期变化须逐项记录在本文件“Execution record”，对照本文明确的前置契约继续；其他无法解释的漂移停止报告。不要为恢复摘录而撤销前置计划。

## Status

- Priority: P1
- Effort: L（包含测试的粗估，L 为多天）
- Risk: HIGH
- Depends on: 011 DONE；015-A 已定义 AC-LIFECYCLE
- Category: direction
- Planned at: commit `2ec48ab`, 2026-11-08
- Status: DONE（隔离工作树）

## Product contract

用户已决定取消注册项目：只管理软件版本、独立数据库实例和 HTTPS 反代。HTTPS 页面仅配置“域名 → 本地端口”规则，不设独立站点实体、详情页或生命周期。应用由用户在终端或 IDE 启动；App 不再管理源码目录、应用命令、依赖安装、编排、项目自启或环境导入导出。Node 按目录读取版本声明仍保留，不要求注册目录。保留现有系统服务管理为次要入口。

不做旧项目 schema、文件、任务或配置的兼容和迁移。开发验收使用新建隔离存储；这不授权清空用户真实数据库、卸载共享 Homebrew keg 或处理旧机器上的遗留进程。软件范围为 Node.js、MySQL、PostgreSQL、Redis、MongoDB 五类；PostgreSQL=Homebrew core，其余四类校验归档。删除 PHP 安装、版本选择、仓库添加和终端集成，不操作机器已有 PHP；macOS arm64、Go/MyGo 技术栈不变。界面主入口为 Software / Databases / HTTPS。尚未发布，任何新状态格式不需要迁移器，但各计划必须维持同一新格式并集成新增字段。

## Current state

以下为旧代码基线摘录，不是保留 PHP 或站点管理的目标要求。

`menubar.go:35`：

```go
// panelItems returns the items of a source in the panel.
func (a *app) panelItems(src service.Source) []service.Item {
	st := a.sources[src]
	if st == nil {
		return nil
	}
	var items []service.Item
	for _, it := range st.items {
		if a.shown(it) && !strings.HasPrefix(it.Name, "xyz.jyeu.msm.dev.") {
			items = append(items, it)
		}
	}
	return items
```

`menubar.go:50`：

```go
// panelRunning counts what runs among the items the panel shows.
func (a *app) panelRunning() int {
	running := 0
	for id := range a.dev.state.Projects {
		if status := a.dev.statuses["project:"+id]; devProjectStarted(status) && status != "Failed" {
			running++
		}
	}
	for _, src := range panelSources {
		for _, it := range a.panelItems(src) {
			if it.Up() {
				running++
			}
		}
	}
	return running
```

`internal/devplatform/shutdown.go:15`：

```go
type ShutdownResource struct{ ID, Name string }
type ShutdownPlan struct {
	Projects        []ShutdownResource
	Instances       []ShutdownResource
	Fingerprint     string
	PrivilegedProxy bool
}
type ShutdownOptions struct{ StopProxy bool }
```

`internal/devplatform/database.go:153`：

```go
	if e = m.checkInstanceAction(ctx, s, id, action); e != nil {
		return e
	}
	if action != "start" {
		m.emit("Stopping database " + id)
		if e = m.stopJob(ctx, "db-"+id); e != nil {
			return e
		}
		if e = m.verifyShutdownTarget(ctx, domain()+"/"+m.label("db-"+id)); e != nil {
			return e
		}
	}
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

- `menubar.go`
- `view_quit.go`
- `view_development.go`
- `view_database_tasks.go`
- `view_package_tasks.go`
- `view_status.go`
- `view_diagnostics.go`
- `view_resource_lifecycle.go`
- `internal/devplatform/shutdown.go`
- `internal/devplatform/shutdown_test.go`
- `internal/devplatform/diagnostics.go`
- `internal/devplatform/database.go`
- `internal/devplatform/database_port_change.go`
- `internal/devplatform/instance_deletion.go`
- `internal/devplatform/resource_locks.go`
- `internal/devplatform/resource_lifecycle_test.go`
- `resource_lifecycle_ui_test.go`
- `quit_e2e_test.go`
- `main_test.go`

每份计划允许更新自身的 Execution record 和 `plans/README.md` 自己的状态行。新文件只可在列出的命名范围内创建。不要修改其他计划的范围/状态。

## Git workflow

在执行者的隔离 checkout/worktree 实施，分支名 `codex/013-independent-resource-lifecycle`；开始记下执行基线与工作区状态，不覆盖用户修改。提交风格沿用仓库 `feat:` / `refactor:` / `test:` / `docs:`。按逻辑单元交付可审查 diff；是否提交由执行任务授权决定，不自动合并、推送、创建 PR 或发布。

## Why this matters

取消项目后，菜单栏需要直接显示自有数据库实例和单个 HTTPS 代理；否则它会隐藏实际运行的开发资源。退出的停止范围也必须与用户看到的资源一致。011 已删除项目依赖，本计划明确日常生命周期行为并补充专门测试。

## Target and boundaries

- 菜单栏分为 Databases、HTTPS、既有系统服务；数据库行使用实际状态与启停/忙碌状态，HTTPS 显示一个代理状态和 Open HTTPS settings 入口，不列出站点或每条规则的菜单项。软件版本不计入 running，规则条数不计为进程数量。既有 provider 行仍按其现有过滤/去重规则显示。
- 关闭窗口继续驻留。Keep services and quit 保留数据库、代理及现有维护行为；Stop managed databases and quit 只停止本 App 自有数据库，另选是否停止代理。保留的代理继续所需日志维护；停止行为和 UI 文案必须一致。取消退出不改变资源；取消已开始停止可能已完成部分动作，刷新并显示实况。
- 不新增登录自启动。删除旧项目 launcher 后，本期数据库默认由用户启动；privileged proxy 的既有 socket activation 持久性要明确显示为系统 listener 特性，不能声称所有资源登录后都停止。
- 数据库 stop/restart/port/delete 仅检查该实例、包、端口、任务和实际 job；所有删除预览/隔离数据恢复机制保留。停止外部应用、Homebrew 默认服务、用户其他 launchd 任务均不在范围内。
- 退出预览 fingerprint 变化后拒绝按旧范围执行；未知停止状态不可当作成功。运行中软件/数据库后台任务继续沿用取消与等待机制。

**Out of scope**：应用命令、资源组、一键开发栈、数据库登录自启、外部进程接管、CA/hosts 修改、代理网络实现、数据格式兼容。

## Steps

### Step 1: 核对 011 后实例与退出契约并补充模型测试

`ShutdownPlan` 不再含 Projects，保留实例身份及代理授权提示。对既有 `PreviewShutdown`/`Shutdown` 保持短事务及租约顺序，测试数据库按独立 ID 操作、退出范围变化拒绝、观察失败、部分失败和取消。不要为了简化移除 task reservation 或删除指纹。

**Verify**：新增 `TestIndependentResourceLifecycle` 于 `resource_lifecycle_test.go`；`go test ./internal/devplatform -list '^TestIndependentResourceLifecycle$'` 列出名称；`go test -race ./internal/devplatform -run '^(TestIndependentResourceLifecycle|TestShutdownRejectsChangedScopeAndCancellationBeforeMutation|TestShutdownDoesNotTreatFailedObservationAsStopped)$' -count=1 -v` → mock 命令全部 PASS。

### Step 2: 菜单栏直接枚举实例与单个代理

改 `panelRunning`、`menubarView`、`panelHeight`，从 Instances 与代理实际运行状态读取；避免同时通过旧 provider 显示同一自有任务。数据库动作复用现有每实例后台任务，不调用原来的全局 project task。HTTPS 设置入口只打开整份规则配置页，不启动上游。代理 Failed/Unknown/Stopped 与运行态分开，健康信息不可由开关布尔值推断。

**Verify**：新增 `TestUIIndependentResourceLifecycle` 于 `resource_lifecycle_ui_test.go`，`menu` 子用例检查零/一/多实例、无站点菜单、unknown、busy、旧 provider 去重、运行数、菜单高度与点击真实目标。`go test -race . -run '^TestUIIndependentResourceLifecycle$/^menu$' -count=1 -v` 和 `go test -race . -run '^TestMenubarPanelTogglesServices$' -count=1 -v` → PASS，无启动外部应用命令。

### Step 3: 接入清晰的退出与失败恢复界面

在 `view_quit.go` 显示自有数据库实例范围及独立代理选择；检查退出中任务完成事件不能把已取消的 dialog 状态覆盖。数据库/HTTPS 规则表未保存草稿分别警告，不调用被删的 projectDirty。停止失败保留窗口，显示失败资源及刷新/重试；不要列出原始凭据或执行命令。

**Verify**：`go test -race . -run '^TestUIIndependentResourceLifecycle/quit$' -count=1 -v` → keep/stop/stop+proxy/cancel/busy/changed-preview/partial-failure/unknown-observation 均 PASS；捕获 Run 必须仅包含期望的自有 labels。`go vet ./...` → exit 0。

### Step 4: 核对原生边界和界面布局

模型测试显式断言数据库数据和凭据文件 digest 在 stop/restart/quit 前后不变；只读操作不得产生删除目录。测试维护任务处理与代理 keep/stop 的组合。UI suite 增加 `layouts`：小窗口/default、明暗、长实例名和多条代理规则（菜单仍仅一个代理入口），用 `MSM_RESOURCE_SHOTS` 输出。

**Verify**：`go test -race ./internal/devplatform -run '^TestIndependentResourceLifecycle$' -count=1 -v` → 数据保留和作用域断言 PASS；`MSM_RESOURCE_SHOTS=build/resource-shots/013 go test . -run '^TestUIIndependentResourceLifecycle/layouts$' -count=1` → 四种布局截图与命中断言通过；`git diff --check` → exit 0。真实启动停止由 015-B 隔离 Mac 验证。

## Test plan

沿用 shutdown_test.go 的注入 Run 和错误观察测试；不要把任意 launchctl 非零都视为 not found。新增 suite 覆盖 stop 一个实例不影响其他实例/代理/外部进程、重启仍同数据、未知状态阻断、并发占用、菜单行为、退出后的缓存刷新。UI tests 统一 mock Run；既有 opt-in 原生生命周期测试保留或按独立资源改写。

## Done criteria

- [ ] 两个新增 suite 含 menu、quit、layouts 与所有模型子用例，实际 PASS，零 SKIP。
- [ ] `go test ./... -run '^$'`、`go vet ./...`、`git diff --check` exit 0。
- [ ] `rg -n 'Projects|projectDirty|projectTasks|devProjectStarted' menubar.go view_quit.go internal/devplatform/shutdown.go` → 无匹配。
- [ ] 命令捕获证明不管理外部上游；数据/凭据前后摘要一致；截图已人工检查。
- [ ] 明确记录 privileged listener 与登录行为的实机未验证边界，不添加自动重启/自启承诺。

## Maintenance notes

菜单栏的运行数量应来自资源状态，不来自可打开的网站数量。任何新自有后台任务都需要同步退出范围和测试；不要恢复项目依赖引用计数来管理共享数据库。

## Non-vacuous verification gate

以下是 Done 的必需补充门槛。`plans/check-test-events.py` 已随计划提供，只读取 stdin 的 Go JSON 事件；拒绝缺失完整测试名、SKIP、FAIL、重复或不完整包事件。不要只凭 exit 0 或顶层 -list 判断子用例通过。先执行 `set -o pipefail`，确保管道前后任一失败都失败。

```sh
set -o pipefail
go test -json -race ./internal/devplatform -run '^TestIndependentResourceLifecycle$' -count=1 | python3 plans/check-test-events.py --package macos-service-manager/internal/devplatform --require TestIndependentResourceLifecycle
go test -json -race . -run '^TestUIIndependentResourceLifecycle$/^(menu|quit)$' -count=1 | python3 plans/check-test-events.py --package macos-service-manager --require TestUIIndependentResourceLifecycle/menu --require TestUIIndependentResourceLifecycle/quit
MSM_RESOURCE_SHOTS=build/resource-shots/013 go test -json -race . -run '^TestUIIndependentResourceLifecycle$' -count=1 | python3 plans/check-test-events.py --package macos-service-manager --require TestUIIndependentResourceLifecycle --require TestUIIndependentResourceLifecycle/layouts --shot-dir build/resource-shots/013
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

013 执行记录：隔离工作树 `/Users/jyeu/code/service-manager-013`，提交 `65cc61e`（基于 012 `60c1b3a`）。八个 Scope 文件；Go 全量、race 复跑、vet、聚焦事件/截图门槛 PASS。race 首轮有旧 Node UI TempDir 清理失败，单项重复及完整复跑 PASS；原生 runner 尚未执行。

集成记录：按用户“合并 master”的明确指示，实现提交链已于本轮快进合入 master（`96549ea`）。此前工作树记录保留为历史；未验证的原生安装、外部卷与系统信任项目仍保持原状态。
