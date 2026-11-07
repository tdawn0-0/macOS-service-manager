# Plan 009: 设计符合项目工具链的依赖安装流程

> **执行说明**：这是给独立执行者的交接计划。逐步实施并执行每一步的验证；不要把测试名称匹配到零个测试当作成功。遇到 STOP 条件立即报告。完成后仅更新 plans/README.md 中本计划一行，并附实际验证结果。当前文件描述待执行工作，不代表已经修复。
>
> **漂移检查（先执行）**：在仓库根运行 `git diff --stat 0cbf7e9..HEAD -- plans/spikes/009-package-managers/design.md plans/spikes/009-package-managers/cases.json plans/spikes/009-package-managers/prototype.py plans/spikes/009-package-managers/test_prototype.py` 和 `git diff -- plans/spikes/009-package-managers/design.md plans/spikes/009-package-managers/cases.json plans/spikes/009-package-managers/prototype.py plans/spikes/009-package-managers/test_prototype.py`，再运行 `git status --short`。基线时工作区干净。检查所有已提交及未提交变化；路径有变化时逐项比对下方摘录，不能直接覆盖。前置计划造成的预期变化也要记录；行为或接口不再匹配时停止更新计划。

## Status

- **Priority**: P3
- **Effort**: M（设计/离线原型，不含产品实现）
- **Risk**: LOW — 不运行包管理器；未来实现风险需在设计中列明
- **Depends on**: 无硬前置；未来 GUI 实现应在 004 草稿保护之后
- **Category**: direction
- **Planned at**: commit `0cbf7e9`, 2026-11-07
- **Status**: TODO
- **Confidence**: HIGH（源码证据；本计划指定的新增测试尚未实现）

## Why this matters

当前“Install project dependencies”只检查 package.json 存在，随后固定运行 npm ci。已有项目命令后台任务足以承接不同 argv，但 pnpm/yarn 项目、缺少 npm 锁文件、冲突锁文件和管理器版本来源需要明确产品决策。本计划产出设计和离线决策原型，以便判断收益/复杂度，不直接接入产品或安装任何包管理器。

## Current state

引用源码也属于漂移检查范围（只读，不扩大 Scope）：`git diff --stat 0cbf7e9..HEAD -- view_development.go internal/devplatform/operations.go docs/local-dev-platform-plan.md scripts/test_collect_third_party_notices.py`；另用 `git diff -- view_development.go internal/devplatform/operations.go docs/local-dev-platform-plan.md scripts/test_collect_third_party_notices.py` 检查未提交变化。下列摘录是精确基线，执行前逐项核对。

仓库根为 `/Users/jyeu/code/service-manager`；在独立 checkout 中使用实际根目录。产品是 Go 1.27.3 + MyGo 的 macOS Apple Silicon 应用，长生命周期服务交给 launchd；项目、实例和软件包由 internal/devplatform 管理。用户操作在 GUI 内完成；cmd/devmgr 是未随 App 分发的测试 harness，不能恢复为公开 CLI。仓库尚未发布，不增加旧来源迁移或兼容层。

### view_development.go:839 — 当前固定 npm ci 的按钮

```go
		if _, e := os.Stat(filepath.Join(d.draft.Directory, "package.json")); e == nil {
			if ui.Button(c, "Install project dependencies").AlignSelf(ui.Start).Tooltip("Runs npm ci in the saved environment, including repository lifecycle scripts. Source and dependency changes remain after cancellation.").Disabled(d.busy || a.projectTaskRunning(d.id) || d.projectDirty()).Clicked() {
				id := d.id
				a.startProjectOperation(d.state.Projects[id], "dependencies", func(m *devplatform.Manager) (string, error) { return m.Exec(m.Context, id, []string{"npm", "ci"}) })
			}
```

### internal/devplatform/operations.go:470 — 可复用的后台命令入口

```go
func (m *Manager) Exec(ctx context.Context, id string, args []string) (string, error) {
	var out string
	err := m.runOperation(ctx, OperationRequest{Kind: "exec", Resource: id}, "", func(w *Manager) error {
		w.operation.phase("Executing")
		var e error
		out, e = w.runCommandWork(ctx, id, args)
		return e
	})
	return out, err
```

### docs/local-dev-platform-plan.md:90 — 用户主动执行、npm ci 只是示例

```text
### 5.2 配置环境

1. 用户选择运行时版本和数据库/缓存实例。
2. 应用展示缺失软件、下载来源、预计体积和要创建的数据目录。
3. 下载、校验并安装缺失版本；创建独立数据库实例和连接凭据。
4. 展示项目依赖安装命令，用户主动执行，例如 `npm ci`。这类命令可能运行仓库脚本，操作记录必须与运行时安装区分。
5. 保存项目声明、解析后的版本锁和本机配置。
```

### docs/local-dev-platform-plan.md:19 — 项目入口与本地优先约束

```text
- 项目是主要入口。用户关心“这个项目能否访问、缺什么、哪里出错”，不必先理解 launchd。
- 本地 HTTPS 可以独立使用。已有 `localhost:3000` 服务也能直接获得 `https://app.test`。
- PHP、PostgreSQL 使用 Homebrew 管理二进制与依赖；其他软件下载经过校验的发行包，在应用管理的目录中安装和运行。
- App 管理二进制包及实例配置；服务进程统一复用现有 `internal/launchd` 和 `launchctl` 能力，由 macOS launchd 托管生命周期。
- 版本按项目选择。二进制可共享，配置、端口、日志和数据库数据按实例隔离。
- 支持任意语言启动的 HTTP 服务。首版直接管理 PHP 和 Node.js，项目模型不绑定单一语言。
- 核心功能开源、无需账户、无站点数量限制。安装完成后，本地运行不依赖云端。
```

### scripts/test_collect_third_party_notices.py:8 — 离线 Python unittest 范式

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

设计匹配 Go + MyGo 表单与既有后台命令任务，不创建第二套进程调度。导入/读取声明只作建议，不执行仓库代码、不装依赖、不改锁文件；用户点击后才执行带明确后果的命令。实际产品命令需要使用项目选择的 Node 和独立工具版本，不静默采用系统 PATH。项目文档明确“不覆盖现有 .env”；设计不得把凭据或本机路径写入共享声明。输出采用标准库 Python 的纯数据原型，仅用于验证决策，不是要分发的新运行时。

来自 CONTRIBUTING.md 的约束：“Use isolated roots and ports in tests. Stop and unregister every test job.” 真实 hosts、钥匙串、管理员授权、重启/注销不属于自动执行范围。只能在明确安排的原生验收中操作；不能为让测试通过关闭 TLS 验证、扩大权限或接管原有数据库。测试数据不引用真实项目、密钥或凭据；发现秘密只记录文件位置和类型，不复制值。

## Commands you will need

所有命令从仓库根执行，要求 darwin/arm64、Go 1.27.3 和需要时的 Python 3。无需升级依赖或安装新的测试框架。

| 用途 | 命令 | 成功条件 |
|---|---|---|
| 工具链 | `go version` | Go 1.27.3 或满足 go.mod 的版本，darwin/arm64 |
| 依赖完整性 | `go mod verify` | exit 0，all modules verified |
| 聚焦验证 | `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s plans/spikes/009-package-managers -p 'test_*.py' -v` | exit 0，指定新增用例实际执行且无 SKIP |
| 静态检查 | `go vet ./...` | exit 0 |
| 差异格式 | `git diff --check -- plans/spikes/009-package-managers/design.md plans/spikes/009-package-managers/cases.json plans/spikes/009-package-managers/prototype.py plans/spikes/009-package-managers/test_prototype.py plans/README.md` | exit 0 |
| 完整回归（隔离验收） | `go test ./...` 和 `go test -race ./...` | exit 0；单列 opt-in SKIP |
| 原生构建（仅代码计划需要时） | `go tool mygo build -sign - -skip-notarize` | exit 0，产物在忽略的 build/ 中；不签发版本、不公证、不发布 |

本计划仅交付设计与离线验证材料，不修改产品源码。原生构建不属于完成条件。

审查阶段已验证过 go vet、go mod verify、7 项隔离 Node 测试及 4 项 Python 测试；不能把这些历史结果视为本计划实施后的验收结果。

## Scope

**允许修改的文件（新增文件也必须在此列出）**：

- plans/spikes/009-package-managers/design.md
- plans/spikes/009-package-managers/cases.json
- plans/spikes/009-package-managers/prototype.py
- plans/spikes/009-package-managers/test_prototype.py
- plans/README.md（仅本计划状态与验证结果）

**禁止修改**：上述列表之外的产品源码、go.mod/go.sum、发布凭据、软件目录版本/来源、用户真实配置和数据库。只读引用文件不等于获准修改。本计划不允许修改任何产品源码或 CI；原型必须留在计划专用目录内。

## Git workflow

- 在独立分支 `codex/009-design-project-package-managers` 执行。保留已有工作；不要切换或重置包含他人修改的工作树。
- 提交不是计划的完成前提；仅在操作者要求时提交。仓库提交标题以直接描述行为为主，例如 `Restrict the shipped app to internal commands`。
- 不自动合并、推送、创建 PR、发布 issue 或触发签名发行。

## Steps

### 步骤 1: 写清问题、选项和工具来源边界

创建 design.md，固定章节为 Goal、Non-goals、Evidence、Detection、Execution contract、UX、Security and cancellation、Compatibility、Decision matrix、Rollout、Open questions。记录 npm/yarn/pnpm 的目标用户与 trade-off：管理器来源和版本可复现性、锁文件冲突、离线可用性、workspace/monorepo 根与所选项目目录的区别。以官方文档核对需要支持的 manager 版本/命令/锁文件约定，记录 URL 与查阅日期；未核实的 flag 不标“可执行”。排除 Bun/Deno/Composer、自动迁移锁文件、全局修改 shell、自动运行 install。

**验证**：`python3 -c 'from pathlib import Path; s=Path("plans/spikes/009-package-managers/design.md").read_text(); assert all(("## "+x) in s for x in ["Goal","Non-goals","Evidence","Detection","Execution contract","UX","Security and cancellation","Compatibility","Decision matrix","Rollout","Open questions"])'` → exit 0，所有章节存在；正文含明确选项与推荐，不只有标题

### 步骤 2: 建立决策表与稳定输入输出契约

创建 cases.json 顶层 schemaVersion=1、cases 数组。每行有 id、input、expected；input 仅包含虚构的文件声明/锁文件集合、运行时与管理器可用性，不包含需要执行的代码。expected 固定字段 status（ready/needs-choice/blocked）、manager、version、argv、cwdPolicy、reason、requiresConfirmation。至少 16 个不同 case，覆盖 npm 锁/无锁、pnpm、yarn 不同主版本、packageManager 与锁文件冲突、多个锁、无 Node、精确工具缺失、离线、workspace、坏 JSON/不支持声明。缺失工具或歧义不偷偷选 npm，blocked/needs-choice 的 argv 应为空。明确只把 manager 版本/来源决策列为建议；缺少安全可复现来源时保持 blocked。

**验证**：`python3 -c 'import json; d=json.load(open("plans/spikes/009-package-managers/cases.json")); c=d["cases"]; assert d["schemaVersion"]==1 and len(c)>=16 and len({x["id"] for x in c})==len(c); assert all(set(["id","input","expected"])<=set(x) for x in c)'` → exit 0，至少 16 个唯一场景及完整结构

### 步骤 3: 写纯函数原型与契约测试

prototype.py 暴露 resolve_request(fixture)，只读传入 dict，返回固定结构；不读取真实项目、不 import subprocess、不调用网络、不执行 argv、不写磁盘。test_prototype.py 用 unittest 加载全部案例并比较实际输出，检查输入深拷贝前后相同、结果没有本机路径/秘密、未知输入安全阻断。命令结构使用数组不拼 shell。另写测试确认所有 ready 结果 requiresConfirmation 为 true；缺失工具/冲突不能以动态下载命令“修复”。模型若和调研结论不符，应修正决策表并在 design.md 记录权衡，不硬编码按 case id 返回答案。

**验证**：`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s plans/spikes/009-package-managers -p 'test_*.py' -v` → 全部至少 16 个场景及不变性/副作用边界测试通过，无安装或外部访问

### 步骤 4: 完成交互和实施拆分

设计至少写出三个文字流程：明确单工具的预览/执行、冲突时选择或取消、工具不可用时的解释/返回。与当前按钮的 dirty/task-running 禁用条件一致，保存草稿不是安装命令隐式副作用。写后续 Go 实现的具体接入点（view_development.go 按钮、project inspection、Manager.Exec/runCommandWork），按检测→预览→工具获取策略→执行→验证拆成独立里程碑，给每个里程碑范围和验收。记录建议方案、被拒绝方案、剩余必须由维护者决定的问题；不得直接修改产品代码或声称产品已支持 pnpm/yarn。

**验证**：`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s plans/spikes/009-package-managers -p 'test_*.py' -v` → 案例仍全部通过；design.md 的决策与输出字段一致

## Test plan

每条 cases.json 场景必须通过 subTest 执行并核对完整 expected。额外测试：输入不变、空/坏声明、未知锁文件、不支持的 manager、任意脚本字符串只作为数据、缺失运行时、没有自动降级。必须有至少一个按官方资料确认的 ready npm case；pnpm/yarn 若版本来源未决可以 blocked，但必须解释可解锁条件，不能伪造支持。

## Done criteria

以下全部满足才可记为 DONE：

- [ ] 四个 scope 文件均存在；design.md 所有固定章节有实质内容。
- [ ] cases.json 至少 16 个唯一场景，覆盖三个管理器与冲突/缺失/离线/workspace。
- [ ] unittest 全部通过，原型没有安装、网络、shell 或真实项目读取副作用。
- [ ] design.md 给出推荐方案、被拒绝选项、资料日期及明确的未来 Go 实施里程碑。
- [ ] 产品源码、go.mod、CI 和用户配置无变化；DONE 只表示设计验证完成。
- [ ] 聚焦验证命令实际执行新增用例且通过；无零用例假通过。
- [ ] `go vet ./...` 与 `git diff --check` 通过；未解决的原生验收限制如实记录。
- [ ] `git status --short` 和相对执行起点的 diff 仅包含 Scope 内文件；不得顺手格式化整个仓库。
- [ ] plans/README.md 中该计划状态及验证说明已更新。

## STOP conditions

- 当前代码与摘录不一致，且变化影响实施假设；先报告差异并刷新计划。
- 必须修改 Scope 外文件、引入依赖或扩大产品能力才能完成。
- 合理修正后同一个验证仍连续失败两次；记录命令与错误，不跳过门槛。
- 必须运行真实第三方包管理器才能回答关键问题；先把该问题列为外部实验并报告，不擅自安装。
- 发现必须改变共享环境 schema 或运行时来源政策才能实现，先记录设计选项而不是修改模型。
- 官方资料无法确认命令/版本策略；保持 blocked 决策，不能用推测宣称可执行。

## Maintenance notes

这个 Python 原型不是长期产品依赖。后续 Go 实现应将 case 表转换成 Go 的黑盒契约测试，并删除或明确维护原型，避免双份逻辑分叉。先验证常见项目收益，再考虑 Composer/其他生态。
