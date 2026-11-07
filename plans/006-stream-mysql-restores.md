# Plan 006: 流式传输 MySQL 恢复输入

> **执行说明**：这是给独立执行者的交接计划。逐步实施并执行每一步的验证；不要把测试名称匹配到零个测试当作成功。遇到 STOP 条件立即报告。完成后仅更新 plans/README.md 中本计划一行，并附实际验证结果。当前文件描述待执行工作，不代表已经修复。
>
> **漂移检查（先执行）**：在仓库根运行 `git diff --stat 0cbf7e9..HEAD -- internal/devplatform/database_other.go internal/devplatform/mysql_restore_test.go` 和 `git diff -- internal/devplatform/database_other.go internal/devplatform/mysql_restore_test.go`，再运行 `git status --short`。基线时工作区干净。检查所有已提交及未提交变化；路径有变化时逐项比对下方摘录，不能直接覆盖。前置计划造成的预期变化也要记录；行为或接口不再匹配时停止更新计划。

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED — 保留恢复目标、取消与失败语义
- **Depends on**: 无
- **Category**: perf
- **Planned at**: commit `0cbf7e9`, 2026-11-07
- **Status**: DONE
- **Confidence**: HIGH（源码证据；本计划指定的新增测试尚未实现）

## Why this matters

MySQL 备份已直接流向文件，恢复却先 CREATE DATABASE，再把完整文件读入 []byte 并拼接成 SQL 字符串。大备份会占用与文件大小成比例的多份内存；读取失败还会留下已创建的空库。本计划在建库前打开/校验源文件，然后流式送入 mysql，不改整个数据库任务框架。未测量实际峰值内存，不预设提速百分比。

## Current state

引用源码也属于漂移检查范围（只读，不扩大 Scope）：`git diff --stat 0cbf7e9..HEAD -- internal/devplatform/database_other.go internal/devplatform/database_operations.go internal/devplatform/database_operations_test.go`；另用 `git diff -- internal/devplatform/database_other.go internal/devplatform/database_operations.go internal/devplatform/database_operations_test.go` 检查未提交变化。下列摘录是精确基线，执行前逐项核对。

仓库根为 `/Users/jyeu/code/service-manager`；在独立 checkout 中使用实际根目录。产品是 Go 1.27.3 + MyGo 的 macOS Apple Silicon 应用，长生命周期服务交给 launchd；项目、实例和软件包由 internal/devplatform 管理。用户操作在 GUI 内完成；cmd/devmgr 是未随 App 分发的测试 harness，不能恢复为公开 CLI。仓库尚未发布，不增加旧来源迁移或兼容层。

### internal/devplatform/database_other.go:78 — 现有 SQL 命令使用完整字符串

```go
func (m *Manager) mysqlSQL(ctx context.Context, s *State, i Instance, sql string) (string, error) {
	bin, e := m.Binary(s, i.Package, "mysql")
	if e != nil {
		return "", e
	}
	c := exec.CommandContext(ctx, bin, "--defaults-file="+filepath.Join(m.instanceDirectory(i.ID), "client.cnf"), "--batch", "--skip-column-names")
	c.Stdin = strings.NewReader(sql)
	out, e := c.CombinedOutput()
	if e != nil {
		return "", fmt.Errorf("MySQL command failed: %w", e)
	}
	return string(out), nil
```

### internal/devplatform/database_other.go:131 — 备份已有流式输出

```go
		f, e := os.OpenFile(path, os.O_WRONLY|os.O_TRUNC, 0600)
		if e != nil {
			return e
		}
		defer f.Close()
		c := exec.CommandContext(ctx, bin, "--defaults-file="+filepath.Join(m.instanceDirectory(i.ID), "client.cnf"), "--single-transaction", "--set-gtid-purged=OFF", name)
		c.Stdout = f
		return c.Run()
```

### internal/devplatform/database_other.go:145 — 恢复整文件读取

```go
func (m *Manager) otherRestore(ctx context.Context, s *State, i Instance, name, path string) error {
	if i.Kind == "redis" || i.Kind == "valkey" {
		return m.redisRestore(ctx, i, name, path)
	}
	if i.Kind == "mysql" {
		if e := m.otherCreateDatabase(ctx, s, i, name); e != nil {
			return e
		}
		b, e := os.ReadFile(path)
		if e != nil {
			return e
		}
		_, e = m.mysqlSQL(ctx, s, i, "USE "+sqlIdentifier(name)+";\n"+string(b))
		return e
	}
	if i.Kind == "mongodb" {
		return m.mongoRestore(ctx, i, name, path)
```

### internal/devplatform/database_operations.go:16 — 数据工具已有资源租约边界

```go
// Data tools use an immutable configuration snapshot, never a state transaction.
// Every lifecycle or data-directory mutation must respect the same instance lease.
func (m *Manager) withDatabaseOperation(ctx context.Context, id string, fn func(*State) error) error {
	if e := validID(id); e != nil {
		return e
	}
	if e := m.init(); e != nil {
		return e
	}
	release, e := m.resourceLeases(ctx, instanceResourceKeys([]string{id})...)
	if e != nil {
		return e
	}
	defer release()
```

### internal/devplatform/database_operations_test.go:26 — 临时包与可执行工具夹具范式

```go
	})
	p := Package{Name: "database-tool-fixture", Version: "operation-test", Arch: "universal", Binaries: map[string]string{"pg_dump": "pg_dump", "createdb": "createdb", "pg_restore": "pg_restore", "psql": "psql"}}
	script := fmt.Sprintf("#!/usr/bin/python3\nimport pathlib,sys,time,os\nroot=pathlib.Path(%q)\nname=sys.argv[-1]\n(root/('entered-'+name)).write_text('started')\n(root/('pid-'+name)).write_text(str(os.getpid()))\noutput=None\nif '-f' in sys.argv:\n output=pathlib.Path(sys.argv[sys.argv.index('-f')+1]); output.write_bytes(b'partial')\nwhile not (root/'release').exists(): time.sleep(.02)\nif output: output.write_bytes(b'completed dump')\n", m.Root)
	if e := os.MkdirAll(m.packageDir(p), 0700); e != nil {
		t.Fatal(e)
	}
	for _, rel := range p.Binaries {
		if e := os.WriteFile(filepath.Join(m.packageDir(p), rel), []byte(script), 0700); e != nil {
			t.Fatal(e)
		}
```

## Conventions and constraints

保持 mysql 的 --defaults-file 参数在首位、--batch/--skip-column-names、sqlIdentifier 引号和 exec.CommandContext。普通建库/用户 SQL 可继续使用 mysqlSQL；新增 restore 专用 reader 通路，避免把其他调用返回值改为丢弃。恢复输出不得无界 CombinedOutput，也不得把 SQL 内容或凭据纳入错误。状态、租约、worker、备份格式和新数据库目标策略不变。

来自 CONTRIBUTING.md 的约束：“Use isolated roots and ports in tests. Stop and unregister every test job.” 真实 hosts、钥匙串、管理员授权、重启/注销不属于自动执行范围。只能在明确安排的原生验收中操作；不能为让测试通过关闭 TLS 验证、扩大权限或接管原有数据库。测试数据不引用真实项目、密钥或凭据；发现秘密只记录文件位置和类型，不复制值。

## Commands you will need

所有命令从仓库根执行，要求 darwin/arm64、Go 1.27.3 和需要时的 Python 3。无需升级依赖或安装新的测试框架。

| 用途 | 命令 | 成功条件 |
|---|---|---|
| 工具链 | `go version` | Go 1.27.3 或满足 go.mod 的版本，darwin/arm64 |
| 依赖完整性 | `go mod verify` | exit 0，all modules verified |
| 聚焦验证 | `go test -race ./internal/devplatform -run '^TestMySQLRestore' -count=1 -v` | exit 0，指定新增用例实际执行且无 SKIP |
| 静态检查 | `go vet ./...` | exit 0 |
| 差异格式 | `git diff --check -- internal/devplatform/database_other.go internal/devplatform/mysql_restore_test.go plans/README.md` | exit 0 |
| 完整回归（隔离验收） | `go test ./...` 和 `go test -race ./...` | exit 0；单列 opt-in SKIP |
| 原生构建（仅代码计划需要时） | `go tool mygo build -sign - -skip-notarize` | exit 0，产物在忽略的 build/ 中；不签发版本、不公证、不发布 |

本计划的必要门槛是下列聚焦测试、go vet 和差异检查。完整 go test 包含真实 launchd 启停；仅在隔离、可清理的 macOS 验证会话中运行，并记录所有 SKIP。

审查阶段已验证过 go vet、go mod verify、7 项隔离 Node 测试及 4 项 Python 测试；不能把这些历史结果视为本计划实施后的验收结果。

## Scope

**允许修改的文件（新增文件也必须在此列出）**：

- internal/devplatform/database_other.go
- internal/devplatform/mysql_restore_test.go
- plans/README.md（仅本计划状态与验证结果）

**禁止修改**：上述列表之外的产品源码、go.mod/go.sum、发布凭据、软件目录版本/来源、用户真实配置和数据库。只读引用文件不等于获准修改。

## Git workflow

- 在独立分支 `codex/006-stream-mysql-restores` 执行。保留已有工作；不要切换或重置包含他人修改的工作树。
- 提交不是计划的完成前提；仅在操作者要求时提交。仓库提交标题以直接描述行为为主，例如 `Restrict the shipped app to internal commands`。
- 不自动合并、推送、创建 PR、发布 issue 或触发签名发行。

## Steps

### 步骤 1: 建立无数据库的恢复夹具

新增 mysql_restore_test.go，用 testManager + 临时 Package 的 mysql 二进制映射。仿照 databaseToolFixture 的隔离布局，但只直接调用 otherRestore，不启动后台 worker/launchd。假 mysql 可通过测试二进制 helper 或临时脚本读取 stdin，并分别记录建库调用与恢复调用的顺序/摘要，不落下完整大文件副本。添加 TestMySQLRestoreRejectsUnreadableSourceBeforeCreate，覆盖不存在/目录输入，断言未调用建库。

**验证**：`go test ./internal/devplatform -run '^TestMySQLRestoreRejectsUnreadableSourceBeforeCreate$' -count=1 -v` → 当前实现因先建库而失败；没有连接真实数据库

### 步骤 2: 添加流式恢复通路

在 MySQL 分支先 os.Open + Stat 检查普通文件，defer Close，再创建新数据库。用 io.MultiReader(strings.NewReader("USE "+sqlIdentifier(name)+";\n"), source) 送入 restore 专用命令。源内容不得转成 string、ReadFile 或 ReadAll。正常 restore stdout 送 io.Discard；stderr 同样不无界缓存，保持现有不返回原始 SQL/凭据的错误风格。使用 exec.CommandContext，输入读取错误和非零退出必须传回调用方；不要在失败时自动 DROP DATABASE，以免引入破坏性补偿。

**验证**：`go test ./internal/devplatform -run '^TestMySQLRestore' -count=1 -v` → 输入先验、SQL 前缀/内容摘要、argv 顺序与退出状态测试通过

### 步骤 3: 验证真实分块和取消

新增 TestMySQLRestoreStreamingAndCancellation。把底层 reader 执行 helper 保持为私有函数，注入按需生成的固定模式 reader（至少 32 MiB，不在内存预建整块），用 gate 在首块已被假 mysql 消费后才允许提供后续块，以证明并非先读完整输入再启动命令。断言源读取请求和活跃缓冲区保持有界，不依赖脆弱的进程 RSS 精确值。假 mysql stdout 大量输出也不导致缓存增长。另测中途 reader 失败、child 非零退出和已取消/中途取消 context；等待子进程退出并清理所有 fixture。

**验证**：`go test -race ./internal/devplatform -run '^TestMySQLRestore' -count=1 -v` → 增量数据消费、取消和错误用例均通过；无子进程遗留

### 步骤 4: 检查回归边界

审查其他 mysqlSQL 调用保持原返回输出，Redis/MongoDB 分支与备份不改动。记录真实 MySQL 单行及中等文件 round trip 为后续隔离验收，使用现有 TestAdditionalEnginesIntegration 的专用安装根；不为了本计划自动安装四类数据库。完成本计划的自动门槛为 fake-process 集成测试加 go vet。

**验证**：`go vet ./...` → exit 0；diff 仅改变 MySQL restore 输入通路与新测试

## Test plan

新测试以实际子进程 stdin 为边界，不只断言使用某个 helper。至少覆盖源打开失败不建库、目录拒绝、完整数据摘要与 USE 前缀、参数顺序、分块提前消费、reader 错误、非零退出、大量输出、取消及进程回收。源读取失败发生在建库之后时可能保留部分目标，维持现有行为并明确记录；不假装 SQL restore 是事务回滚。

## Done criteria

以下全部满足才可记为 DONE：

- [x] TestMySQLRestore* 全部实际执行且通过，包括分块 gate 测试。
- [x] MySQL restore 分支不使用 os.ReadFile、io.ReadAll 或 dump 大字符串拼接。
- [x] 新 restore 命令不使用无界 CombinedOutput，敏感 SQL 不进错误/测试输出。
- [x] 打开源失败时零建库调用；取消后子进程被等待并退出。
- [x] 聚焦验证命令实际执行新增用例且通过；无零用例假通过。
- [x] `go vet ./...` 与 `git diff --check` 通过；未解决的原生验收限制如实记录。
- [x] `git status --short` 和相对执行起点的 diff 仅包含 Scope 内文件；不得顺手格式化整个仓库。
- [x] plans/README.md 中该计划状态及验证说明已更新。

## STOP conditions

- 当前代码与摘录不一致，且变化影响实施假设；先报告差异并刷新计划。
- 必须修改 Scope 外文件、引入依赖或扩大产品能力才能完成。
- 合理修正后同一个验证仍连续失败两次；记录命令与错误，不跳过门槛。
- 真实备份格式要求使用不可流式的第三方解析器，或必须改变数据库任务持久化模型。
- 发现恢复命令会选择不同目标数据库；先核实既有格式，不新增 SQL 重写器。
- 需要自动删除失败目标或覆盖已有数据库；这属于独立破坏性产品决定。

## Maintenance notes

mysqlSQL 仍用于小型查询；以后大数据路径不要重新套入字符串接口。新增可观测性时只记录阶段/字节数量，不记录 dump 内容。数据库权限、租约与恢复目标语义由原上层负责。
