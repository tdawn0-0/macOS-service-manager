# Plan 012: 让 Node.js 安装后可独立在终端使用

> 执行者：本文件是独立交接计划。按步骤实施并验证；仅更新自己的状态记录。先读完 Scope 与 STOP conditions。计划作者本轮没有实施源码。
>
> 漂移检查：`git diff --stat 2ec48ab..HEAD -- 'internal/devplatform/node_manage.go' 'internal/devplatform/node_test.go' 'internal/devplatform/model.go' 'internal/devplatform/packages.go' 'internal/devplatform/package_repair.go' 'internal/devplatform/runtime_terminal_test.go' 'view_runtime.go' 'view_development.go' 'view_runtime_terminal.go' 'runtime_terminal_ui_test.go' 'node_e2e_test.go'`，另运行 `git status --short`。本文件摘录描述规划基线。前置计划造成的预期变化须逐项记录在本文件“Execution record”，对照本文明确的前置契约继续；其他无法解释的漂移停止报告。不要为恢复摘录而撤销前置计划。

## Status

- Priority: P1
- Effort: M（包含测试的粗估）
- Risk: MED
- Depends on: 011 DONE；015-A 已定义 AC-RUNTIME
- Category: direction
- Planned at: commit `2ec48ab`, 2026-11-08
- Status: DONE（隔离工作树）

## Product contract

用户已决定取消注册项目：只管理软件版本、独立数据库实例和 HTTPS 反代。HTTPS 页面仅配置“域名 → 本地端口”规则，不设独立站点实体、详情页或生命周期。应用由用户在终端或 IDE 启动；App 不再管理源码目录、应用命令、依赖安装、编排、项目自启或环境导入导出。Node 按目录读取版本声明仍保留，不要求注册目录。保留现有系统服务管理为次要入口。

不做旧项目 schema、文件、任务或配置的兼容和迁移。开发验收使用新建隔离存储；这不授权清空用户真实数据库、卸载共享 Homebrew keg 或处理旧机器上的遗留进程。软件范围为 Node.js、MySQL、PostgreSQL、Redis、MongoDB 五类；PostgreSQL=Homebrew core，其余四类校验归档。删除 PHP 安装、版本选择、仓库添加和终端集成，不操作机器已有 PHP；macOS arm64、Go/MyGo 技术栈不变。界面主入口为 Software / Databases / HTTPS。尚未发布，任何新状态格式不需要迁移器，但各计划必须维持同一新格式并集成新增字段。

## Current state

以下为旧代码基线摘录，不是保留 PHP 或站点管理的目标要求。

`view_development.go:1094`：

```go
			if p.Name == "node" {
				if id == d.state.NodeDefault {
					ui.Text(c, "Default").FontSize(textSmall).FontWeight(600).TextColor(c.Theme().Accent).Shrink(0)
				} else if ui.Button(c, "Set default "+p.Version).Disabled(d.busy).Clicked() {
					a.devTask(func(m *devplatform.Manager) (string, error) {
						return "Default is Node.js " + p.Version, m.SetNodeDefault(m.Context, id)
					})
				}
```

`view_development.go:1118`：

```go
	if d.software == "node" {
		a.devNodeTerminal(c)
	}
```

`internal/devplatform/node_manage.go:184`：

```go
// SelectNode chooses the version for a directory. override, when set, beats
// the project's files, which beat the default.
func (m *Manager) SelectNode(dir, override string) (NodeSelection, error) {
	s, e := m.Read()
	if e != nil {
		return NodeSelection{}, e
	}
	installed := installedNodes(s, m.cachedNodeReleases())
	spec, source := strings.TrimSpace(override), "the requested version"
	if spec == "" {
		spec, source = NodeVersionFile(dir)
	}
	if spec == "" {
		for _, n := range installed {
			if n.ID == s.NodeDefault {
				return m.selectionOf(n, "", "default"), nil
			}
		}
		return NodeSelection{}, ErrNoNodeDefault
	}
	if strings.EqualFold(spec, "system") {
		return NodeSelection{Spec: spec, Source: source, System: true}, nil
```

`internal/devplatform/node_manage.go:363`：

```go
func (c nodeShellConfiguration) commit(data []byte) error {
	current, e := readNodeShellConfiguration(c.rc)
	if e != nil {
		return fmt.Errorf("shell configuration changed; review it and retry: %w", e)
	}
	same := c.target == current.target && c.mode == current.mode && bytes.Equal(c.data, current.data)
	if c.entry == nil {
		same = same && current.entry == nil
	} else {
		same = same && current.entry != nil && os.SameFile(c.entry, current.entry)
	}
	if c.file == nil {
		same = same && current.file == nil
	} else {
		same = same && current.file != nil && os.SameFile(c.file, current.file)
	}
	if !same {
		return errors.New("shell configuration changed; review it and retry")
	}
	return atomicWrite(c.target, data, c.mode)
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

- `internal/devplatform/node_manage.go`
- `internal/devplatform/node_test.go`
- `internal/devplatform/model.go`
- `internal/devplatform/packages.go`
- `internal/devplatform/package_repair.go`
- `internal/devplatform/runtime_terminal_test.go`
- `view_runtime.go`
- `view_development.go`
- `view_runtime_terminal.go`
- `runtime_terminal_ui_test.go`
- `node_e2e_test.go`

每份计划允许更新自身的 Execution record 和 `plans/README.md` 自己的状态行。新文件只可在列出的命名范围内创建。不要修改其他计划的范围/状态。

## Git workflow

在执行者的隔离 checkout/worktree 实施，分支名 `codex/012-standalone-runtime-terminal`；开始记下执行基线与工作区状态，不覆盖用户修改。提交风格沿用仓库 `feat:` / `refactor:` / `test:` / `docs:`。按逻辑单元交付可审查 diff；是否提交由执行任务授权决定，不自动合并、推送、创建 PR 或发布。

## Why this matters

取消项目后，Node.js 的安装、默认版本和终端使用需要各自清楚的状态。现有目录版本解析不需要项目注册，可以直接保留。PHP 已退出产品范围，不再为它创建默认版本、shim 或 shell 配置。

## Target and boundaries

- 保持 Node 当前声明优先级：显式选择 > 最近目录 `.node-version` / `.nvmrc` / `engines.node` > NodeDefault；不注册源码目录、不写仓库文件。
- 安装与终端启用分开；首次安装不自动写 shell 配置。提供 Copy executable path / Set default / Enable or Disable in terminal，并分别展示已安装、默认版本、终端启用状态。
- 仅自动配置 zsh；保留现有 Node shell 的幂等、symlink、文件身份和并发编辑保护。不提取只为已取消 PHP 复用的抽象。
- 说明启用/关闭在新终端生效，已运行 Node 进程不会随默认版本变化。不改变原有目录切换与错误行为，不额外承诺 fail-closed。
- 删除默认版本时清除其引用，不自动选另一个版本；保留 global npm 前缀及现有包卸载保护。缺失二进制显示不可用，不能把安装成功当成终端已可用。

**Out of scope**：PHP 的全部版本/终端能力（011 负责删除现有入口）、软件 catalog 增项、项目包管理器、终端命令执行器、用户 dotfile 无关内容、shell 进程管理、真实机器 shell 设置。

## Steps

### Step 1: 固定 Node 独立选择契约

核对 011 已删除项目绑定，但保留 SelectNode、SetNodeDefault 和 NodeVersionFile。用临时安装目录验证显式版本、最近目录声明、默认和 system 声明；没有任何 Project 注册。默认缺失、无匹配版本、错误软件类型与状态写失败时保持已有错误语义，不静默创建配置。

**Verify**：新增 `TestRuntimeTerminalSelection` 于 `runtime_terminal_test.go`；`go test ./internal/devplatform -list '^TestRuntimeTerminalSelection$'` 列出它；`go test -race ./internal/devplatform -run '^(TestRuntimeTerminalSelection|TestSelectNodeFollowsOverrideFilesThenDefault)$' -count=1 -v` → 无网络临时夹具 PASS。

### Step 2: 保留显式 shell 启用与安全移除

沿用 `node_manage.go` 的 readNodeShellConfiguration / commit，不建立通用 shell 框架。检查重复启用、只移除自有块、有效/失效 symlink、并发内容/身份变化、权限与路径引用。新用例归入 `node_test.go` 的 `TestNodeTerminalConfiguration`，在临时 HOME/ZDOTDIR 下执行 zsh，禁止读取或改写用户实际 rc。

**Verify**：`go test -race ./internal/devplatform -run '^(TestNodeTerminalConfiguration|TestNodeShell.*)$' -count=1 -v` → 全部 PASS。测试生成脚本以 `zsh -n` 校验，并断言取消/失败不修改 dotfile；不创建 PHPDefault、php launcher 或 PHP 标记块。

### Step 3: 替换软件完成动作并接入 GUI

在 `view_runtime_terminal.go` 展示 Node 默认精确版本、终端启用状态和 executable path；清除原项目“使用此版本”跳转。Copy 使用已验证 binary；失败保留选择，启用需明确点击。其余四种软件仅展示安装与数据库相关动作，不提供 Node 的终端切换控件。

**Verify**：新增 `TestUIRuntimeTerminal` 于 `runtime_terminal_ui_test.go`；`go test -race . -run '^TestUIRuntimeTerminal$' -count=1 -v` → mock Home/Run 下状态独立、缺失版本、复制路径、取消/失败、无 PHP 控件均 PASS；`go vet ./...` → exit 0。

### Step 4: 验证卸载与界面

用两个临时 Node 版本验证默认移除、重新设定和新 shell 解析；已有 global npm 行为不变。UI suite 的 `layouts` 子用例在四种尺寸/主题绘制，检查长路径和操作按钮；`MSM_RESOURCE_SHOTS` 指定本次截图目录。

**Verify**：`go test -race ./internal/devplatform -run '^(TestRuntimeTerminalSelection|TestNodeTerminalConfiguration|TestUninstallingNodeClearsDefaultAndGlobalPackages)$' -count=1 -v` → PASS；`MSM_RESOURCE_SHOTS=build/resource-shots/012 go test . -run '^TestUIRuntimeTerminal/layouts$' -count=1` → 四张可读截图；`git diff --check` → exit 0。

## Test plan

模型/链接测试使用 testManager 与临时归档 fixture；模式参考 node_test.go 的 TestNodeShellSymlinkLifecycle、TestNodeShellRejectsChangedConfiguration。覆盖两个版本共存、取消不改 dotfile、默认移除和目录选择。015-B 用真实 Node 验证新隔离 zsh 的 executable/version；本计划 mock 结果不算原生通过。

## Done criteria

- [ ] 三个新增 suite 实际执行且零 SKIP；既有 Node shell/selection 回归 PASS。
- [ ] `go test ./... -run '^$'`、`go vet ./...`、`git diff --check` 均 exit 0。
- [ ] 四种 UI 截图已检查，默认与启用两个状态可区分；界面无 PHP 安装或版本操作。
- [ ] mock 命令捕获中没有 Homebrew link/unlink、PHP 调用或第三方版本管理器配置改写。
- [ ] Scope 与本机 HOME 未被测试越界写入；原生未执行时明确留待 015-B。

## Maintenance notes

只维护 Node 的终端契约。未来新增运行时需要新的产品决策，不恢复本轮取消的 PHPDefault 或 PHP shell 集成。审查重点是目录声明优先级、dotfile 幂等及默认版本移除后的可解释状态。

## Non-vacuous verification gate

以下是 Done 的必需补充门槛。`plans/check-test-events.py` 已随计划提供，只读取 stdin 的 Go JSON 事件；拒绝缺失完整测试名、SKIP、FAIL、重复或不完整包事件。不要只凭 exit 0 或顶层 -list 判断子用例通过。先执行 `set -o pipefail`，确保管道前后任一失败都失败。

```sh
set -o pipefail
go test -json -race ./internal/devplatform -run '^(TestRuntimeTerminalSelection|TestNodeTerminalConfiguration)$' -count=1 | python3 plans/check-test-events.py --package macos-service-manager/internal/devplatform --require TestRuntimeTerminalSelection --require TestNodeTerminalConfiguration
MSM_RESOURCE_SHOTS=build/resource-shots/012 go test -json -race . -run '^TestUIRuntimeTerminal$' -count=1 | python3 plans/check-test-events.py --package macos-service-manager --require TestUIRuntimeTerminal --require TestUIRuntimeTerminal/layouts --shot-dir build/resource-shots/012
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

012 执行记录：隔离工作树 `/Users/jyeu/code/service-manager-012`，提交 `60c1b3a`（基于 011 `f987c0c`）。六个 Scope 文件；全量 Go、vet、聚焦 race 事件门槛、四张本次截图检查、diff check PASS。原生 runner 尚未执行，留给 015-B。

集成记录：按用户“合并 master”的明确指示，实现提交链已于本轮快进合入 master（`96549ea`）。此前工作树记录保留为历史；未验证的原生安装、外部卷与系统信任项目仍保持原状态。
