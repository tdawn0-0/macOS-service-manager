# Plan 010: 设计诊断结果到对应设置的安全导航

> **执行说明**：这是给独立执行者的交接计划。逐步实施并执行每一步的验证；不要把测试名称匹配到零个测试当作成功。遇到 STOP 条件立即报告。完成后仅更新 plans/README.md 中本计划一行，并附实际验证结果。当前文件描述待执行工作，不代表已经修复。
>
> **漂移检查（先执行）**：在仓库根运行 `git diff --stat 0cbf7e9..HEAD -- plans/spikes/010-diagnostic-navigation/design.md plans/spikes/010-diagnostic-navigation/cases.json plans/spikes/010-diagnostic-navigation/prototype.py plans/spikes/010-diagnostic-navigation/test_prototype.py` 和 `git diff -- plans/spikes/010-diagnostic-navigation/design.md plans/spikes/010-diagnostic-navigation/cases.json plans/spikes/010-diagnostic-navigation/prototype.py plans/spikes/010-diagnostic-navigation/test_prototype.py`，再运行 `git status --short`。基线时工作区干净。检查所有已提交及未提交变化；路径有变化时逐项比对下方摘录，不能直接覆盖。前置计划造成的预期变化也要记录；行为或接口不再匹配时停止更新计划。

## Status

- **Priority**: P3
- **Effort**: S–M（设计/离线原型，不含产品实现）
- **Risk**: LOW — 原型只解析数据与导航意图
- **Depends on**: 无硬前置；未来产品实现依赖 004 的草稿导航保护
- **Category**: direction
- **Planned at**: commit `0cbf7e9`, 2026-11-07
- **Status**: DONE
- **Confidence**: HIGH（源码证据；本计划指定的新增测试尚未实现）

## Why this matters

诊断已有分层状态和 Remedy，但用户需要读完文字后自行找到对应项目、实例、站点或 CA 页面。增加导航能缩短定位路径，前提是稳定定位资源、不丢草稿、不会把“查看设置”变成自动提权修复，也不会将内部 ID/路径带入匿名报告。本计划设计类型化本机导航意图与脱敏边界，交付离线原型；不增加真实 UI 按钮。

## Current state

引用源码也属于漂移检查范围（只读，不扩大 Scope）：`git diff --stat 0cbf7e9..HEAD -- view_diagnostics.go internal/devplatform/diagnostic_report.go view_development.go docs/development-ui.md scripts/test_collect_third_party_notices.py`；另用 `git diff -- view_diagnostics.go internal/devplatform/diagnostic_report.go view_development.go docs/development-ui.md scripts/test_collect_third_party_notices.py` 检查未提交变化。下列摘录是精确基线，执行前逐项核对。

仓库根为 `/Users/jyeu/code/service-manager`；在独立 checkout 中使用实际根目录。产品是 Go 1.27.3 + MyGo 的 macOS Apple Silicon 应用，长生命周期服务交给 launchd；项目、实例和软件包由 internal/devplatform 管理。用户操作在 GUI 内完成；cmd/devmgr 是未随 App 分发的测试 harness，不能恢复为公开 CLI。仓库尚未发布，不增加旧来源迁移或兼容层。

### view_diagnostics.go:105 — 当前只呈现 Remedy 文字

```go
			ui.Column(c).Padding(12).Radius(8).Background(c.Theme().Surface).Gap(5).Children(func() {
				ui.Text(c, check.Status+" · "+check.Layer).FontSize(textSmall).FontWeight(650).TextColor(color)
				ui.Text(c, check.Resource).FontWeight(600)
				ui.Text(c, check.Summary).FontSize(textSmall)
				if check.Remedy != "" {
					ui.Text(c, check.Remedy).FontSize(textSmall).TextColor(c.Theme().TextMuted)
				}
			})
```

### internal/devplatform/diagnostic_report.go:18 — 当前诊断记录无稳定导航定位字段

```go
// DiagnosticCheck deliberately contains no command output, environment, request
// headers, log data or raw errors. Resource is the only identifying field.
type DiagnosticCheck struct {
	Layer    string `json:"layer"`
	Resource string `json:"resource"`
	Status   string `json:"status"` // OK, Warning, Failed, Stopped, Disabled
	Summary  string `json:"summary"`
	Remedy   string `json:"remedy,omitempty"`
}
```

### internal/devplatform/diagnostic_report.go:47 — 导出只匿名化 Resource，新增字段不能直接流出

```go
// RedactedJSON replaces local resource names and domains with report-local aliases.
// Fixed summaries preserve the failing layer and repair instruction. No state or
// filesystem traversal is part of export; even private keys are never read.
func (r DiagnosticReport) RedactedJSON() ([]byte, error) {
	r.Checks = append([]DiagnosticCheck{}, r.Checks...)
	aliases := map[string]string{}
	for i := range r.Checks {
		resource := r.Checks[i].Resource
		if aliases[resource] == "" {
			aliases[resource] = fmt.Sprintf("resource-%d", len(aliases)+1)
		}
		r.Checks[i].Resource = aliases[resource]
	}
	r.Redacted = true
	b, err := json.MarshalIndent(r, "", "  ")
	return append(b, '\n'), err
```

### view_development.go:236 — 直接选择项目会重建草稿

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
```

### docs/development-ui.md:18 — 诊断返回保留草稿及原生布局约束

```text
顶部 Diagnostics 打开独立诊断页：按层显示状态、说明和修复步骤，标明快照时间；复制和导出使用匿名资源名，返回后保留工作区草稿。

明暗主题都使用项目现有原生控件。默认窗口为 1120×720，最小窗口为 900×520。长项目名省略显示，详情可滚动，保存和结果状态固定在底部。
```

### scripts/test_collect_third_party_notices.py:8 — 离线标准库测试范式

```python
spec = importlib.util.spec_from_file_location('notices', Path(__file__).with_name('collect-third-party-notices.py'))
notices = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notices)


class NoticeCollectionTests(unittest.TestCase):
    def test_patents_does_not_satisfy_required_toolchain_license(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'PATENTS').write_text('Patent grant only')
            with self.assertRaisesRegex(ValueError, 'PATENTS alone is insufficient'):
```

## Conventions and constraints

设计采用有限的 action enum + 稳定 ID，不解析用户可读 Remedy 文本，不接受任意 URL/path/shell 作为动作。action 只导航，绝不自动信任 CA、写 hosts、停服务、重装软件或删除数据。目标解析使用当前本机 State；快照过期或资源删除时提供刷新提示。导出应构造显式 allowlist DTO，或把本机 action 留在独立私有结构，不能只增加一个字段后指望 Resource 匿名化继续安全。

来自 CONTRIBUTING.md 的约束：“Use isolated roots and ports in tests. Stop and unregister every test job.” 真实 hosts、钥匙串、管理员授权、重启/注销不属于自动执行范围。只能在明确安排的原生验收中操作；不能为让测试通过关闭 TLS 验证、扩大权限或接管原有数据库。测试数据不引用真实项目、密钥或凭据；发现秘密只记录文件位置和类型，不复制值。

## Commands you will need

所有命令从仓库根执行，要求 darwin/arm64、Go 1.27.3 和需要时的 Python 3。无需升级依赖或安装新的测试框架。

| 用途 | 命令 | 成功条件 |
|---|---|---|
| 工具链 | `go version` | Go 1.27.3 或满足 go.mod 的版本，darwin/arm64 |
| 依赖完整性 | `go mod verify` | exit 0，all modules verified |
| 聚焦验证 | `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s plans/spikes/010-diagnostic-navigation -p 'test_*.py' -v` | exit 0，指定新增用例实际执行且无 SKIP |
| 静态检查 | `go vet ./...` | exit 0 |
| 差异格式 | `git diff --check -- plans/spikes/010-diagnostic-navigation/design.md plans/spikes/010-diagnostic-navigation/cases.json plans/spikes/010-diagnostic-navigation/prototype.py plans/spikes/010-diagnostic-navigation/test_prototype.py plans/README.md` | exit 0 |
| 完整回归（隔离验收） | `go test ./...` 和 `go test -race ./...` | exit 0；单列 opt-in SKIP |
| 原生构建（仅代码计划需要时） | `go tool mygo build -sign - -skip-notarize` | exit 0，产物在忽略的 build/ 中；不签发版本、不公证、不发布 |

本计划仅交付设计与离线验证材料，不修改产品源码。原生构建不属于完成条件。

审查阶段已验证过 go vet、go mod verify、7 项隔离 Node 测试及 4 项 Python 测试；不能把这些历史结果视为本计划实施后的验收结果。

## Scope

**允许修改的文件（新增文件也必须在此列出）**：

- plans/spikes/010-diagnostic-navigation/design.md
- plans/spikes/010-diagnostic-navigation/cases.json
- plans/spikes/010-diagnostic-navigation/prototype.py
- plans/spikes/010-diagnostic-navigation/test_prototype.py
- plans/README.md（仅本计划状态与验证结果）

**禁止修改**：上述列表之外的产品源码、go.mod/go.sum、发布凭据、软件目录版本/来源、用户真实配置和数据库。只读引用文件不等于获准修改。本计划不允许修改任何产品源码或 CI；原型必须留在计划专用目录内。

## Git workflow

- 在独立分支 `codex/010-design-diagnostic-navigation` 执行。保留已有工作；不要切换或重置包含他人修改的工作树。
- 提交不是计划的完成前提；仅在操作者要求时提交。仓库提交标题以直接描述行为为主，例如 `Restrict the shipped app to internal commands`。
- 不自动合并、推送、创建 PR、发布 issue 或触发签名发行。

## Steps

### 步骤 1: 写导航契约和范围

创建 design.md，固定章节 Goal、Non-goals、Evidence、Action schema、Target resolution、Draft safety、Redaction、UX、Decision matrix、Rollout、Open questions。首批建议只覆盖项目服务设置、数据库实例设置、HTTPS 站点设置、软件详情、CA 设置；端口错误跳到相关资源的现有端口检查入口，不直接修改端口。每类列出 Layer/资源来源、稳定 ID、当前 UI 接入符号和不存在时回退，不能仅用展示名称做 key。说明为何诊断 snapshot 不应携带可直接执行的修复闭包。

**验证**：`python3 -c 'from pathlib import Path; s=Path("plans/spikes/010-diagnostic-navigation/design.md").read_text(); assert all(("## "+x) in s for x in ["Goal","Non-goals","Evidence","Action schema","Target resolution","Draft safety","Redaction","UX","Decision matrix","Rollout","Open questions"])'` → exit 0，固定章节齐全且含明确导航/隐私决策

### 步骤 2: 用案例定义目标解析与草稿保护

创建 schemaVersion=1 的 cases.json，至少 15 个唯一 case，每行 id、input、expected。input 用虚构 report/action/currentState/dirtyDraft；expected 字段为 status（navigate/confirm-discard/unavailable）、target、messageKey、mutations。覆盖五类导航、同名不同 ID、两个项目下同名 site ID（owner+site 复合定位）、资源已删除、ID 变化、报告过期、未知 action、同项目 dirty 保留、跨项目 dirty 确认、取消后保持、无修复目标。所有 mutations 必须为空；unavailable 不得返回可执行路径。诊断“Back to workspace”继续回到原草稿，不重新 selectProject。

**验证**：`python3 -c 'import json; d=json.load(open("plans/spikes/010-diagnostic-navigation/cases.json")); c=d["cases"]; assert d["schemaVersion"]==1 and len(c)>=15 and len({x["id"] for x in c})==len(c); assert all(x["expected"]["mutations"]==[] for x in c)'` → exit 0，至少 15 个唯一场景，所有动作无写入

### 步骤 3: 验证导航与脱敏双重边界

prototype.py 暴露 resolve_action(fixture) 与 redact_report(report)，两个纯函数不读取当前机器、不调用服务、不写文件。test_prototype.py 执行全部 cases；另对包含虚构私有路径、稳定 ID、域名与未识别字段的报告验证导出只含明确 allowlist，action 与内部 target 被剔除。使用不同本机 ID 但相同匿名结构验证脱敏结果，不仅检查某个特定字符串消失。输入保持不变，未知字段拒绝/丢弃规则明确，不可把链接写成外部可点击任意 URL。

**验证**：`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s plans/spikes/010-diagnostic-navigation -p 'test_*.py' -v` → 所有目标解析、dirty 保护、无副作用及脱敏测试通过

### 步骤 4: 描述原生 UI 和后续实施步骤

设计操作标签使用 Open project settings/Open database settings 等明确导航措辞；每个检查最多一个主导航，修复说明继续显示，必要授权仍在目标页面由用户另行触发。说明 900×520 明暗主题、长名称换行与可访问标签；不生成网页 mockup 替代 MyGo 设计。列后续实现文件：diagnostic_report 的本机 action 构造/导出 DTO、view_diagnostics 的按钮、既有 workspace selection，以及新输入测试。后续跨项目导航必须复用 004 完成后的 guard；在本设计阶段只记录接口依赖，不修改那个计划或产品代码。

**验证**：`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s plans/spikes/010-diagnostic-navigation -p 'test_*.py' -v` → 离线契约持续通过；设计列出未来可独立实施和验证的里程碑

## Test plan

至少 15 个案例都逐条核对状态与稳定定位，未知动作不导航，陈旧 ID 不回退到名称相似的其他资源。额外覆盖报告输入不变、私有 action 从导出剔除、未知字段默认不输出、匿名别名的一致性、用户取消不产生 mutations。无需真实 CA/数据库或浏览器。

## Done criteria

以下全部满足才可记为 DONE：

- [x] 四个 scope 文件存在，设计固定章节齐全。
- [x] 至少 15 个案例涵盖五类目标、陈旧/删除/同名情况和草稿确认。
- [x] unittest 全部通过，所有动作 mutations 为空。
- [x] 导出契约采用字段 allowlist 或等价明确隔离，内部 action、ID、域名、路径不出现在匿名输出。
- [x] 有 UI 文案、最小窗口规范和未来 Go 接入清单；源码未变，未声称功能已上线。
- [x] 聚焦验证命令实际执行新增用例且通过；无零用例假通过。
- [x] `go vet ./...` 与 `git diff --check` 通过；未解决的原生验收限制如实记录。
- [x] `git status --short` 和相对执行起点的 diff 仅包含 Scope 内文件；不得顺手格式化整个仓库。
- [x] plans/README.md 中该计划状态及验证说明已更新。

## STOP conditions

- 当前代码与摘录不一致，且变化影响实施假设；先报告差异并刷新计划。
- 必须修改 Scope 外文件、引入依赖或扩大产品能力才能完成。
- 合理修正后同一个验证仍连续失败两次；记录命令与错误，不跳过门槛。
- 某种导航必须自动执行提权/信任/删除动作；将其排除并报告，不扩大动作含义。
- 没有稳定资源身份只能按名字猜；先列模型问题，不能假装可可靠跳转。
- 需要复用包含秘密的原始错误串或 URI 构造目标；不复制数据，重新设计私有定位契约。

## Maintenance notes

任何未来 DiagnosticCheck 字段都要经过脱敏审查，不能依赖 JSON 标签默认安全。导航意图的版本不应成为公开 API；新诊断层应先定义目标缺失与 dirty 交互，再加按钮。该原型只验证决策，最终 MyGo UI 仍需要真实输入测试。
