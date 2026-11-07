# Plan 003: 在无法确认运行状态时拒绝项目配置保存

> **执行说明**：这是给独立执行者的交接计划。逐步实施并执行每一步的验证；不要把测试名称匹配到零个测试当作成功。遇到 STOP 条件立即报告。完成后仅更新 plans/README.md 中本计划一行，并附实际验证结果。当前文件描述待执行工作，不代表已经修复。
>
> **漂移检查（先执行）**：在仓库根运行 `git diff --stat 0cbf7e9..HEAD -- internal/devplatform/model.go internal/devplatform/project_settings_test.go` 和 `git diff -- internal/devplatform/model.go internal/devplatform/project_settings_test.go`，再运行 `git status --short`。基线时工作区干净。检查所有已提交及未提交变化；路径有变化时逐项比对下方摘录，不能直接覆盖。前置计划造成的预期变化也要记录；行为或接口不再匹配时停止更新计划。

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW — 复用现有错误感知检查
- **Depends on**: 无
- **Category**: bug
- **Planned at**: commit `0cbf7e9`, 2026-11-07
- **Status**: TODO
- **Confidence**: HIGH（源码证据；本计划指定的新增测试尚未实现）

## Why this matters

SaveProject 用布尔 projectActive 判断能否保存已修改项目；其底层 loaded 把任意 launchctl 错误当成未加载。状态不可观测时仍保存删除/改名后的服务配置，会让旧 job 脱离后续停止枚举。仓库已有 projectActiveKnown，可以区分明确不存在和无法查询，避免重新设计生命周期。

## Current state

引用源码也属于漂移检查范围（只读，不扩大 Scope）：`git diff --stat 0cbf7e9..HEAD -- internal/devplatform/model.go internal/devplatform/engine.go internal/devplatform/project_lifecycle.go internal/devplatform/package_repair.go internal/devplatform/project_settings_test.go`；另用 `git diff -- internal/devplatform/model.go internal/devplatform/engine.go internal/devplatform/project_lifecycle.go internal/devplatform/package_repair.go internal/devplatform/project_settings_test.go` 检查未提交变化。下列摘录是精确基线，执行前逐项核对。

仓库根为 `/Users/jyeu/code/service-manager`；在独立 checkout 中使用实际根目录。产品是 Go 1.27.3 + MyGo 的 macOS Apple Silicon 应用，长生命周期服务交给 launchd；项目、实例和软件包由 internal/devplatform 管理。用户操作在 GUI 内完成；cmd/devmgr 是未随 App 分发的测试 harness，不能恢复为公开 CLI。仓库尚未发布，不增加旧来源迁移或兼容层。

### internal/devplatform/model.go:487 — 有缺陷的保存事务

```go
	return m.withStateTransaction(ctx, func(s *State) (func() error, func(), error) {
		if old, ok := s.Projects[p.ID]; ok && !reflect.DeepEqual(old, p) && m.projectActive(ctx, old) {
			return nil, nil, errors.New("stop the project before changing its settings")
		}
		s.Projects[p.ID] = p
		if e := validateSites(allSites(s)); e != nil {
			return nil, nil, e
		}
		undo, e := m.snapshotLoginConfiguration(p.ID)
		if e != nil {
			return nil, nil, e
		}
		return undo, nil, m.configureLogin(ctx, p)
```

### internal/devplatform/engine.go:35 — 错误被折叠为 false

```go
func (m *Manager) loaded(ctx context.Context, id string) bool {
	_, e := m.Run(ctx, "launchctl", "print", domain()+"/"+m.label(id))
	return e == nil
}

// A loaded, finite launchd marker records intent for this login session. This
// also gives database-only projects an owner without adding a supervisor.
func (m *Manager) projectActive(ctx context.Context, p Project) bool {
	if m.loaded(ctx, "project-"+p.ID) {
		return true
	}
	// Retain resources of jobs started by versions predating project markers.
	for _, svc := range p.Services {
		if m.loaded(ctx, "p-"+p.ID+"-"+svc.ID) {
			return true
		}
	}
```

### internal/devplatform/project_lifecycle.go:103 — 应复用的状态检查

```go
func (m *Manager) projectActiveKnown(ctx context.Context, p Project) (bool, error) {
	jobs := []string{"project-" + p.ID}
	for _, svc := range p.Services {
		jobs = append(jobs, "p-"+p.ID+"-"+svc.ID)
	}
	for _, job := range jobs {
		loaded, e := m.packageJobLoaded(ctx, m.label(job))
		if e != nil {
			return false, fmt.Errorf("cannot confirm project %s state: %w", p.ID, e)
		}
		if loaded {
			return true, nil
		}
	}
	return false, nil
```

### internal/devplatform/package_repair.go:117 — 明确未找到与其他错误的分类

```go
func (m *Manager) packageJobLoaded(ctx context.Context, label string) (bool, error) {
	ctx, cancel := context.WithTimeout(ctx, time.Second)
	defer cancel()
	_, err := m.Run(ctx, "launchctl", "print", domain()+"/"+label)
	if ctx.Err() != nil {
		return false, ctx.Err()
	}
	if err == nil {
		return true, nil
	}
	if strings.Contains(err.Error(), "Could not find service") || strings.Contains(err.Error(), "Could not find specified service") {
		return false, nil
	}
	return false, fmt.Errorf("could not verify whether the package is in use: %w", err)
```

### internal/devplatform/project_settings_test.go:108 — 隔离 root、LoginDir 和原配置快照范式

```go
			m := testManager(t)
			m.LoginDir = t.TempDir()
			ctx := context.Background()
			p := Project{SchemaVersion: Schema, ID: "login-rollback", Name: "Saved", Directory: t.TempDir(), Autostart: change != "enable"}
			if e := m.SaveProject(ctx, p); e != nil {
				t.Fatal(e)
			}
			before, readErr := os.ReadFile(m.loginPath(p.ID))
			oldState, e := os.ReadFile(filepath.Join(m.Root, "state.json"))
			if e != nil {
				t.Fatal(e)
			}
			draft := p.Clone()
```

## Conventions and constraints

继续使用 resourceLeases 与 withStateTransaction，先检查旧配置，再修改 s.Projects，再配置登录任务。不要在事务外做一次检查后直接相信它，也不要让 UI 提供的状态代替后端检查。匹配现有错误返回风格。测试中“未找到”应模拟实际分类器接受的 Could not find service，而不是泛化为任意 error。

来自 CONTRIBUTING.md 的约束：“Use isolated roots and ports in tests. Stop and unregister every test job.” 真实 hosts、钥匙串、管理员授权、重启/注销不属于自动执行范围。只能在明确安排的原生验收中操作；不能为让测试通过关闭 TLS 验证、扩大权限或接管原有数据库。测试数据不引用真实项目、密钥或凭据；发现秘密只记录文件位置和类型，不复制值。

## Commands you will need

所有命令从仓库根执行，要求 darwin/arm64、Go 1.27.3 和需要时的 Python 3。无需升级依赖或安装新的测试框架。

| 用途 | 命令 | 成功条件 |
|---|---|---|
| 工具链 | `go version` | Go 1.27.3 或满足 go.mod 的版本，darwin/arm64 |
| 依赖完整性 | `go mod verify` | exit 0，all modules verified |
| 聚焦验证 | `go test -race ./internal/devplatform -run '^TestProjectSave' -count=1 -v` | exit 0，指定新增用例实际执行且无 SKIP |
| 静态检查 | `go vet ./...` | exit 0 |
| 差异格式 | `git diff --check -- internal/devplatform/model.go internal/devplatform/project_settings_test.go plans/README.md` | exit 0 |
| 完整回归（隔离验收） | `go test ./...` 和 `go test -race ./...` | exit 0；单列 opt-in SKIP |
| 原生构建（仅代码计划需要时） | `go tool mygo build -sign - -skip-notarize` | exit 0，产物在忽略的 build/ 中；不签发版本、不公证、不发布 |

本计划的必要门槛是下列聚焦测试、go vet 和差异检查。完整 go test 包含真实 launchd 启停；仅在隔离、可清理的 macOS 验证会话中运行，并记录所有 SKIP。

审查阶段已验证过 go vet、go mod verify、7 项隔离 Node 测试及 4 项 Python 测试；不能把这些历史结果视为本计划实施后的验收结果。

## Scope

**允许修改的文件（新增文件也必须在此列出）**：

- internal/devplatform/model.go
- internal/devplatform/project_settings_test.go
- plans/README.md（仅本计划状态与验证结果）

**禁止修改**：上述列表之外的产品源码、go.mod/go.sum、发布凭据、软件目录版本/来源、用户真实配置和数据库。只读引用文件不等于获准修改。

## Git workflow

- 在独立分支 `codex/003-reject-unknown-project-state` 执行。保留已有工作；不要切换或重置包含他人修改的工作树。
- 提交不是计划的完成前提；仅在操作者要求时提交。仓库提交标题以直接描述行为为主，例如 `Restrict the shipped app to internal commands`。
- 不自动合并、推送、创建 PR、发布 issue 或触发签名发行。

## Steps

### 步骤 1: 添加失败保护回归

新增 TestProjectSaveRejectsUnknownState，先用隔离 LoginDir 和有效项目保存基线，再替换 m.Run：launchctl print 返回 observation denied 或独立超时错误，任何 bootout/bootstrap 调用直接令测试失败。分别修改服务名称、移除服务引用和 autostart；比较 state.json、登录 plist 及 jobs 目录前后相同。再加 TestProjectSaveAllowsKnownStoppedState，所有目标返回明确未找到时允许保存。

**验证**：`go test ./internal/devplatform -run '^TestProjectSave(RejectsUnknownState|AllowsKnownStoppedState)$' -count=1 -v` → 改实现前 unknown-state 用例在“错误被接受”断言失败；known-stopped 通过

### 步骤 2: 替换保存前置检查

仅当同 ID 项目存在且 !reflect.DeepEqual(old,p) 时调用 projectActiveKnown(ctx,old)。err 非 nil 直接返回事务错误；active 为 true 保留 stop the project 提示；确定停止才赋值。新增项目无旧 job 检查，同值保存保持原语义。检查必须位于任何 s.Projects 赋值及 configureLogin 之前；不要修改全局 loaded 或 projectActive，避免扩大范围。

**验证**：`go test ./internal/devplatform -run '^TestProjectSave' -count=1 -v` → unknown、known-stopped 及既有登录回滚用例通过

### 步骤 3: 补全状态矩阵

加入 TestProjectSaveStateGuardMatrix：marker 已加载拒绝、marker 未找到但某服务已加载拒绝、全部明确未找到成功、新项目创建成功、同值保存不引入新状态拒绝、父 context 已取消不写入。以只读计数 stub 保证未知状态路径没有写任务的命令。保留 TestProjectSaveFailureRestoresLoginPreference 对发布失败的既有检查。

**验证**：`go test -race ./internal/devplatform -run '^TestProjectSave' -count=1 -v` → 全部命中测试通过，无真实服务启停，无竞态

## Test plan

测试落在 project_settings_test.go，复用 testManager、Project.Clone 和基线字节比较。至少覆盖 3 类未知状态变更以及上述 6 类状态矩阵。所有 m.Run stub 仅作用于该 Manager；LoginDir 必须为 t.TempDir，不能使用用户 LaunchAgents。测试失败不得修改生产 classifier 使任意 error 都被视为未找到。

## Done criteria

以下全部满足才可记为 DONE：

- [ ] TestProjectSaveRejectsUnknownState、TestProjectSaveAllowsKnownStoppedState、TestProjectSaveStateGuardMatrix 实际执行并通过。
- [ ] TestProjectSaveFailureRestoresLoginPreference 通过。
- [ ] 保存检查调用 projectActiveKnown；loaded/projectActive 定义保持原样。
- [ ] 状态未知时持久配置、登录 plist、任务文件保持原字节。
- [ ] 聚焦验证命令实际执行新增用例且通过；无零用例假通过。
- [ ] `go vet ./...` 与 `git diff --check` 通过；未解决的原生验收限制如实记录。
- [ ] `git status --short` 和相对执行起点的 diff 仅包含 Scope 内文件；不得顺手格式化整个仓库。
- [ ] plans/README.md 中该计划状态及验证说明已更新。

## STOP conditions

- 当前代码与摘录不一致，且变化影响实施假设；先报告差异并刷新计划。
- 必须修改 Scope 外文件、引入依赖或扩大产品能力才能完成。
- 合理修正后同一个验证仍连续失败两次；记录命令与错误，不跳过门槛。
- projectActiveKnown 不再区分不存在和观察失败；需单独刷新该前提，不能在本计划复制另一套 classifier。
- 新检查引起其他测试依赖“任意错误表示不存在”；在本 Scope 内无法纠正 fixture 时列出外部文件并停止。
- 必须放宽锁或在 GUI 加绕过选项才能让保存工作。

## Maintenance notes

审查未来配置写入都应使用“已确认停止”，不是“未观察到运行”。此计划不改 Snapshot 展示状态，也不修改启动/停止算法。新增查询错误信息不可附原始环境或凭据。
