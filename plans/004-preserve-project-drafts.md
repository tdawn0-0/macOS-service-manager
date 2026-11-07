# Plan 004: 保护项目列表导航中的未保存草稿

> **执行说明**：这是给独立执行者的交接计划。逐步实施并执行每一步的验证；不要把测试名称匹配到零个测试当作成功。遇到 STOP 条件立即报告。完成后仅更新 plans/README.md 中本计划一行，并附实际验证结果。当前文件描述待执行工作，不代表已经修复。
>
> **漂移检查（先执行）**：在仓库根运行 `git diff --stat 0cbf7e9..HEAD -- view_development.go project_navigation_test.go docs/development-ui.md` 和 `git diff -- view_development.go project_navigation_test.go docs/development-ui.md`，再运行 `git status --short`。基线时工作区干净。检查所有已提交及未提交变化；路径有变化时逐项比对下方摘录，不能直接覆盖。前置计划造成的预期变化也要记录；行为或接口不再匹配时停止更新计划。

## Status

- **Priority**: P1
- **Effort**: S–M
- **Risk**: MED — 区分用户导航与程序化刷新
- **Depends on**: 无；未来诊断跳转实现应复用本计划的导航保护
- **Category**: bug
- **Planned at**: commit `0cbf7e9`, 2026-11-07
- **Status**: DONE
- **Confidence**: HIGH（源码证据；本计划指定的新增测试尚未实现）

## Why this matters

项目列表任何点击都会调用 selectProject，立即用已保存数据重置草稿和服务编辑器；再次点击当前项目也会丢失输入。现有 projectDirty 已能检测两层未保存状态。为用户导航增加明确的丢弃确认，并把当前项目重复点击变为不重置，保留程序化选择的既有用途。

## Current state

引用源码也属于漂移检查范围（只读，不扩大 Scope）：`git diff --stat 0cbf7e9..HEAD -- view_development.go view.go development_test.go`；另用 `git diff -- view_development.go view.go development_test.go` 检查未提交变化。下列摘录是精确基线，执行前逐项核对。

仓库根为 `/Users/jyeu/code/service-manager`；在独立 checkout 中使用实际根目录。产品是 Go 1.27.3 + MyGo 的 macOS Apple Silicon 应用，长生命周期服务交给 launchd；项目、实例和软件包由 internal/devplatform 管理。用户操作在 GUI 内完成；cmd/devmgr 是未随 App 分发的测试 harness，不能恢复为公开 CLI。仓库尚未发布，不增加旧来源迁移或兼容层。

### view_development.go:236 — 底层选择会重置草稿、服务和位置

```go
func (d *developmentState) selectProject(p devplatform.Project) {
	if d.runtimeTargetProject != "" && d.runtimeTargetProject != p.ID {
		d.runtimeTargetProject = ""
		d.runtimeTargetService = ""
		d.softwareContext = ""
	}
	d.id = p.ID
	d.folderPreview = nil
	d.folderDraft = nil
	d.draft = p.Clone()
	d.selectService(devplatform.Service{})
	d.logText = ""
	d.logService = ""
	d.projectTab = 0
	d.detailScroll[0].Y = 0
	if len(p.Services) > 0 {
		d.selectService(p.Services[0])
		d.logService = p.Services[0].ID
	}
```

### view_development.go:353 — 已有完整 dirty 检测

```go
func (d *developmentState) projectDirty() bool {
	if d.id == "" {
		return false
	}
	if !reflect.DeepEqual(d.draft, d.state.Projects[d.id]) {
		return true
	}
	if d.serviceID != "" {
		for _, s := range d.draft.Services {
			if s.ID == d.serviceOriginalID {
				return d.serviceID != s.ID || d.servicePackage != s.Package || d.arguments != displayServiceCommand(s) || !slices.Equal(d.localArguments, s.LocalArguments) || d.workdir != s.Directory || d.environment != launchd.FormatEnv(s.Environment) || d.dependencies != strings.Join(s.DependsOn, ",") || d.health != s.Health || d.envFile != s.EnvFile || d.serviceRestart != s.Restart || d.serviceReadinessTimeout != serviceSeconds(s.ReadinessTimeoutSeconds, 30) || d.serviceRestartThrottle != serviceSeconds(s.RestartThrottleSeconds, 10)
			}
		}
		return true
	}
```

### view_development.go:612 — 无保护的列表点击

```go
		for _, id := range sortedKeys(d.state.Projects) {
			p := d.state.Projects[id]
			status := d.statuses["project:"+id]
			if devListEntry(c, "Project "+p.Name, p.Name, status, devNews(status), d.id == id && !d.importing) {
				d.selectProject(p)
				d.commandWorkOpen = false
				d.importing = false
			}
```

### view.go:396 — 现有确认框范式

```go
func (a *app) askDialog(c *ui.Context) {
	if ui.AlertDialog(c, &a.ask.open, a.ask.title, a.ask.message, "Cancel", a.ask.button) == 1 && a.ask.run != nil {
		run := a.ask.run
		a.ask.run = nil
		run()
	}
}

```

### development_test.go:16 — 固定示例数据与可控异步测试范式

```go
func TestStatusPollCannotOverwriteAnOperationOrUnsavedDraft(t *testing.T) {
	a := designFixture(t)
	a.dev.draft.Name = "Unsaved project name"
	a.dev.statusLast = time.Now().Add(-time.Minute)
	var work []func()
	a.spawn = func(f func()) { work = append(work, f) }
	a.post = func(f func()) { f() }
	a.dev.managerFactory = func() *devplatform.Manager {
		m := devplatform.New(t.TempDir())
		m.Run = func(context.Context, string, ...string) (string, error) { return "", errors.New("not found") }
		return m
	}
	a.pollDevStatus()
	if len(work) != 1 || !a.dev.statusPolling {
		t.Fatal("poll did not start")
```

## Conventions and constraints

选择最小行为变更：点击当前项目保留草稿；切换到其他项目且 dirty 时弹出现有 askState，按钮为 Cancel / Discard changes。取消和 Escape 保留全部输入；确认后只切换，不自动保存。不要加入全局草稿持久化、多个项目的草稿缓存或新保存机制。docs/development-ui.md 要求稳定可访问名称、原生控件、默认 1120×720 与最小 900×520、明暗主题；复用 ui.AlertDialog 和主题，不另建视觉语言。

来自 CONTRIBUTING.md 的约束：“Use isolated roots and ports in tests. Stop and unregister every test job.” 真实 hosts、钥匙串、管理员授权、重启/注销不属于自动执行范围。只能在明确安排的原生验收中操作；不能为让测试通过关闭 TLS 验证、扩大权限或接管原有数据库。测试数据不引用真实项目、密钥或凭据；发现秘密只记录文件位置和类型，不复制值。

## Commands you will need

所有命令从仓库根执行，要求 darwin/arm64、Go 1.27.3 和需要时的 Python 3。无需升级依赖或安装新的测试框架。

| 用途 | 命令 | 成功条件 |
|---|---|---|
| 工具链 | `go version` | Go 1.27.3 或满足 go.mod 的版本，darwin/arm64 |
| 依赖完整性 | `go mod verify` | exit 0，all modules verified |
| 聚焦验证 | `go test -race . -run '^TestProjectNavigation' -count=1 -v` | exit 0，指定新增用例实际执行且无 SKIP |
| 静态检查 | `go vet ./...` | exit 0 |
| 差异格式 | `git diff --check -- view_development.go project_navigation_test.go docs/development-ui.md plans/README.md` | exit 0 |
| 完整回归（隔离验收） | `go test ./...` 和 `go test -race ./...` | exit 0；单列 opt-in SKIP |
| 原生构建（仅代码计划需要时） | `go tool mygo build -sign - -skip-notarize` | exit 0，产物在忽略的 build/ 中；不签发版本、不公证、不发布 |

本计划的必要门槛是下列聚焦测试、go vet 和差异检查。完整 go test 包含真实 launchd 启停；仅在隔离、可清理的 macOS 验证会话中运行，并记录所有 SKIP。

审查阶段已验证过 go vet、go mod verify、7 项隔离 Node 测试及 4 项 Python 测试；不能把这些历史结果视为本计划实施后的验收结果。

## Scope

**允许修改的文件（新增文件也必须在此列出）**：

- view_development.go
- project_navigation_test.go
- docs/development-ui.md
- plans/README.md（仅本计划状态与验证结果）

**禁止修改**：上述列表之外的产品源码、go.mod/go.sum、发布凭据、软件目录版本/来源、用户真实配置和数据库。只读引用文件不等于获准修改。

## Git workflow

- 在独立分支 `codex/004-preserve-project-drafts` 执行。保留已有工作；不要切换或重置包含他人修改的工作树。
- 提交不是计划的完成前提；仅在操作者要求时提交。仓库提交标题以直接描述行为为主，例如 `Restrict the shipped app to internal commands`。
- 不自动合并、推送、创建 PR、发布 issue 或触发签名发行。

## Steps

### 步骤 1: 用真实点击固定数据丢失用例

在新 project_navigation_test.go 使用 designFixture 和 ui.NewTester 创建两个内存项目。分别修改 d.draft.Name、未 flush 的服务参数和环境字段；通过项目行可访问名称点击当前项目和另一项目。新增 TestProjectNavigationKeepsCurrentDraft 与 TestProjectNavigationConfirmsDiscard。测试不运行后端或 launchd；使用可控 spawn/post，断言点击前后的 saved state 未变化。

**验证**：`go test . -run '^TestProjectNavigation' -count=1 -v` → 改实现前仅草稿保护/确认框断言失败，不能依赖原生文件选择器

### 步骤 2: 增加用户导航边界

在 view_development.go 新增 app.requestProjectSelection(id string)（若仓库已有等价 helper 则先核对）。若目标不存在则保持当前选择并给出现有 toast；同一项目只退出 importing/命令详情等必要外层视图，不重建 draft、服务字段、tab 或 scroll。目标不同且 dirty 时复用 askState；确认 callback 记录来源项目 ID，执行时重新查询目标的最新保存记录。来源已改变、目标已删除或当前任务阻止切换时保持现状并提示重试。实际切换的共同 helper 才调用 selectProject 并关闭 importing/commandWorkOpen。干净切换不提示；不在 selectProject 内加弹窗，以免影响导入与后台刷新。

**验证**：`go test . -run '^TestProjectNavigation' -count=1 -v` → 当前项目输入不变；其他项目只在确认后重置

### 步骤 3: 覆盖取消、竞态与既有刷新

新增 TestProjectNavigationStaleTarget：弹窗打开后从内存状态移除目标，确认不切到旧快照；另测来源项目改变时 callback 不丢弃新草稿。测试 Cancel、Escape、干净切换、不自动调用 SaveProject。复用旧 TestStatusPollCannotOverwriteAnOperationOrUnsavedDraft 验证异步轮询不重置草稿。确认后使用最新目标记录，而不是闭包捕获的过期整个 Project。

**验证**：`go test -race . -run 'Test(ProjectNavigation|StatusPollCannotOverwriteAnOperationOrUnsavedDraft|ProjectFormDoesNotMutateSavedConfiguration)' -count=1 -v` → 全部通过且没有后端写入或竞态

### 步骤 4: 检查最小窗口并记录交互契约

让导航测试分别以 900×520 明暗主题及 1120×720 运行，断言确认/取消按钮可点击，长名称不会挤掉操作。使用 ui.Tester 渲染图像检查弹窗；如输出复核图，仅写忽略的 build/plan-004/，不重新生成所有历史截图。更新 development-ui.md 一段说明当前行保留、切换确认、取消保留。

**验证**：`go test . -run '^TestProjectNavigation' -count=1 -v` → 所有尺寸与主题子用例通过；按钮可命中，草稿保持断言成立

## Test plan

新文件使用 development_test.go 的 designFixture + ui.NewTester 范式。包含同项目重复点击、dirty 切换确认、Cancel、Escape、干净切换、目标删除、来源改变及保存记录刷新。同时检查 draft 和 service editor；仅检查 draft.Name 不足以保护尚未 flush 的服务输入。所有断言使用虚构数据。

## Done criteria

以下全部满足才可记为 DONE：

- [x] 三个 TestProjectNavigation* 测试组实际执行且通过，含两个窗口尺寸和明暗主题。
- [x] 当前项目重复点击不改草稿、服务字段和详情位置。
- [x] dirty 跨项目切换只有确认后发生；取消不保存也不丢弃。
- [x] saved state 不因导航写入；程序化 selectProject 调用语义保持原样。
- [x] 聚焦验证命令实际执行新增用例且通过；无零用例假通过。
- [x] `go vet ./...` 与 `git diff --check` 通过；未解决的原生验收限制如实记录。
- [x] `git status --short` 和相对执行起点的 diff 仅包含 Scope 内文件；不得顺手格式化整个仓库。
- [x] plans/README.md 中该计划状态及验证说明已更新。

## STOP conditions

- 当前代码与摘录不一致，且变化影响实施假设；先报告差异并刷新计划。
- 必须修改 Scope 外文件、引入依赖或扩大产品能力才能完成。
- 合理修正后同一个验证仍连续失败两次；记录命令与错误，不跳过门槛。
- 必须修改共享 AlertDialog 实现或引入跨页面路由框架。
- 既有导入/保存刷新依赖在 selectProject 内执行确认；应停下调查，而不是把确认强塞入低层函数。
- 测试需要真实服务或管理员授权，说明夹具范围过大。

## Maintenance notes

后续诊断跳转、搜索结果、菜单栏导航若会切换项目，应走同一用户导航边界。保留低层 selectProject 用于明确已经保存/确认的流程。不要用自动保存掩盖丢弃问题，尤其是尚未校验的环境字段。

## 最终集成修订（2026-11-07）

主代理全量测试发现两处既有 E2E 会从未保存项目切换目标后立即继续操作。允许额外修改 project_commands_e2e_test.go 和 project_lifecycle_e2e_test.go：保留原来的草稿保留断言，在有意丢弃后继续测试的切换点明确断言确认框并点击 Discard changes。禁止放宽产品 guard 或移除原断言。聚焦验证：go test -race . -run 'Test(UIBackgroundDependencyCommandKeepsIndependentConfigurationAndDraft|UIIndependentProjectsStartConcurrentlyKeepDraftsAndLatestHTTPS)' -count=1。修订随004原子提交一并交付。
