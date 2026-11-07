# Plan 007: 为运行状态轮询分配有界且公平的探测预算

> **执行说明**：这是给独立执行者的交接计划。逐步实施并执行每一步的验证；不要把测试名称匹配到零个测试当作成功。遇到 STOP 条件立即报告。完成后仅更新 plans/README.md 中本计划一行，并附实际验证结果。当前文件描述待执行工作，不代表已经修复。
>
> **漂移检查（先执行）**：在仓库根运行 `git diff --stat 0cbf7e9..HEAD -- internal/devplatform/diagnostics.go internal/devplatform/health_test.go` 和 `git diff -- internal/devplatform/diagnostics.go internal/devplatform/health_test.go`，再运行 `git status --short`。基线时工作区干净。检查所有已提交及未提交变化；路径有变化时逐项比对下方摘录，不能直接覆盖。前置计划造成的预期变化也要记录；行为或接口不再匹配时停止更新计划。

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED — 涉及并发、截止时间与状态聚合
- **Depends on**: 无
- **Category**: perf
- **Planned at**: commit `0cbf7e9`, 2026-11-07
- **Status**: TODO
- **Confidence**: HIGH（源码证据；本计划指定的新增测试尚未实现）

## Why this matters

Snapshot 在 5 秒总预算中按固定顺序串行查询，每个服务健康检查可占用 1 秒。多个前置慢服务会使后置健康项目每轮都变成 Unknown。采用有界并发与按观察项分配的预算，在不延长总体刷新期限的条件下让后面的资源也获得观察机会；不能用取消总超时或无界 goroutine 解决。

## Current state

引用源码也属于漂移检查范围（只读，不扩大 Scope）：`git diff --stat 0cbf7e9..HEAD -- internal/devplatform/diagnostics.go internal/devplatform/health_test.go view_status.go`；另用 `git diff -- internal/devplatform/diagnostics.go internal/devplatform/health_test.go view_status.go` 检查未提交变化。下列摘录是精确基线，执行前逐项核对。

仓库根为 `/Users/jyeu/code/service-manager`；在独立 checkout 中使用实际根目录。产品是 Go 1.27.3 + MyGo 的 macOS Apple Silicon 应用，长生命周期服务交给 launchd；项目、实例和软件包由 internal/devplatform 管理。用户操作在 GUI 内完成；cmd/devmgr 是未随 App 分发的测试 harness，不能恢复为公开 CLI。仓库尚未发布，不增加旧来源迁移或兼容层。

### internal/devplatform/diagnostics.go:66 — 单项探测及 PID 二次检查

```go
	check, done := context.WithTimeout(ctx, time.Second)
	err = probeHealth(check, svc.Health)
	done()
	if ctx.Err() != nil {
		result.Status = "Unknown"
		return result
	}
	if err != nil {
		result.Status = "Not ready"
		result.Message = "Process is running but the readiness check failed."
		return result
	}
	// Recheck ownership after the network probe. A listening external process
	// must not hide a managed process which exited during the probe.
	after, err := m.readJobState(ctx, job)
	if err != nil || !after.Running || (state.PID > 0 && after.PID != state.PID) {
		result.Status = "Not ready"
		result.Message = "Process changed or exited during the readiness check."
		return result
```

### internal/devplatform/diagnostics.go:96 — 总预算和结果容器

```go
func (m *Manager) Snapshot(ctx context.Context, s State) RuntimeSnapshot {
	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	out := RuntimeSnapshot{States: map[string]string{}, Services: map[string]ServiceStatus{}, FolderErrors: map[string]string{}}
	instanceRunning := map[string]bool{}
	for _, id := range diagnosticKeys(s.Instances) {
```

### internal/devplatform/diagnostics.go:116 — 固定顺序串行服务观察

```go
	for _, id := range diagnosticKeys(s.Projects) {
		p := s.Projects[id]
		if err := projectFolderError(p.Directory); err != nil {
			out.FolderErrors[id] = err.Error()
		}
		marked := m.loaded(ctx, "project-"+id)
		owns := marked
		running, healthy, unready, restarting, failed, unknown := 0, 0, 0, 0, 0, 0
		for _, svc := range p.Services {
			state := m.observeService(ctx, "p-"+id+"-"+svc.ID, svc)
			key := ServiceStatusKey(id, svc.ID)
			out.Services[key] = state
			out.States[key] = state.Status
```

### internal/devplatform/diagnostics.go:159 — 聚合含全局 context 过期分支

```go
		status := "Stopped"
		total := len(p.Services) + len(p.Instances)
		switch {
		case ctx.Err() != nil || unknown > 0:
			status = "Unknown"
		case total == 0 && marked:
			status = "Failed"
			if proxy, err := m.readJobState(ctx, "proxy"); (err == nil && proxy.Running) || m.socketLoaded(ctx) {
				status = "Running"
			}
		case !owns: // A standalone database is not a project owner.
		case running > 0 && running < total:
			status = "Partly running"
		case failed > 0 || (marked && running == 0 && restarting == 0):
			status = "Failed"
		case restarting > 0:
			status = "Restarting"
		case unready > 0:
			status = "Not ready"
		case running == total && total > 0:
			status = "Running"
			if len(p.Services) > 0 && healthy == len(p.Services) {
				status = "Healthy"
```

### internal/devplatform/health_test.go:70 — 可注入 launchctl 与 HTTP 的测试范式

```go
	defer server.Close()
	jobOutput := "state = running\npid = 123\nruns = 1"
	queryErr := error(nil)
	m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
		if strings.HasSuffix(args[len(args)-1], m.label("p-app-web")) {
			return jobOutput, queryErr
		}
		return "", errors.New("not found")
	}
	s := emptyState()
	p := Project{ID: "app", Services: []Service{{ID: "web", Command: []string{"web"}, Health: server.URL}}}
	s.Projects[p.ID] = p
	check := func(expected string) {
		t.Helper()
		r := m.Snapshot(context.Background(), s)
		if r.States["project:app"] != expected || r.Services[ServiceStatusKey("app", "web")].Status != expected {
			t.Fatal("incorrect observed state", expected, r)
		}
	}
```

### view_status.go:21 — UI 每轮新 Manager 和固定预算

```go
	if d.busy || d.statusPolling || time.Since(d.statusLast) < 5*time.Second {
		return
	}
	d.statusPolling = true
	s, sequence, factory := d.state, d.taskSequence, d.managerFactory
	a.spawn(func() {
		m := devplatform.New(devplatform.DefaultRoot())
		if factory != nil {
			m = factory()
		}
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		result := m.Snapshot(ctx, s)
```

## Conventions and constraints

保留 Stopped/Failed/Restarting/Running/Healthy/Not ready/Unknown 的既有含义；观察超时应为 Unknown，不能冒充进程停止。保留服务健康检查前后 PID 归属核验和 database-only 项目 marker 规则。聚合写 map 放在一个 goroutine；工作任务只返回独立值。现有 UI 每轮创建 Manager，故不要在 Manager 内放“下次起点”而期待跨轮持久化。

来自 CONTRIBUTING.md 的约束：“Use isolated roots and ports in tests. Stop and unregister every test job.” 真实 hosts、钥匙串、管理员授权、重启/注销不属于自动执行范围。只能在明确安排的原生验收中操作；不能为让测试通过关闭 TLS 验证、扩大权限或接管原有数据库。测试数据不引用真实项目、密钥或凭据；发现秘密只记录文件位置和类型，不复制值。

## Commands you will need

所有命令从仓库根执行，要求 darwin/arm64、Go 1.27.3 和需要时的 Python 3。无需升级依赖或安装新的测试框架。

| 用途 | 命令 | 成功条件 |
|---|---|---|
| 工具链 | `go version` | Go 1.27.3 或满足 go.mod 的版本，darwin/arm64 |
| 依赖完整性 | `go mod verify` | exit 0，all modules verified |
| 聚焦验证 | `go test -race ./internal/devplatform -run 'Test(RuntimeSnapshot\|ObservedHealthUsesTheStartupProbeDeadline)' -count=1 -v` | exit 0，指定新增用例实际执行且无 SKIP |
| 静态检查 | `go vet ./...` | exit 0 |
| 差异格式 | `git diff --check -- internal/devplatform/diagnostics.go internal/devplatform/health_test.go plans/README.md` | exit 0 |
| 完整回归（隔离验收） | `go test ./...` 和 `go test -race ./...` | exit 0；单列 opt-in SKIP |
| 原生构建（仅代码计划需要时） | `go tool mygo build -sign - -skip-notarize` | exit 0，产物在忽略的 build/ 中；不签发版本、不公证、不发布 |

本计划的必要门槛是下列聚焦测试、go vet 和差异检查。完整 go test 包含真实 launchd 启停；仅在隔离、可清理的 macOS 验证会话中运行，并记录所有 SKIP。

审查阶段已验证过 go vet、go mod verify、7 项隔离 Node 测试及 4 项 Python 测试；不能把这些历史结果视为本计划实施后的验收结果。

## Scope

**允许修改的文件（新增文件也必须在此列出）**：

- internal/devplatform/diagnostics.go
- internal/devplatform/health_test.go
- plans/README.md（仅本计划状态与验证结果）

**禁止修改**：上述列表之外的产品源码、go.mod/go.sum、发布凭据、软件目录版本/来源、用户真实配置和数据库。只读引用文件不等于获准修改。

## Git workflow

- 在独立分支 `codex/007-fair-runtime-observation` 执行。保留已有工作；不要切换或重置包含他人修改的工作树。
- 提交不是计划的完成前提；仅在操作者要求时提交。仓库提交标题以直接描述行为为主，例如 `Restrict the shipped app to internal commands`。
- 不自动合并、推送、创建 PR、发布 issue 或触发签名发行。

## Steps

### 步骤 1: 加入后置服务饥饿回归

扩展 health_test.go，新增 TestRuntimeSnapshotFairBudget：多个排序靠前的服务以本地 httptest 阻塞到 context 取消，最后一个立即返回 204；m.Run 返回受控运行 PID。验证最后服务与其项目在多轮快照中均 Healthy，并且慢项明确 Not ready/Unknown。测试不启动 launchd。用通道控制服务进入/退出，deadline 仅作上界，避免靠短 sleep 赌顺序。

**验证**：`go test ./internal/devplatform -run '^TestRuntimeSnapshotFairBudget$' -count=1 -v` → 旧实现因最后服务被饿死而失败；测试服务和所有 handler 均清理

### 步骤 2: 拆分观察和聚合

把观察项展开为数据库状态、项目 marker、服务观察，以及仅有站点的项目需要的代理状态。保持每项稳定 key。用最多 4 个工作 goroutine；结果缓冲区按项数分配且不共享可变 map。工作上下文来自共同 5 秒 deadline，每项再获得按剩余总预算和队列批次估算的公平上限，预留收集/聚合时间；服务 probe 仍最多 1 秒，小负载不得把 600ms 的合法健康响应误判。排队也要响应取消；未获得完整观察的项标为 Unknown。不要只并发服务而让串行实例/marker 继续耗尽全局预算。

**验证**：`go test ./internal/devplatform -run 'Test(RuntimeSnapshot|ObservedHealthUsesTheStartupProbeDeadline)' -count=1 -v` → 公平性及既有进程/健康/共享实例归属用例通过

### 步骤 3: 取消全局截止对已完成结果的污染

根据每项已返回的确定状态聚合项目，而不是因为聚合时 ctx.Err 非 nil 就抹掉所有成功结果。仍未完成或被取消的项为 Unknown；保留 Unknown 的优先级和已确认停止的特殊规则。observeService 的二次 readJobState 必须沿用同一任务 context，不能在背景上下文拖延结束。同步等待所有合作式 worker 收尾，不能返回后继续写 out。

**验证**：`go test -race ./internal/devplatform -run 'Test(RuntimeSnapshot|ObservedHealthUsesTheStartupProbeDeadline)' -count=1 -v` → 已完成健康项不因其他超时变为 Unknown；无竞态

### 步骤 4: 验证限流、取消和多轮稳定

新增 TestRuntimeSnapshotBoundedCancellation 和 TestRuntimeSnapshotKeepsCompletedResults。用原子计数确认活跃观察不超过 4；取消后合作式 fake Run/HTTP handler 全部退出且 Snapshot 有限时间返回。包含空集合、多个实例、仅数据库项目、proxy-only 项目、PID 变化、总预算耗尽。先检查旧测试 m.Run stub 是否捕获非并发安全计数，限定本文件内用 mutex/atomic 修正；不得掩盖产品竞态。

**验证**：`go test -race ./internal/devplatform -run 'Test(RuntimeSnapshot|ObservedHealthUsesTheStartupProbeDeadline)' -count=3 -v` → 三轮全部通过；无残留 goroutine/handler，活跃数上限成立

## Test plan

沿用 TestRuntimeSnapshotSeparatesProcessReadinessAndCrash 与 TestRuntimeSnapshotDoesNotAssignStandaloneDatabaseToInactiveProject。新增三个测试组检查公平预算、并发上限、取消清理及已完成状态保留。时间断言采用宽容的总上界，不写毫秒级性能承诺。600ms 合法响应测试必须继续通过。不得通过减少假慢服务数量或取消 5 秒预算让测试变绿。

## Done criteria

以下全部满足才可记为 DONE：

- [ ] TestRuntimeSnapshotFairBudget、TestRuntimeSnapshotBoundedCancellation、TestRuntimeSnapshotKeepsCompletedResults 执行且通过。
- [ ] 聚焦 race 测试连续三轮通过，活跃观察数不超过 4。
- [ ] 已完成健康项不被全局过期抹除；未观察项明确 Unknown。
- [ ] 单服务 600ms 合法健康响应、PID 归属和独立数据库归属既有测试通过。
- [ ] UI 轮询间隔、持久状态格式和服务启动 readiness 逻辑不变。
- [ ] 聚焦验证命令实际执行新增用例且通过；无零用例假通过。
- [ ] `go vet ./...` 与 `git diff --check` 通过；未解决的原生验收限制如实记录。
- [ ] `git status --short` 和相对执行起点的 diff 仅包含 Scope 内文件；不得顺手格式化整个仓库。
- [ ] plans/README.md 中该计划状态及验证说明已更新。

## STOP conditions

- 当前代码与摘录不一致，且变化影响实施假设；先报告差异并刷新计划。
- 必须修改 Scope 外文件、引入依赖或扩大产品能力才能完成。
- 合理修正后同一个验证仍连续失败两次；记录命令与错误，不跳过门槛。
- m.Run 的生产实现不响应 context，导致 worker 无法有界退出；先单列该前置问题。
- 必须改动 view_status.go 或持久化轮询游标才能完成。
- 新并发要求更改其他文件内广泛共用的测试/global state；停止列出范围而非批量改 fixture。

## Maintenance notes

状态公平性不意味着任意规模都可在 5 秒内获得成功响应。规模增大时可以返回 Unknown，但不能固定偏袒前几项；记录预算分配策略并通过测试固定。后续增加探测类型应纳入同一总预算，而不是在聚合阶段临时做无预算 I/O。
