# Plan 005: 保留 Node 终端集成的符号链接配置

> **执行说明**：这是给独立执行者的交接计划。逐步实施并执行每一步的验证；不要把测试名称匹配到零个测试当作成功。遇到 STOP 条件立即报告。完成后仅更新 plans/README.md 中本计划一行，并附实际验证结果。当前文件描述待执行工作，不代表已经修复。
>
> **漂移检查（先执行）**：在仓库根运行 `git diff --stat 0cbf7e9..HEAD -- internal/devplatform/node_manage.go internal/devplatform/node_test.go` 和 `git diff -- internal/devplatform/node_manage.go internal/devplatform/node_test.go`，再运行 `git status --short`。基线时工作区干净。检查所有已提交及未提交变化；路径有变化时逐项比对下方摘录，不能直接覆盖。前置计划造成的预期变化也要记录；行为或接口不再匹配时停止更新计划。

## Status

- **Priority**: P1
- **Effort**: S–M
- **Risk**: LOW–MED — 需明确链接解析和并发修改失败行为
- **Depends on**: 无
- **Category**: bug
- **Planned at**: commit `0cbf7e9`, 2026-11-07
- **Status**: TODO
- **Confidence**: HIGH（源码证据；本计划指定的新增测试尚未实现）

## Why this matters

EnableNodeShell/DisableNodeShell 通过链接读取 ~/.zshrc，随后 atomicWrite 在链接路径 rename，导致 dotfile 管理器的链接变为普通文件。本计划保留有效链接并更新其最终普通文件目标；悬空/循环/非普通目标明确拒绝。普通文件与原本不存在的 .zshrc 保持既有行为，不改变 generic atomicWrite 的语义。

## Current state

引用源码也属于漂移检查范围（只读，不扩大 Scope）：`git diff --stat 0cbf7e9..HEAD -- internal/devplatform/node_manage.go internal/devplatform/model.go internal/devplatform/node_test.go`；另用 `git diff -- internal/devplatform/node_manage.go internal/devplatform/model.go internal/devplatform/node_test.go` 检查未提交变化。下列摘录是精确基线，执行前逐项核对。

仓库根为 `/Users/jyeu/code/service-manager`；在独立 checkout 中使用实际根目录。产品是 Go 1.27.3 + MyGo 的 macOS Apple Silicon 应用，长生命周期服务交给 launchd；项目、实例和软件包由 internal/devplatform 管理。用户操作在 GUI 内完成；cmd/devmgr 是未随 App 分发的测试 harness，不能恢复为公开 CLI。仓库尚未发布，不增加旧来源迁移或兼容层。

### internal/devplatform/node_manage.go:337 — Enable 的读取与替换

```go
	b, e := os.ReadFile(rc)
	switch {
	case e == nil:
		if st, se := os.Stat(rc); se == nil {
			mode = st.Mode().Perm()
		}
	case errors.Is(e, os.ErrNotExist):
		b = nil
	default:
		return rc, e
	}
	if bytes.Contains(b, []byte(nodeBlockBegin)) {
		return rc, nil
	}
	if len(b) > 0 && !bytes.HasSuffix(b, []byte("\n")) {
		b = append(b, '\n')
	}
	if len(b) > 0 {
		b = append(b, '\n')
	}
	b = append(b, m.nodeBlock()...)
	return rc, atomicWrite(rc, b, mode)
```

### internal/devplatform/node_manage.go:370 — Disable 的相同风险

```go
	b, e := os.ReadFile(rc)
	if errors.Is(e, os.ErrNotExist) {
		return rc, nil
	}
	if e != nil {
		return rc, e
	}
	begin := bytes.Index(b, []byte(nodeBlockBegin))
	if begin < 0 {
		return rc, nil
	}
	end := bytes.Index(b[begin:], []byte(nodeBlockEnd))
	if end < 0 {
		return rc, errors.New("the Service Manager block in .zshrc has no end line; remove it by hand")
	}
	end += begin + len(nodeBlockEnd)
	if end < len(b) && b[end] == '\n' {
		end++
	}
	// The blank line that separated the block from what came before goes too.
	if begin > 0 && b[begin-1] == '\n' && (begin == 1 || b[begin-2] == '\n') {
		begin--
	}
	st, e := os.Stat(rc)
	if e != nil {
		return rc, e
	}
	return rc, atomicWrite(rc, append(b[:begin:begin], b[end:]...), st.Mode().Perm())
```

### internal/devplatform/model.go:344 — generic writer 最终 rename

```go
func atomicWrite(path string, b []byte, mode os.FileMode) error {
	if e := os.MkdirAll(filepath.Dir(path), 0700); e != nil {
		return e
	}
	f, e := os.CreateTemp(filepath.Dir(path), ".write-*")
	if e != nil {
		return e
	}
	defer os.Remove(f.Name())
	if e = f.Chmod(mode); e == nil {
		_, e = f.Write(b)
	}
	if e == nil {
		e = f.Sync()
	}
	ce := f.Close()
	if e != nil {
		return e
	}
	if ce != nil {
		return ce
	}
	return os.Rename(f.Name(), path)
```

### internal/devplatform/node_test.go:239 — Home 和 Program 的隔离测试夹具

```go
func TestNodeShellBlockIsAddedOnceAndRemovedCleanly(t *testing.T) {
	m := New(t.TempDir())
	m.Home = t.TempDir()
	m.Program = filepath.Join(t.TempDir(), "app")
	if err := os.WriteFile(m.Program, []byte("#!/bin/sh\n"), 0700); err != nil {
		t.Fatal(err)
	}
	rc := filepath.Join(m.Home, ".zshrc")
	original := "export EDITOR=vim\nalias ll='ls -l'"
	os.WriteFile(rc, []byte(original), 0640)
	if m.NodeShellEnabled() {
		t.Fatal("enabled before anything was written")
	}
	for i := 0; i < 2; i++ {
		if _, err := m.EnableNodeShell(context.Background()); err != nil {
			t.Fatal(err)
```

## Conventions and constraints

用 m.Home 指向 t.TempDir，不改真实 HOME。shell block 仍只保存 source init.sh，不能向 dotfile 增加新的命令解析。保持标记块单次插入、原文件权限、其他字节以及返回 rc 路径的含义。安全目标是正常 dotfile 链接不被破坏，并检测观察到的并发改动；不要声称能在同用户恶意目录替换下提供完整无竞态文件系统事务。

来自 CONTRIBUTING.md 的约束：“Use isolated roots and ports in tests. Stop and unregister every test job.” 真实 hosts、钥匙串、管理员授权、重启/注销不属于自动执行范围。只能在明确安排的原生验收中操作；不能为让测试通过关闭 TLS 验证、扩大权限或接管原有数据库。测试数据不引用真实项目、密钥或凭据；发现秘密只记录文件位置和类型，不复制值。

## Commands you will need

所有命令从仓库根执行，要求 darwin/arm64、Go 1.27.3 和需要时的 Python 3。无需升级依赖或安装新的测试框架。

| 用途 | 命令 | 成功条件 |
|---|---|---|
| 工具链 | `go version` | Go 1.27.3 或满足 go.mod 的版本，darwin/arm64 |
| 依赖完整性 | `go mod verify` | exit 0，all modules verified |
| 聚焦验证 | `go test -race ./internal/devplatform -run '^TestNodeShell' -count=1 -v` | exit 0，指定新增用例实际执行且无 SKIP |
| 静态检查 | `go vet ./...` | exit 0 |
| 差异格式 | `git diff --check -- internal/devplatform/node_manage.go internal/devplatform/node_test.go plans/README.md` | exit 0 |
| 完整回归（隔离验收） | `go test ./...` 和 `go test -race ./...` | exit 0；单列 opt-in SKIP |
| 原生构建（仅代码计划需要时） | `go tool mygo build -sign - -skip-notarize` | exit 0，产物在忽略的 build/ 中；不签发版本、不公证、不发布 |

本计划的必要门槛是下列聚焦测试、go vet 和差异检查。完整 go test 包含真实 launchd 启停；仅在隔离、可清理的 macOS 验证会话中运行，并记录所有 SKIP。

审查阶段已验证过 go vet、go mod verify、7 项隔离 Node 测试及 4 项 Python 测试；不能把这些历史结果视为本计划实施后的验收结果。

## Scope

**允许修改的文件（新增文件也必须在此列出）**：

- internal/devplatform/node_manage.go
- internal/devplatform/node_test.go
- plans/README.md（仅本计划状态与验证结果）

**禁止修改**：上述列表之外的产品源码、go.mod/go.sum、发布凭据、软件目录版本/来源、用户真实配置和数据库。只读引用文件不等于获准修改。

## Git workflow

- 在独立分支 `codex/005-preserve-zshrc-symlinks` 执行。保留已有工作；不要切换或重置包含他人修改的工作树。
- 提交不是计划的完成前提；仅在操作者要求时提交。仓库提交标题以直接描述行为为主，例如 `Restrict the shipped app to internal commands`。
- 不自动合并、推送、创建 PR、发布 issue 或触发签名发行。

## Steps

### 步骤 1: 添加链接回归

扩展 node_test.go，新增 TestNodeShellSymlinkLifecycle：相对、绝对、两级链接均指向 t.TempDir 下的普通文件，目标包含原有文本和 0640 权限。enable 两次、disable 两次；每次 os.Readlink 的原值与链接 inode 类型保留，目标内容只改变指定块，重复操作幂等。沿用现有 fixture Program，禁止真实 shell rc。当前实现应在链接保存断言失败。

**验证**：`go test ./internal/devplatform -run '^TestNodeShellSymlinkLifecycle$' -count=1 -v` → 当前实现因替换链接而失败，fixture 与测试本身可运行

### 步骤 2: 在 shell 配置边界解析写入目标

在 node_manage.go 提取小型读取/提交 helper，返回原 rc、最终写入路径、原字节/模式和观察到的链接身份。先 Lstat rc 区分“路径不存在”和“链接目标不存在”；有效链接用 EvalSymlinks 解析，只接受普通目标。原路径不存在时 Enable 可新建，Disable 保持无操作。悬空、循环、目录/设备目标报可操作错误，不创建目标、不删除链接。enable 和 disable 共同使用该 helper，atomicWrite 仅对最终目标执行；保留原 rc 作为对外返回路径。不要改 model.go。

**验证**：`go test ./internal/devplatform -run '^TestNodeShell' -count=1 -v` → 普通文件、缺失文件、有效链接路径均通过

### 步骤 3: 拒绝检测到的配置漂移

提交前重查链接解析结果、原目标文件身份与字节；若发现用户或 dotfile 工具改动，返回 refresh/retry 错误，不覆盖新内容。让 helper 的提交部分可通过单元测试先 prepare、再改 fixture、最后 commit 来覆盖这个窗口，无需引入生产全局 hook。再新增 TestNodeShellSymlinkRejectsUnsafeTarget（悬空/循环/目录）与 TestNodeShellRejectsChangedConfiguration（改目标、换链接、改内容）。generic atomicWrite 依然只负责替换一个已选定路径。

**验证**：`go test -race ./internal/devplatform -run '^TestNodeShell' -count=1 -v` → 无效/漂移路径报错，链接与目标原内容保留；无竞态

### 步骤 4: 保持 shell 行为与兼容性

运行已有 shell block 和选择器测试。新增断言 NodeShellEnabled 在有效链接启用后为 true、禁用后为 false；语法仍由 /bin/zsh -n 与 /bin/bash -n 检查，不执行脚本。检查 source 路径 quoting 与默认版本逻辑未改动。

**验证**：`go test -race ./internal/devplatform -run 'Test(NodeShell|SelectNodeFollowsOverrideFilesThenDefault)' -count=1 -v` → 现有和新增用例通过

## Test plan

测试沿用 TestNodeShellBlockIsAddedOnceAndRemovedCleanly 的隔离 Home/Program、权限和标记计数。至少 3 类有效链接、3 类拒绝目标、3 类检测到的漂移；每类覆盖 enable/disable。对只读目标，以实际错误为准，避免 root 环境假通过；本仓库验收以普通用户 macOS 执行。

## Done criteria

以下全部满足才可记为 DONE：

- [ ] TestNodeShellSymlinkLifecycle、TestNodeShellSymlinkRejectsUnsafeTarget、TestNodeShellRejectsChangedConfiguration 通过。
- [ ] 链接字节不变，目标仅变化集成块，0640 权限保持。
- [ ] 无效链接和检测到的外部修改不被覆盖。
- [ ] model.go 的 atomicWrite 与运行时版本选择逻辑未修改。
- [ ] 聚焦验证命令实际执行新增用例且通过；无零用例假通过。
- [ ] `go vet ./...` 与 `git diff --check` 通过；未解决的原生验收限制如实记录。
- [ ] `git status --short` 和相对执行起点的 diff 仅包含 Scope 内文件；不得顺手格式化整个仓库。
- [ ] plans/README.md 中该计划状态及验证说明已更新。

## STOP conditions

- 当前代码与摘录不一致，且变化影响实施假设；先报告差异并刷新计划。
- 必须修改 Scope 外文件、引入依赖或扩大产品能力才能完成。
- 合理修正后同一个验证仍连续失败两次；记录命令与错误，不跳过门槛。
- 需要跟随链接创建缺失的真实目标，或需要修改整个通用文件写入层。
- 遇到 hard-link、多用户受保护目标或权限不足；明确返回错误，不提权或 chmod 放宽权限。
- 为避免误覆盖必须改变现有 shell block 语义。

## Maintenance notes

今后其他 shell 的 rc 支持应复用显式路径策略，而不是直接把链接传给 rename writer。此方案不承诺跨多个 dotfile 工具的事务隔离；检测到冲突应保留内容让用户重试。
