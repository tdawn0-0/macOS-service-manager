# Plan 015: 建立并完成无项目产品的验收与文档

> 执行者：本文件是独立交接计划。按步骤实施并验证；仅更新自己的状态记录。先读完 Scope 与 STOP conditions。计划作者本轮没有实施源码。
>
> 漂移检查：`git diff --stat 2ec48ab..HEAD -- 'README.md' 'CONTRIBUTING.md' 'docs/product-scope.md' 'docs/resources-acceptance.md' 'docs/local-dev-platform-plan.md' 'docs/development-ui.md' 'docs/development-usage.md' 'docs/development-completion-audit.md' 'docs/development-verification.md' 'docs/images/development/*' 'scripts/verify-native-release.py' 'scripts/test_verify_native_release.py' 'scripts/prepare-native-ui-verification.py' 'scripts/test_prepare_native_ui_verification.py' 'scripts/verify-development.py' '.github/workflows/ci.yml' '.github/workflows/github-actions-publish.yml' '*_test.go' 'internal/devplatform/*_test.go' 'internal/devcli/*_test.go'`，另运行 `git status --short`。本文件摘录描述规划基线。前置计划造成的预期变化须逐项记录在本文件“Execution record”，对照本文明确的前置契约继续；其他无法解释的漂移停止报告。不要为恢复摘录而撤销前置计划。

## Status

- Priority: P1
- Effort: L（包含测试的粗估，L 为多天）
- Risk: MED
- Depends on: A 阶段无依赖，先于 011；B 阶段依赖 011–014、016、017 全部 DONE
- Category: direction
- Planned at: commit `2ec48ab`, 2026-11-08
- Status: BLOCKED：A DONE；B 代码/文档完成但原生与系统信任验收环境缺失

## Product contract

用户已决定取消注册项目：只管理软件版本、独立数据库实例和 HTTPS 反代。HTTPS 页面仅配置“域名 → 本地端口”规则，不设独立站点实体、详情页或生命周期。应用由用户在终端或 IDE 启动；App 不再管理源码目录、应用命令、依赖安装、编排、项目自启或环境导入导出。Node 按目录读取版本声明仍保留，不要求注册目录。保留现有系统服务管理为次要入口。

不做旧项目 schema、文件、任务或配置的兼容和迁移。开发验收使用新建隔离存储；这不授权清空用户真实数据库、卸载共享 Homebrew keg 或处理旧机器上的遗留进程。软件范围为 Node.js、MySQL、PostgreSQL、Redis、MongoDB 五类；PostgreSQL=Homebrew core，其余四类校验归档。删除 PHP 安装、版本选择、仓库添加和终端集成，不操作机器已有 PHP；macOS arm64、Go/MyGo 技术栈不变。界面主入口为 Software / Databases / HTTPS。尚未发布，任何新状态格式不需要迁移器，但各计划必须维持同一新格式并集成新增字段。

## Current state

以下为旧代码基线摘录，不是保留 PHP 或站点管理的目标要求。

`scripts/verify-native-release.py:18`：

```python
PACKAGE = 'macos-service-manager'
CORE = ('TestUIRuntimeDeclarationsReviewInstallAndHTTPS',
        'TestUIDatabaseBackgroundInitializationAllowsConfigurationAndKeepsDrafts')
CATALOG = 'TestUISoftwareCatalogAllSix'
MATRIX = (CATALOG,) + tuple(CATALOG + '/' + x for x in ('php', 'node', 'mysql', 'postgresql', 'redis', 'mongodb'))
REQUIRED = CORE + MATRIX
SANDBOX = ('sandbox-exec', '-p', '(version 1)(allow default)(deny file-write* (subpath "/opt/homebrew"))')

VERSIONS = (('node', '22.14.0'), ('node', '20.19.0'), ('mysql', '8.4.4'),
            ('redis', '7.4.7'), ('mongodb', '8.0.5'), ('php', '8.5'), ('postgresql', '17'))
```

`development_e2e_test.go:43`：

```go
func newDevUIHarnessSize(t *testing.T, width, height int, dark bool) *devUIHarness {
	t.Helper()
	oldAdmin := system.RunAdmin
	system.RunAdmin = func(string, ...string) (string, error) {
		return "", errors.New("privileged operations are disabled in unattended UI tests")
	}
	t.Cleanup(func() { system.RunAdmin = oldAdmin })
	root := t.TempDir()
	binary := filepath.Join(t.TempDir(), "devmgr")
	c := exec.Command("go", "build", "-o", binary, "./cmd/devmgr")
	if out, e := c.CombinedOutput(); e != nil {
		t.Fatalf("build app entry point: %v %s", e, out)
	}
	h := &devUIHarness{t: t, root: root, posted: make(chan func(), 256), width: width, height: height}
	a := newAppWith(nil, nil, t.TempDir())
	h.a = a
	a.nav = "development"
	a.copy = func(value string) { h.copied = append(h.copied, value) }
	a.dev.loaded = true
	a.post = func(f func()) { h.posted <- f }
	a.spawn = func(f func()) { go f() }
	a.dev.nodeReleasesTried = true // the release list is read from nodejs.org only where a test asks
```

`runtime_e2e_test.go:82`：

```go
	h.fill("Project folder", dir)
	h.click("Import project")
	id := h.a.dev.id
	manager := devplatform.New(h.root)
	defer func() {
		_ = manager.RemoveProject(context.Background(), id)
		_ = manager.StopProxy(context.Background())
	}()
	if !strings.Contains(strings.Join(h.ui.Texts(), " "), "declarations disagree") {
		t.Fatal("UI hid conflicting exact runtime declarations")
	}
	h.fill("Project name", "Versioned app")
	h.click("Choose Node.js 22.14.0 from .nvmrc")
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

- `README.md`
- `CONTRIBUTING.md`
- `docs/product-scope.md`
- `docs/resources-acceptance.md`
- `docs/local-dev-platform-plan.md`
- `docs/development-ui.md`
- `docs/development-usage.md`
- `docs/development-completion-audit.md`
- `docs/development-verification.md`
- `docs/images/development/*`
- `scripts/verify-native-release.py`
- `scripts/test_verify_native_release.py`
- `scripts/prepare-native-ui-verification.py`
- `scripts/test_prepare_native_ui_verification.py`
- `scripts/verify-development.py`
- `.github/workflows/ci.yml`
- `.github/workflows/github-actions-publish.yml`
- `*_test.go`
- `internal/devplatform/*_test.go`
- `internal/devcli/*_test.go`

每份计划允许更新自身的 Execution record 和 `plans/README.md` 自己的状态行。新文件只可在列出的命名范围内创建。不要修改其他计划的范围/状态。

## Git workflow

在执行者的隔离 checkout/worktree 实施，分支名 `codex/015-resource-product-acceptance`；开始记下执行基线与工作区状态，不覆盖用户修改。提交风格沿用仓库 `feat:` / `refactor:` / `test:` / `docs:`。按逻辑单元交付可审查 diff；是否提交由执行任务授权决定，不自动合并、推送、创建 PR 或发布。

## Why this matters

当前真实验收以注册项目为主。直接删旧测试会使五类软件、后台任务、反代协议和数据保留的验证丢失。本计划先固定新验收契约，再在全部功能完成后收口真实测试与文档；两个阶段合为一份计划，不能完成 A 就把整项标 DONE。

## Phase contract

**015-A**：先交付 `docs/product-scope.md` 与 `docs/resources-acceptance.md`，只定义目标/测试矩阵和状态，不声称功能已实现。不改生产代码、不提前启用不存在的 CI 测试。README/旧文档全面改写留到 B，A 文档顶部明确其优先级及尚未交付项。

**015-B**：依赖各功能计划完成，更新测试门槛、工具、发布文档并实际执行。011 为删除项目 API 已做最小夹具改写与 required 测试重命名，本计划从该可编译基线上扩展，不再调用旧项目入口。依赖图为 A→011→其余功能→B，没有等待自己 DONE 的循环。

**Out of scope**：生产 Go 功能修复、放宽软件来源/校验、依赖升级、签名凭据、远程发布/推送、用户真实配置迁移。发现产品实现失败，报告对应计划并回交；本计划不在测试里补生产逻辑。

## Steps

### Step A1: 写明产品边界和可观察验收矩阵

`docs/product-scope.md` 固定 Software/Databases/HTTPS 三入口；应用在外部运行；无项目兼容迁移；原系统服务保留。软件固定 Node.js/MySQL/PostgreSQL/Redis/MongoDB 五类，保留各自来源，PHP 移除；软件版本不作为运行进程计数。关闭窗口/退出及 privileged listener 语义按 013 的目标写明。列出不支持：PHP 安装/版本管理、独立站点管理、项目编排、依赖安装、团队环境分享、自动接管上游、隧道、FastCGI。

`docs/resources-acceptance.md` 每一行包含场景 ID、输入、用户操作、预期状态/外部副作用、测试层、负责计划、测试名、执行结果（初始 NOT RUN）。最少定义：

| ID | 场景与必需证据 |
|---|---|
| AC-CORE | 新状态三个资源集合；旧 schema 拒绝且原文件不变；旧项目命令拒绝 |
| AC-RUNTIME | 两 Node 版本按目录选择；显式终端启用/关闭；PHP 软件入口和安装能力已移除；真实新 zsh 中 binary/version 匹配；无项目注册 |
| AC-DATABASE | 四数据库独立初始化、start/stop/restart、认证、库/用户、备份恢复与数据保持；Redis 用编号库 |
| AC-LIFECYCLE | 关闭窗口继续服务，keep/stop quit、代理独立选择、外部上游进程仍活着；任务取消和范围变化拒绝 |
| AC-PROXY | 单页 Domain→LocalPort 规则表、整表 Save/Discard/Apply、revision 冲突、空表发布、两域名转发；无站点实体/详情/启停；WebSocket、SSE、上传、Host/timeout、last-good route |
| AC-DIAG | DNS/TLS/upstream/HTTP 分层结果，401/404 与传输失败区分，无重定向跟随，无原始响应体导出 |
| AC-CONNECTION | 四引擎连接配置 URI、Mongo authSource、端口变化后 URI 更新、业务账号不复制 admin 密码 |
| AC-UI | 三入口、菜单栏、任务及诊断跳转、草稿、四种尺寸/主题 |
| AC-NATIVE-TRUST | 已签 App 从 Finder/open 启动，macOS 授权、真实系统信任、Chrome 无证书警告；单独记录 |

只登记合成例子；不抄录机器路径、凭据或数据库内容。最终验收说明应区分 mocked、isolated-native、system-trust/Chrome，不能互相顶替。

**Verify**：`python3 -c "from pathlib import Path; a=Path('docs/resources-acceptance.md').read_text(); p=Path('docs/product-scope.md').read_text(); assert all(x in a for x in ['AC-CORE','AC-RUNTIME','AC-DATABASE','AC-LIFECYCLE','AC-PROXY','AC-DIAG','AC-CONNECTION','AC-UI','AC-NATIVE-TRUST']); assert all(x in p for x in ['Software','Databases','HTTPS'])"` → exit 0；`git diff --check` → exit 0。索引状态写 `IN PROGRESS — A DONE; B waiting`，011 可开始。

### Step B1: 将测试与准备脚本改为独立资源入口

核对 011 已替换的 `TestUIStandaloneRuntimeAndHTTPS`、`TestUIDatabaseInitializationKeepsIndependentDrafts` 以及 `TestUISoftwareCatalogAllFive`。扩展新增顶层 `TestNativeResourceProduct`（建议文件 `resource_product_e2e_test.go`），固定直接子用例 `node-terminal`、`external-upstream-https`、`resource-quit`、`connection-profiles`。各 child 必须在测试中实际执行其名称对应的行为；资料缺失允许普通本地测试 SKIP，但原生门槛必须拒绝。

上游 HTTP server 由测试持有 exec.Cmd 启动，来源为已核验 Node；App 不登记它。cleanup 只 kill 自己持有的 PID/process group、移除自己的 temp root。终端用临时 HOME/ZDOTDIR，不 source 用户真实 rc。启动数据库使用独立 roots/ports，禁止 Homebrew default service。HTTP/TLS 测试可使用私有信任池，必须单列不等于系统信任通过。

脚本 `prepare-native-ui-verification.py` 生成的记录改为实例、反代规则集、外部测试上游的身份，不含项目；保持 `--devmgr` 为测试入口，shipped App 只允许内部入口。`verify-development.py` 只验证当前五类主产品能力，不再通过隐藏 Go/Valkey/项目流程代表新产品验收。

**Verify**：`go test ./... -run '^$'` → exit 0；`go test -list '^(TestNativeResourceProduct|TestUIStandaloneRuntimeAndHTTPS|TestUIDatabaseInitializationKeepsIndependentDrafts|TestUISoftwareCatalogAllFive)$' .` → 四个顶层存在；`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'` → exit 0。

### Step B2: 收紧原生 required 结果并验证失败模式

在 `verify-native-release.py` 的 required 清单保留五类软件父/子结果，以及 011 新名字；另包含 `TestNativeResourceProduct` 父项及四个确切 child。以实际最终名字唯一列出，不仅检查父级 PASS。沿用现有事件校验：任何 fail/skip、缺失 child、重复、截断、包未完成均失败；零匹配不能通过。不移除 runner/架构/磁盘/receipt/清理检查。

在 `test_verify_native_release.py` 增加 success、missing child、all skipped、wrong package、no matching tests、重复、timeout、准备失败、外部 fixture 启动失败、只清理自身资源的测试。CI 保持无凭据的 native gate 先于 release signing，不能因为任务耗时将其改成 allow-failure。

**Verify**：`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_verify_native_release.py'` → 全部 PASS；在一次性 GitHub-hosted macOS runner 执行 `PYTHONDONTWRITEBYTECODE=1 python3 scripts/verify-native-release.py --run` → exit 0，所有 required 父/子 PASS 且零 SKIP。没有 runner 时记录 BLOCKED/NOT RUN，不能用本机 mock 宣称关闭 B。

### Step B3: 完整回归、构建、截图与实机记录

依次运行编译/vet、完整 Go/race、Python、notice check；同一版本不无意义重复。执行 `go tool mygo build -sign - -skip-notarize` 只作 ad-hoc 构建检查。汇总每个 UI suite 四种尺寸/主题截图；真实 macOS 信任/443 授权与 Chrome 通过另记 AC-NATIVE-TRUST，如无法执行则状态保持 NOT RUN，不把 ad-hoc 构建当签名发行验收。

**Verify**：`go test ./...`、`go test -race ./...`、`go vet ./...`、`go mod verify`、`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'`、`python3 scripts/collect-third-party-notices.py --check`、`go tool mygo build -sign - -skip-notarize` → 全部 exit 0；记录完整套件 SKIP；AC 必需路径不能靠 SKIP 达标。AC-NATIVE-TRUST 留有具体 App 版本、macOS、Chrome、授权/证书结果的人工记录才算通过。

### Step B3a: 按 runbook 完成系统信任与 Chrome 验收

这是 AC-NATIVE-TRUST 的独立交接流程。只在获准使用的一次性 macOS arm64 测试机器执行，该机器需有管理员操作条件和已安装 Chrome。专用账户或 `MSM_DEV_HOME` 只能隔离部分用户数据，不能隔离系统 hosts、信任与监听端口，不能单独作为隔离条件。缺一次性测试机器、签名产物、管理员条件或 Chrome 时将 B 标 BLOCKED，列明缺项。

**启动前检查**：读取 `/etc/hosts`，确认没有 `# BEGIN Service Manager Development` / `# END Service Manager Development` 既有受管区块，也没有 `acceptance.test` 条目；检查 TCP 443、48123 没有既有 listener，并确认没有其他 App 实例或其受管代理在运行。这里的 hosts 标记来自基线 `internal/devplatform/proxy.go`；执行前核对最终实现的标记。发现任一冲突或无法确认归属立即 BLOCKED，不覆盖、停止、清理或恢复其他资源。记录干净前置状态后才继续。`HostsContent` 会替换整块受管 hosts 内容，不能把另一账户的区块当作本次可覆盖数据。

**产物输入与身份核对**：使用全部功能合入后的当前提交，记录 `git rev-parse HEAD` 和 `git status --short`（除测试产物/验证记录外应无代码改动）。在配置了 Developer ID 的授权构建机运行仓库现有 `scripts/package.sh --arch arm64` 生成 App，或取得同一提交的 CI 签名产物并核对构建记录。不可把早期基线/另一分支的 App 作为本次产物；不改签名脚本、不导出私钥。公证如未执行须单列，其状态不得伪装成本项通过；本项验证签名 App 的本机运行和 CA 信任，不等同发行 Gatekeeper 全矩阵。

```sh
task_app="$PWD/build/darwin-arm64/MacOS Service Manager.app"
codesign --verify --deep --strict --verbose=2 "$task_app"
codesign -dv --verbose=4 "$task_app"
```

预期 verify exit 0，显示 Developer ID Application authority（非 ad-hoc）、预期 TeamIdentifier/BundleIdentifier。记录 App executable SHA-256、构建 commit 和签名身份摘要，不能只凭 Info.plist marketing version 判断提交一致。构建来源不明确即 BLOCKED。

**隔离启动**：从测试账户在独立终端准备空根，仅将路径传给 LaunchServices，不直接运行 bundle 内二进制：

```sh
task_root=$(mktemp -d /tmp/msm-resource-acceptance.XXXXXX)
chmod 700 "$task_root"
open -n --env "MSM_DEV_HOME=$task_root" "$task_app"
```

`open --env` 已在规划机器 `open -h` 核对；目标机器不支持时停止，不以直接启动继承终端权限替代。确认 App 显示三资源空态，且产生的 state.json 位于本次 root；不能看到原账户资源。外部应用在另一个测试终端用 `python3 -m http.server 48123 --bind 127.0.0.1` 启动；若前置检查后又发生端口冲突立即 BLOCKED，不停止未知 listener。该 terminal 归测试者所有，App 不保存其启动命令。

**用户操作与判定**：

1. HTTPS 规则表添加 Domain=`acceptance.test`、Local port=`48123`；全局 listener port=443；Save 保存整表。预期是 Pending，未自动授权、未启动/停止外部 server。
2. 明确点击 Configure local domains、Trust local CA（或 Local certificates 中等价入口）、Apply proxy；逐次完成 macOS 授权，不跳过拒绝分支。先对一次授权选择取消，确认未报告成功，再重新操作完成。记录生成 CA 的 fingerprint 和本次 app-owned job label，供精确清理。
3. Chrome 打开 `https://acceptance.test`；正常显示测试 server 的 HTTP 内容，无 interstitial，不点击忽略证书错误。查看证书的域名/issuer 与 App fingerprint 信息对应；规则行检查显示 DNS/TLS 和 HTTP 观测结果，保存截图使用测试名称。
4. 停止外部 server（在其所属终端 Ctrl-C）；Chrome 重载出现明确 upstream unavailable，App 不尝试启动应用。重新启动该 server 后可再次访问。
5. 关闭 App 窗口，Chrome 仍可访问；打开菜单栏只有单个代理状态和 HTTPS settings 入口，没有站点列表。先用 Keep services and quit 检查代理仍可访问，再重新用同一 task_root 打开 App；Stop proxy 后 Chrome 不再连上 listener。不存在无意停止外部 server 的行为。
6. 记录 macOS、Chrome、App commit/hash/signature、规则/CA、各步 PASS/FAIL、取消结果和清理结果。CLI TLS 或私有测试 trust pool 不能代替 Chrome 证据。

**精确清理**：重新打开同一隔离 App/root，仅对本次创建的 CA 使用 Remove CA trust（核对 fingerprint）；从表中移除本次规则并 Save，Apply 空集合并 Configure local domains 撤销本次受管 hosts 条目；停止本次 proxy/listener 和维护任务并退出 App。核对本次自有 label 已停止、hosts 不含 acceptance.test、本次 CA 已不受信任。最后在自己启动的终端停止外部 server；只有清理成功才移除 task_root。清理失败保留 root/证书身份和错误记录，不删除材料后留下无法定位的系统信任，不枚举并删除其他 CA 或 jobs。

**Verify**：一次性机器及干净前置状态有记录；签名检查 exit 0；本 runbook 的六步均有实际 PASS 与清理证据，AC-NATIVE-TRUST 才能标 PASS。若任一步 FAIL/BLOCKED，015-B 保持未完成并回交对应功能计划；不补造自动批准或静默系统修改。

### Step B4: 替换旧产品文档和截图

README 首屏与安装后流程变为软件、数据库、HTTPS 规则配置；删除旧项目用法及指向已清理计划的链接。用 product-scope.md 取代 `docs/local-dev-platform-plan.md` 的现行要求角色；旧文件可删除，并修复所有引用。改写 usage/UI/verification/completion-audit；历史通过记录只能标历史，不能复制成新版本通过。新截图来自合成夹具，不含机器真实信息。保留软件来源、权限与数据删除边界的准确说明。

**Verify**：`rg -n 'Import a project|Start project|Prepare imported environment|devmanager\.json|plans/(00[1-9]|010)-' README.md CONTRIBUTING.md docs --glob '*.md'` → 当前用法文件无匹配，若保留历史事实必须明确历史标题并记录允许位置；`git diff --check` → exit 0。执行本计划 Done criteria 的全部检查后才标 DONE。

## Test plan

此计划直接负责测试基础设施可靠性：Python 事件 parser 的正反用例；新 required child 的真实执行；准备脚本异常清理；UI screenshot inventory；所有现有保护（取消、数据保持、无外部进程控制、校验失败）映射到新测试而非整组删除。现有 `TestConnectionURIsExcludePasswordsAndSelectWorkingDefaults` 与 shell symlink tests 必须保留。

## Done criteria

- [ ] A 文档包含九项 AC、负责人和可观测结果；所有依赖计划状态及执行基线可追溯。
- [ ] 真实 native gate required 父/子均 PASS、零 SKIP，parser 负例全部拒绝。
- [ ] 完整 Go/race、Python、vet、mod verify、notice、构建通过；结果按层记录，没有把本轮规划/旧版通过当作新验收。
- [ ] AC-NATIVE-TRUST 已实际执行并记账；若未执行，B 不得 DONE。
- [ ] docs/README 不再将项目作为功能；所有图片/link 目标存在；四种 UI 布局截图已检查。
- [ ] 不涉及生产 Go 代码或发行凭据修改；没有发布、推送、真实数据迁移。

## Maintenance notes

015 的阶段状态是刻意设计：A 可以独立完成并放行实施，B 才是整体完成门槛。任何 required 测试改名都必须在代码、事件清单和 Python parser fixture 同时更新。修改清单不能减少产品行为覆盖。

## STOP conditions

- 非预期代码漂移、前置契约不成立，或需要修改 Scope 外路径：先报告具体差异并修订计划，不能自行扩范围。
- 同一验证失败两次合理修复后仍未通过：留下命令和失败原因，不以删除断言、增加 SKIP 或降低 required 清单通过。
- 新能力要求兼容旧项目、迁移真实数据、接管外部应用进程或加入项目替代实体：停止；这些与批准方向冲突。
- 准备环境需要替换本机 Homebrew 安装、操作真实 hosts/钥匙串、注销/重启：不把这些当成普通测试前置；使用隔离测试并单列实机未验证项。

## Execution record

执行者填写：实际起点 SHA、前置计划契约核对、文件差异、运行命令、测试名/子用例 PASS 与 SKIP、原生环境、截图路径、未验证边界。未执行不写 PASS；只有全部 Done criteria 达成才标 DONE。

015-A 执行记录：隔离工作树 `/Users/jyeu/code/service-manager-015a`，提交 `ad9f773`（父提交 `2ec48ab`）。仅新增 `docs/product-scope.md`、`docs/resources-acceptance.md`；九项结果 NOT RUN。A1 Python 断言和提交差异 `git diff ad9f773^ ad9f773 --check` 经复核通过。015-B 尚未执行，整体未 DONE。

015-B 执行记录：隔离工作树 `/Users/jyeu/code/service-manager-015b`，提交 `c4246ae`（基于 017 `d76d545`）。19 个 Scope 文件；Go 全量/race、vet、mod verify、Python 31 项、notices、ad-hoc MyGo 构建、五组 UI 布局截图门槛、diff check PASS。`verify-native-release.py --run` 与签名 App/系统信任/Chrome runbook 均 NOT RUN：缺一次性 GitHub-hosted macOS runner，以及获准使用的可清理 macOS/Chrome 机器与同提交 Developer ID 签名 App。九项 AC 保持 NOT RUN，计划整体 BLOCKED；未改用户主工作树或本机 hosts/信任。

集成记录：按用户“合并 master”的明确指示，实现提交链已于本轮快进合入 master（`96549ea`）。此前工作树记录保留为历史；未验证的原生安装、外部卷与系统信任项目仍保持原状态。
