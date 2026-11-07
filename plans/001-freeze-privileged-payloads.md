# Plan 001: 冻结管理员批准的任务与 hosts 内容

> **执行说明**：这是给独立执行者的交接计划。逐步实施并执行每一步的验证；不要把测试名称匹配到零个测试当作成功。遇到 STOP 条件立即报告。完成后仅更新 plans/README.md 中本计划一行，并附实际验证结果。当前文件描述待执行工作，不代表已经修复。
>
> **漂移检查（先执行）**：在仓库根运行 `git diff --stat 0cbf7e9..HEAD -- internal/launchd/edit.go internal/launchd/root_test.go internal/devplatform/proxy.go internal/devplatform/hosts_authorization_test.go` 和 `git diff -- internal/launchd/edit.go internal/launchd/root_test.go internal/devplatform/proxy.go internal/devplatform/hosts_authorization_test.go`，再运行 `git status --short`。基线时工作区干净。检查所有已提交及未提交变化；路径有变化时逐项比对下方摘录，不能直接覆盖。前置计划造成的预期变化也要记录；行为或接口不再匹配时停止更新计划。

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED — 需保留安装顺序、权限及失败语义
- **Depends on**: 无
- **Category**: security
- **Planned at**: commit `0cbf7e9`, 2026-11-07
- **Status**: DONE
- **Confidence**: HIGH（源码证据；本计划指定的新增测试尚未实现）

## Why this matters

任务安装先验证用户拥有的临时 plist，却只把临时路径交给管理员命令；hosts 写入同样在授权后读取用户可改的文件。本地同用户进程可在合法授权等待期间改变这些字节，系统 daemon 随后可能执行不同于批准内容的程序。修复要绑定批准时的内容，不改变哪些操作需要授权；本计划没有远程或无需授权攻击的假设。

## Current state

引用源码也属于漂移检查范围（只读，不扩大 Scope）：`git diff --stat 0cbf7e9..HEAD -- internal/launchd/edit.go internal/devplatform/proxy.go internal/devplatform/network.go internal/devplatform/network_test.go`；另用 `git diff -- internal/launchd/edit.go internal/devplatform/proxy.go internal/devplatform/network.go internal/devplatform/network_test.go` 检查未提交变化。下列摘录是精确基线，执行前逐项核对。

仓库根为 `/Users/jyeu/code/service-manager`；在独立 checkout 中使用实际根目录。产品是 Go 1.27.3 + MyGo 的 macOS Apple Silicon 应用，长生命周期服务交给 launchd；项目、实例和软件包由 internal/devplatform 管理。用户操作在 GUI 内完成；cmd/devmgr 是未随 App 分发的测试 harness，不能恢复为公开 CLI。仓库尚未发布，不增加旧来源迁移或兼容层。

### internal/launchd/edit.go:408 — 任务内容与管理员命令之间的边界

```go
	data, err := s.Marshal(current)
	if err != nil {
		return "", err
	}
	tmp, err := writeChecked(os.TempDir(), data)
	if err != nil {
		return "", err
	}
	defer os.Remove(tmp)
	// Agents load into the user's domain, for which the user needs no root.
	inScript := load && p.Elevated
	target := p.Domain + "/" + s.Label
	script := installScript(tmp, path, p.Domain, inScript)
	if existing != "" {
		script = replaceScript(tmp, path, p.Domain, target, inScript)
	}
	if _, err := system.RunAdmin("/bin/sh", "-c", script); err != nil {
```

### internal/launchd/edit.go:445 — 安装与替换脚本读取临时路径

```go
// installScript is what root runs for a new job: it never replaces a job that
// appeared since the check, and launchd refuses a file that root does not own.
func installScript(tmp, path, domain string, load bool) string {
	script := "[ ! -e " + system.ShellQuote(path) + " ]" +
		" && /bin/mkdir -p " + system.ShellQuote(filepath.Dir(path)) +
		" && /usr/bin/install -S -m 644 -o root -g wheel " + system.ShellQuote(tmp) + " " + system.ShellQuote(path)
	if load {
		script += " && /bin/launchctl bootstrap " + system.ShellQuote(domain) + " " + system.ShellQuote(path)
	}
	return script
}

// replaceScript is what root runs for an edit. The job stays as it is until the
// new file is in place, which replaces the old one all at once, and a loaded job
// keeps what it was loaded with until it loads again.
func replaceScript(tmp, path, domain, target string, load bool) string {
	script := "[ -f " + system.ShellQuote(path) + " ]" +
		" && /usr/bin/install -S -m 644 -o root -g wheel " + system.ShellQuote(tmp) + " " + system.ShellQuote(path)
	if load {
		script += " && { " + unloadScript(target) + " /bin/launchctl bootstrap " + system.ShellQuote(domain) + " " + system.ShellQuote(path) + "; }"
	}
```

### internal/devplatform/proxy.go:346 — hosts 授权读取用户临时文件

```go
		tmp, e := os.CreateTemp("", "msm-hosts-")
		if e != nil {
			return e
		}
		defer os.Remove(tmp.Name())
		if _, e = tmp.WriteString(updated); e != nil {
			tmp.Close()
			return e
		}
		tmp.Close()
		// Check the original hash in the privileged operation before replacement.
		sum := sha1.Sum(old)
		script := "test \"$(/usr/bin/shasum /etc/hosts | /usr/bin/cut -d ' ' -f 1)\" = " + system.ShellQuote(hex.EncodeToString(sum[:])) + " && /bin/cat " + system.ShellQuote(tmp.Name()) + " > /etc/hosts && /usr/bin/dscacheutil -flushcache"
		_, e = system.RunAdmin("/bin/sh", "-c", script)
```

### internal/devplatform/network.go:77 — 现有不可变 payload 范式

```go
func socketInstallScript(data []byte, path, target string) string {
	// Embed immutable bytes in the authorized command. No user-writable staging
	// file is read as root after the authorization dialog.
	encoded := base64.StdEncoding.EncodeToString(data)
	return "set -eu; tmp=$(/usr/bin/mktemp /Library/LaunchDaemons/.msm-socket.XXXXXX); trap '/bin/rm -f \"$tmp\"' EXIT; /usr/bin/printf '%s' " + system.ShellQuote(encoded) + " | /usr/bin/base64 -D > \"$tmp\"; /usr/bin/plutil -lint \"$tmp\" >/dev/null; /usr/sbin/chown root:wheel \"$tmp\"; /bin/chmod 644 \"$tmp\"; if /bin/launchctl print " + system.ShellQuote(target) + " >/dev/null 2>&1; then /bin/launchctl bootout " + system.ShellQuote(target) + "; fi; /bin/mv -f \"$tmp\" " + system.ShellQuote(path) + "; /bin/launchctl bootstrap system " + system.ShellQuote(path)
```

### internal/devplatform/network_test.go:36 — 仅检查授权脚本、不执行提权的测试范式

```go
func TestAuthorizedSocketInstallEmbedsImmutableConfiguration(t *testing.T) {
	payload := []byte("config with 'quoted' names and $(shell)")
	script := socketInstallScript(payload, "/Library/LaunchDaemons/example.plist", "system/example")
	if strings.Contains(script, "$(shell)") || strings.Contains(script, "config with") {
		t.Fatal("configuration injected into shell")
	}
	if !strings.Contains(script, "/Library/LaunchDaemons/.msm-socket.") || !strings.Contains(script, "root:wheel") || !strings.Contains(script, "bootstrap system") {
		t.Fatal("missing secure root install steps")
	}
```

## Conventions and constraints

复用 socketInstallScript 的思路，不导入 devplatform 到 launchd（会破坏分层）。两个包各自的小型脚本构造函数即可；不要为两处调用建立通用权限框架。参数使用 system.ShellQuote；配置以 base64 数据嵌入命令，不作为 shell 源码解释。保留 SaveAsRoot 的系统 agent 由普通用户加载、daemon 在管理员命令中加载、取消时不保存、已保存但加载失败返回路径等语义。现有 root_test.go 的部分测试解析旧临时路径，须更新为验证解码后的内容。

来自 CONTRIBUTING.md 的约束：“Use isolated roots and ports in tests. Stop and unregister every test job.” 真实 hosts、钥匙串、管理员授权、重启/注销不属于自动执行范围。只能在明确安排的原生验收中操作；不能为让测试通过关闭 TLS 验证、扩大权限或接管原有数据库。测试数据不引用真实项目、密钥或凭据；发现秘密只记录文件位置和类型，不复制值。

## Commands you will need

所有命令从仓库根执行，要求 darwin/arm64、Go 1.27.3 和需要时的 Python 3。无需升级依赖或安装新的测试框架。

| 用途 | 命令 | 成功条件 |
|---|---|---|
| 工具链 | `go version` | Go 1.27.3 或满足 go.mod 的版本，darwin/arm64 |
| 依赖完整性 | `go mod verify` | exit 0，all modules verified |
| 聚焦验证 | `go test -race ./internal/launchd ./internal/devplatform -run '^TestAuthorized(Job\|Hosts)Payload' -count=1 -v` | exit 0，指定新增用例实际执行且无 SKIP |
| 静态检查 | `go vet ./...` | exit 0 |
| 差异格式 | `git diff --check -- internal/launchd/edit.go internal/launchd/root_test.go internal/devplatform/proxy.go internal/devplatform/hosts_authorization_test.go plans/README.md` | exit 0 |
| 完整回归（隔离验收） | `go test ./...` 和 `go test -race ./...` | exit 0；单列 opt-in SKIP |
| 原生构建（仅代码计划需要时） | `go tool mygo build -sign - -skip-notarize` | exit 0，产物在忽略的 build/ 中；不签发版本、不公证、不发布 |

本计划的必要门槛是下列聚焦测试、go vet 和差异检查。完整 go test 包含真实 launchd 启停；仅在隔离、可清理的 macOS 验证会话中运行，并记录所有 SKIP。

审查阶段已验证过 go vet、go mod verify、7 项隔离 Node 测试及 4 项 Python 测试；不能把这些历史结果视为本计划实施后的验收结果。

## Scope

**允许修改的文件（新增文件也必须在此列出）**：

- internal/launchd/edit.go
- internal/launchd/root_test.go
- internal/devplatform/proxy.go
- internal/devplatform/hosts_authorization_test.go
- plans/README.md（仅本计划状态与验证结果）

**禁止修改**：上述列表之外的产品源码、go.mod/go.sum、发布凭据、软件目录版本/来源、用户真实配置和数据库。只读引用文件不等于获准修改。

## Git workflow

- 在独立分支 `codex/001-freeze-privileged-payloads` 执行。保留已有工作；不要切换或重置包含他人修改的工作树。
- 提交不是计划的完成前提；仅在操作者要求时提交。仓库提交标题以直接描述行为为主，例如 `Restrict the shipped app to internal commands`。
- 不自动合并、推送、创建 PR、发布 issue 或触发签名发行。

## Steps

### 步骤 1: 固定既有行为与安全回归

在 root_test.go 增加 TestAuthorizedJobPayload，并在新 hosts_authorization_test.go 增加 TestAuthorizedHostsPayload。拦截 system.RunAdmin，保存参数，使用普通示例字节比较批准内容；不得执行任何实际管理员命令。覆盖新建/编辑、load true/false、不同引号与 Unicode 的无害字段。编写断言：授权参数包含可解码的确切 payload，不包含需在批准后读取的用户 staging 路径。当前实现应在该断言失败。所有全局 stub 用 t.Cleanup 还原，不用 t.Parallel。

**验证**：`go test ./internal/launchd ./internal/devplatform -run '^TestAuthorized(Job|Hosts)Payload' -count=1 -v` → 新增用例实际执行，改实现前仅在 payload 边界断言失败，不能因编译错误失败

### 步骤 2: 修改任务安装脚本

让 installScript/replaceScript 接收批准的序列化字节。继续在授权前校验 plist；授权脚本内用 root 在目标受保护目录创建 staging，trap 清理，以 printf + base64 解码出数据，再运行 plutil、设置 0644/root:wheel 并按原顺序安装。新建必须保留目标已存在拒绝；编辑保留卸载失败不继续加载的行为。目标路径若变为符号链接或非普通文件，应拒绝。不要在脚本执行时回读用户临时 plist。用既有语义测试替换对旧命令字符串细节的依赖，脚本用 /bin/sh -n 检查而不执行。若编码后参数超过受支持的命令长度，应明确报错，不降级到可变文件。

**验证**：`go test ./internal/launchd -run 'Test(AuthorizedJobPayload|EditingADaemon|EditingASystemAgent|SavingOverWhatIsNotAFile|AFailureSays|AReplacedFile)' -count=1 -v` → 所有命中测试通过，批准内容与最终待安装内容相同

### 步骤 3: 修改 hosts 写入并保留并发检查

在 ConfigureHosts 完成 HostsContent 和备份之后，直接把 updated 字节及旧内容摘要交给局部脚本构造函数。管理员脚本在受保护的 hosts 所在目录中创建 staging，解码固定字节；检查当前 hosts 摘要仍匹配再替换，保留文件权限/所有者与刷新缓存行为。无变化不请求授权，冲突不覆盖，取消不写。新测试直接测试构造函数与模拟 RunAdmin；不要为测试读取、替换真实 /etc/hosts。为临时目录准备的无害 payload 只验证数据解析与 shell 语法。

**验证**：`go test ./internal/devplatform -run 'Test(AuthorizedHostsPayload|HostsPreserveAndConflict|AuthorizedSocketInstallEmbedsImmutableConfiguration)' -count=1 -v` → 通过；原 hosts 保留/冲突测试不回归

### 步骤 4: 完成回归

运行聚焦 race 测试和 go vet。检视 diff 中无新增任意脚本、命令输出或真实秘密记录。安全回归必须验证数据内容，不能只检查出现 base64 这个字符串。管理员实机验收另行安排；本计划完成可以基于无提权的精确脚本与行为验证，但必须说明未执行实机授权。

**验证**：`go test -race ./internal/launchd ./internal/devplatform -run 'Test(Authorized|HostsPreserve|EditingADaemon|EditingASystemAgent|AFailureSays|AReplacedFile)' -count=1 -v` → 通过，所有测试均使用拦截器或纯数据，不弹出授权

## Test plan

新增 TestAuthorizedJobPayload 和 TestAuthorizedHostsPayload 均应有内容一致、无害特殊字符、失败/取消、冲突拒绝子用例。仿照 network_test.go 的 TestAuthorizedSocketInstallEmbedsImmutableConfiguration 和 root_test.go 的 stubRoot；保留既有“保存成功但加载失败”断言。通过模拟的旧 staging 内容变化证明授权参数不依赖它；不提供或执行利用代码。

## Done criteria

以下全部满足才可记为 DONE：

- [x] 两个新增测试组均执行且通过；批准后的数据来源不再是用户临时文件。
- [x] 新建/编辑与 agent/daemon 权限分工、授权取消和加载失败语义的既有测试通过。
- [x] hosts 摘要冲突拒绝与无变化不授权的断言通过。
- [x] 所有生成脚本通过 /bin/sh -n；测试不执行真实 root 安装。
- [x] 聚焦验证命令实际执行新增用例且通过；无零用例假通过。
- [x] `go vet ./...` 与 `git diff --check` 通过；未解决的原生验收限制如实记录。
- [x] `git status --short` 和相对执行起点的 diff 仅包含 Scope 内文件；不得顺手格式化整个仓库。
- [x] plans/README.md 中该计划状态及验证说明已更新。

## STOP conditions

- 当前代码与摘录不一致，且变化影响实施假设；先报告差异并刷新计划。
- 必须修改 Scope 外文件、引入依赖或扩大产品能力才能完成。
- 合理修正后同一个验证仍连续失败两次；记录命令与错误，不跳过门槛。
- 为解决参数长度问题需要新 helper、持久特权服务或更改授权 API。
- 发现现有 hosts 或目标目录的特殊链接/权限语义无法用现有行为安全保留。
- 测试必须执行真实提权才能验证；改用捕获脚本/注入边界，否则停止。

## Maintenance notes

重点审查：base64 是数据传输方式而不是额外信任来源；批准时的字节必须不可变。新增其他管理员写文件功能时沿用此边界。不要把 root staging 放回 TMPDIR；该目录不能提供所需的权限边界。
