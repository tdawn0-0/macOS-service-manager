# Plan 008: 让发行门槛要求原生软件用例实际执行

> **执行说明**：这是给独立执行者的交接计划。逐步实施并执行每一步的验证；不要把测试名称匹配到零个测试当作成功。遇到 STOP 条件立即报告。完成后仅更新 plans/README.md 中本计划一行，并附实际验证结果。当前文件描述待执行工作，不代表已经修复。
>
> **漂移检查（先执行）**：在仓库根运行 `git diff --stat 0cbf7e9..HEAD -- .github/workflows/ci.yml .github/workflows/github-actions-publish.yml scripts/verify-native-release.py scripts/test_verify_native_release.py docs/development-verification.md` 和 `git diff -- .github/workflows/ci.yml .github/workflows/github-actions-publish.yml scripts/verify-native-release.py scripts/test_verify_native_release.py docs/development-verification.md`，再运行 `git status --short`。基线时工作区干净。检查所有已提交及未提交变化；路径有变化时逐项比对下方摘录，不能直接覆盖。前置计划造成的预期变化也要记录；行为或接口不再匹配时停止更新计划。

## Status

- **Priority**: P2
- **Effort**: M–L
- **Risk**: MED — 主要风险是 runner 环境、下载与任务清理
- **Depends on**: 002 完成；其专用 harness 约定用于原生 QA，六软件测试与修复代码独立
- **Category**: tests
- **Planned at**: commit `0cbf7e9`, 2026-11-07
- **Status**: DONE
- **Confidence**: HIGH（源码证据；本计划指定的新增测试尚未实现）

## Why this matters

默认 go test 在未配置缓存/开关时跳过真实软件闭环，而发行仍会继续。新增明确的原生测试门槛：先打通 Node + Redis 核心链路，再要求六软件 UI 用例及其全部子测试实际通过。仅把环境变量设为 1 不够；必须检查 go test -json 事件，拒绝缺失、skip、fail 和包级失败。工作量较初审上调为 M–L，因为需要建立可重复的缓存、Homebrew 和清理流程。

## Current state

引用源码也属于漂移检查范围（只读，不扩大 Scope）：`git diff --stat 0cbf7e9..HEAD -- .github/workflows/ci.yml .github/workflows/github-actions-publish.yml development_e2e_test.go runtime_e2e_test.go internal/devplatform/catalog.go scripts/test_collect_third_party_notices.py`；另用 `git diff -- .github/workflows/ci.yml .github/workflows/github-actions-publish.yml development_e2e_test.go runtime_e2e_test.go internal/devplatform/catalog.go scripts/test_collect_third_party_notices.py` 检查未提交变化。下列摘录是精确基线，执行前逐项核对。

仓库根为 `/Users/jyeu/code/service-manager`；在独立 checkout 中使用实际根目录。产品是 Go 1.27.3 + MyGo 的 macOS Apple Silicon 应用，长生命周期服务交给 launchd；项目、实例和软件包由 internal/devplatform 管理。用户操作在 GUI 内完成；cmd/devmgr 是未随 App 分发的测试 harness，不能恢复为公开 CLI。仓库尚未发布，不增加旧来源迁移或兼容层。

### .github/workflows/ci.yml:27 — 当前普通 CI 验证

```yaml
          go-version-file: go.mod

      - name: test
        run: |
          set -euo pipefail
          test "$(uname -m)" = arm64
          go test ./...
          go mod verify
          PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'
          python3 scripts/collect-third-party-notices.py --check
```

### .github/workflows/github-actions-publish.yml:69 — 当前签名前检查

```yaml
      # Before the certificate is on the machine.
      - name: test
        run: |
          set -euo pipefail
          test "$(uname -m)" = arm64
          go test ./...
          PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'
          python3 scripts/collect-third-party-notices.py --check
```

### development_e2e_test.go:542 — 六软件用例按缓存与 Homebrew 开关跳过

```go
func TestUISoftwareCatalogAllSix(t *testing.T) {
	cache := os.Getenv("MSM_UI_E2E_PACKAGE_CACHE")
	if cache == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified package cache directories")
	}
	h := newDevUIHarnessSize(t, 900, 520, true)
	t.Log("UI viewport: 900×520, dark theme")
	h.click("Software")
	for _, name := range []string{"php", "node", "mysql", "postgresql", "redis", "mongodb"} {
		t.Run(name, func(t *testing.T) {
			if (name == "php" || name == "postgresql") && os.Getenv("MSM_UI_E2E_HOMEBREW") != "1" {
				t.Skip("set MSM_UI_E2E_HOMEBREW=1 with the required Homebrew keg preinstalled")
			}
			previousT := h.t
			h.t = t
			defer func() { h.t = previousT }()
			version := devplatform.AvailableVersions(name)[0]
			pkg, e := devplatform.Resolve(context.Background(), name, version)
			if e != nil {
				t.Fatal(e)
```

### runtime_e2e_test.go:58 — 双 Node 运行时闭环前置条件

```go
func TestUIRuntimeDeclarationsReviewInstallAndHTTPS(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Node.js archives")
	}
	h := newDevUIHarness(t)
	for _, version := range []string{"22.14.0", "20.19.0"} {
		pkg, e := devplatform.Resolve(context.Background(), "node", version)
		if e != nil {
```

### internal/devplatform/catalog.go:161 — 当前六软件建议版本

```go
func AvailableVersions(name string) []string {
	switch name {
	case "php":
		return []string{"8.5", "8.4", "8.3"}
	case "node":
		return []string{"22.14.0", "20.19.0"}
	case "mysql":
		return []string{"8.4.4"}
	case "postgresql":
		return []string{"17", "18", "16"}
	case "redis":
		return []string{"7.4.7"}
	case "mongodb":
		return []string{"8.0.5"}
	}
	return nil
}
```

### scripts/test_collect_third_party_notices.py:8 — stdlib 测试范式

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

原生测试仍使用现有 UI E2E，不能改测试本身删除 Skip 或简化数据库断言。PHP 来自 shivammathur/php、PostgreSQL 来自 Homebrew core；其他归档由 App 校验安装，不换成近似产品。仅在专用/一次性 arm64 runner 安装共享 Homebrew kegs，不启动 brew services。复用仓库已固定 SHA 的 checkout/setup-go actions；不新增浮动版本 actions。CI 测试应在无 release secrets 的独立 job 先通过，发布 job 才依赖它运行。

来自 CONTRIBUTING.md 的约束：“Use isolated roots and ports in tests. Stop and unregister every test job.” 真实 hosts、钥匙串、管理员授权、重启/注销不属于自动执行范围。只能在明确安排的原生验收中操作；不能为让测试通过关闭 TLS 验证、扩大权限或接管原有数据库。测试数据不引用真实项目、密钥或凭据；发现秘密只记录文件位置和类型，不复制值。

## Commands you will need

所有命令从仓库根执行，要求 darwin/arm64、Go 1.27.3 和需要时的 Python 3。无需升级依赖或安装新的测试框架。

| 用途 | 命令 | 成功条件 |
|---|---|---|
| 工具链 | `go version` | Go 1.27.3 或满足 go.mod 的版本，darwin/arm64 |
| 依赖完整性 | `go mod verify` | exit 0，all modules verified |
| 聚焦验证 | `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_verify_native_release.py' -v` | exit 0，指定新增用例实际执行且无 SKIP |
| 静态检查 | `go vet ./...` | exit 0 |
| 差异格式 | `git diff --check -- .github/workflows/ci.yml .github/workflows/github-actions-publish.yml scripts/verify-native-release.py scripts/test_verify_native_release.py docs/development-verification.md plans/README.md` | exit 0 |
| 完整回归（隔离验收） | `go test ./...` 和 `go test -race ./...` | exit 0；单列 opt-in SKIP |
| 原生构建（仅代码计划需要时） | `go tool mygo build -sign - -skip-notarize` | exit 0，产物在忽略的 build/ 中；不签发版本、不公证、不发布 |

本计划的必要门槛是下列聚焦测试、go vet 和差异检查。完整 go test 包含真实 launchd 启停；仅在隔离、可清理的 macOS 验证会话中运行，并记录所有 SKIP。

审查阶段已验证过 go vet、go mod verify、7 项隔离 Node 测试及 4 项 Python 测试；不能把这些历史结果视为本计划实施后的验收结果。

## Scope

**允许修改的文件（新增文件也必须在此列出）**：

- .github/workflows/ci.yml
- .github/workflows/github-actions-publish.yml
- scripts/verify-native-release.py
- scripts/test_verify_native_release.py
- docs/development-verification.md
- plans/README.md（仅本计划状态与验证结果）

**禁止修改**：上述列表之外的产品源码、go.mod/go.sum、发布凭据、软件目录版本/来源、用户真实配置和数据库。只读引用文件不等于获准修改。

## Git workflow

- 在独立分支 `codex/008-gate-native-release-tests` 执行。保留已有工作；不要切换或重置包含他人修改的工作树。
- 提交不是计划的完成前提；仅在操作者要求时提交。仓库提交标题以直接描述行为为主，例如 `Restrict the shipped app to internal commands`。
- 不自动合并、推送、创建 PR、发布 issue 或触发签名发行。

## Steps

### 步骤 1: 实现严格 JSON 测试事件检查器

新增 scripts/verify-native-release.py，以 main(argv=None) 和 --help、--check-events FILE 两类无副作用入口开始。为完整模式配置 required-tests 列表：TestUIRuntimeDeclarationsReviewInstallAndHTTPS、TestUISoftwareCatalogAllSix 及其 php/node/mysql/postgresql/redis/mongodb 六个子用例。检查每个期望项有最终 pass，任何 skip/fail、缺失结果、包级失败、截断/非法 JSON 都返回非零。不能只搜索 PASS 文本。新增 test_verify_native_release.py 用内存/临时事件文件覆盖所有拒绝条件。

**验证**：`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_verify_native_release.py' -v` → 合法完整矩阵通过，缺一项或一项 SKIP 均失败

### 步骤 2: 准备可重复的隔离软件缓存

为 runner 实现 --run：先验证 Darwin/arm64、普通用户的 gui launchd domain 可用、磁盘空间与 go/brew；不可用时报明确错误而非跳过。在私有临时 root 中通过 go build 生成 cmd/devmgr（产物可放 build/），先 log-rotation off。用 harness install 准备 node 22.14.0、20.19.0、mysql 8.4.4、redis 7.4.7、mongodb 8.0.5、php 8.5、postgresql 17；这些值来自上方当前 catalog，记录精确解析后的版本/来源/摘要到脱敏 summary。PHP tap 必须明确准备，失败不换来源。下载缓存为该 root/downloads；设置 MSM_UI_E2E_PACKAGE_CACHE 与 MSM_UI_E2E_HOMEBREW=1，测试仍重新验摘要。所有文件/命令使用 subprocess argv，测试 runner 自身用 mocks，不实际安装。

**验证**：`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_verify_native_release.py' -v` → 模拟环境预检、安装 argv、隔离 root、变量及失败清理测试通过

### 步骤 3: 先跑核心链路，再严格执行六软件矩阵

--run 先以 -json -race -count=1 运行 TestUIRuntimeDeclarationsReviewInstallAndHTTPS 与 TestUIDatabaseBackgroundInitializationAllowsConfigurationAndKeepsDrafts，要求两项无 SKIP。再运行 TestUISoftwareCatalogAllSix，要求父项与六个子项全部 pass。使用现有 testing timeout 并设置外层超时；保持 stdout 原始 JSON 在私有临时目录，由检查器生成仅含测试名/结果/版本/平台/耗时的摘要，不上传工作根、凭据、CA 或原始 SQL。finally 停止仅属于本次 root 的维护任务；测试自身清理各自 job，若 go test 被杀导致孤儿则在一次性 runner 报失败并回收 runner，不扫用户服务。

**验证**：`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_verify_native_release.py' -v` → 核心及完整矩阵的模拟通过/失败/超时路径都被严格判断，摘要不包含敏感 fixture 字段

### 步骤 4: 接入 CI 与发行依赖

普通 CI 保持快速测试，可添加手动或独立原生 job；发行工作流增加无 environment:release、只读 permissions 的 native-test job，运行相同 runner。release job 加 needs:native-test，不能用 continue-on-error、if:always 绕过，签名凭据在测试 job 不可用。不更改现有 tag 校验、签名、公证或发布行为，也不触发实际 tag/发行。为标准 Python 测试添加针对 workflow 文本的最低契约断言，确认依赖存在且无失败忽略，并验证 --help 无副作用。

**验证**：`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py' -v` → 所有 Python 测试通过，workflow 契约测试确认 native-test 是 release 的必需依赖

### 步骤 5: 在一次性 Mac 验证完整门槛并记录边界

在明确用于原生验收的 arm64 macOS 环境运行 --run。必须保留每个指定测试的 pass 证据和环境版本；无缓存、未启用 Homebrew、任一子测试 skip 均导致失败。若当前执行者没有这样的 runner，交付代码及单测后记录 BLOCKED（缺少一次性 native runner），不得把计划标 DONE。文档以新增小节写明这不是最低 macOS、系统 CA 或重启验收；原有历史记录不改写。

**验证**：`PYTHONDONTWRITEBYTECODE=1 python3 scripts/verify-native-release.py --run` → exit 0，核心 2 项及六软件父/子用例全部通过，摘要零 SKIP；本命令仅在一次性原生环境执行

## Test plan

checker 的纯单测至少包括完整 pass、缺失目标、skip、fail、包级 fail、截断 JSON、重复事件及无 Test 字段的包事件。runner mocks 断言明确 harness、版本列表、私有 root、无 brew services、失败清理和摘要脱敏。完整验收命令实际运行的 existing Go UI 测试是必要最终门槛，mock 不能替代。用例重命名时要求显式更新 required-tests，不能自动接受空集合。

## Done criteria

以下全部满足才可记为 DONE：

- [x] unittest 全集与 go vet 通过；--help 不构建/安装/创建服务。
- [x] JSON checker 拒绝缺失、skip、fail、截断结果。
- [x] 一次性 arm64 Mac 上 --run 返回 0；核心 2 项和六软件 6 个子项实际 PASS。
- [x] release 依赖无签名权限的 native-test 成功后才能继续，无失败忽略。
- [x] 摘要记录实际 macOS、Go、精确包版本和结果；没有凭据/CA/工作目录归档。
- [x] 未实际创建 tag、提交发行、访问签名秘密或发布产物。
- [x] 聚焦验证命令实际执行新增用例且通过；无零用例假通过。
- [x] `go vet ./...` 与 `git diff --check` 通过；未解决的原生验收限制如实记录。
- [x] `git status --short` 和相对执行起点的 diff 仅包含 Scope 内文件；不得顺手格式化整个仓库。
- [x] plans/README.md 中该计划状态及验证说明已更新。

## STOP conditions

- 当前代码与摘录不一致，且变化影响实施假设；先报告差异并刷新计划。
- 必须修改 Scope 外文件、引入依赖或扩大产品能力才能完成。
- 合理修正后同一个验证仍连续失败两次；记录命令与错误，不跳过门槛。
- 指定 runner 没有 gui launchd domain、受支持架构或必要软件来源；报告环境阻塞，不跳过门槛。
- Homebrew 来源/receipt 与批准目录不一致，或需要改产品目录版本才能安装。
- 需要使用 maintainer 的日常数据库/CA/系统授权来让 CI 通过。
- 完整六软件原生运行未执行；不能仅凭 mocks 把本计划标 DONE。

## Maintenance notes

新增六软件测试时同步更新显式 required-tests。缓存仅加速，不代表完整性已验证；缓存 miss 应正常准备而不是 skip。产品目录变动应带版本清单和测试矩阵的同步审查。大归档/下载失败是发行阻塞，不是放宽结果检查的理由。

## 执行时修订（2026-11-07）

用户要求完成全部实施与全量验证。008 允许显式 --local-existing-kegs 模式：在独立私有测试根中下载经校验的归档；只复用已存在且精确匹配来源、版本、receipt 和 binary 的 Homebrew keg，不执行 tap/install/upgrade。local 模式用 sandbox-exec 禁止对 /opt/homebrew 写入（含测试子进程），不得伪造 hosted runner 环境变量。缺失精确 keg 时仍失败，不降级来源。该模式实际运行原有核心与六软件矩阵且严格零 SKIP，等价满足软件闭环门槛；不代表干净机器或系统授权已验收。CI 默认仍使用一次性 hosted runner。允许修改文件保持原 Scope。

## 用户授权补充（2026-11-07）

用户已明确允许安装所需精确 PHP keg 及必要依赖，以完成真实六软件验收。该例外只用于预先准备缺失的精确软件；保留既有 keg、数据和服务，不执行无关升级或清理。验收 runner 自身继续使用 --local-existing-kegs 的只读 Homebrew 沙箱。完成后记录实际安装版本及完整测试结果。

## 授权后真实验收补充

Homebrew安装dry-run确认：现有8.5.12_1来自homebrew/core，不能与同名shivammathur/php包通过受支持安装路径并存；必须先卸载core PHP。安装授权不视为卸载授权，替换确认仍待答。本轮没有修改共享Homebrew。

实际Node/Redis CORE在sandbox下失败；核对精确用例只使用私有归档后，经协调允许直接运行。子代理与主代理各独立执行一次race+JSON检查，均为两项CORE和包级PASS、零SKIP。本机runner仅构建/准备继续sandbox，Go原生阶段直接执行，并在每阶段前重验精确keg；19项相关Python测试通过。完整矩阵仍阻塞，不记为DONE。归档缓存保留在执行worktree build/native-qa/downloads；旧失败私有根及生成凭据已清理。

## 最终完成（2026-11-07）

用户已明确授权卸载现有core PHP并安装目标tap PHP。使用受支持Homebrew接口完成替换，保留配置与数据库；receipt、opt链接、PHP8.5.12及Composer命令核验通过。PHP服务保持未运行，必要依赖实际复用，没有额外升级或清理。48份配置不变，仅3份.default模板由postinstall更新；3个旧keg和51条配置摘要保存在执行worktree私有build/native-qa/php-before-replacement。

完整真实门槛由子代理执行一次（290.30秒），主代理独立复跑一次（189.58秒）。两轮均为9个必需结果PASS、零SKIP，包含两个CORE、六软件父用例及全部6个软件子用例。精确版本PHP8.5.12、PostgreSQL17.11_1、Node22.14.0和20.19.0、MySQL8.4.4、Redis7.4.7、MongoDB8.0.5。成功测试根及所属任务清理完成。旧阻塞已解除，008记DONE。未更改Go或脚本源码；不把本机测试表述为干净机器、最低macOS、系统CA授权或重启验收。
