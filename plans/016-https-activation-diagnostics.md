# Plan 016: 提供 HTTPS 反代检查与准确的设置跳转

> 执行者：本文件是独立交接计划。按步骤实施并验证；仅更新自己的状态记录。先读完 Scope 与 STOP conditions。计划作者本轮没有实施源码。
>
> 漂移检查：`git diff --stat 2ec48ab..HEAD -- 'internal/devplatform/proxy_activation.go' 'internal/devplatform/proxy_activation_test.go' 'internal/devplatform/proxy_rules.go' 'internal/devplatform/diagnostic_report.go' 'internal/devplatform/diagnostic_report_test.go' 'view_https_proxy.go' 'view_proxy_activation.go' 'view_diagnostics.go' 'view_development.go' 'view_authority.go' 'view_ports.go' 'https_activation_ui_test.go'`，另运行 `git status --short`。本文件摘录描述规划基线。前置计划造成的预期变化须逐项记录在本文件“Execution record”，对照本文明确的前置契约继续；其他无法解释的漂移停止报告。不要为恢复摘录而撤销前置计划。

## Status

- Priority: P2
- Effort: L（包含测试的粗估，L 为多天）
- Risk: MED
- Depends on: 011、013、014 DONE；015-A 已定义 AC-DIAG
- Category: direction
- Planned at: commit `2ec48ab`, 2026-11-08
- Status: DONE（隔离工作树）

## Product contract

用户已决定取消注册项目：只管理软件版本、独立数据库实例和 HTTPS 反代。HTTPS 页面仅配置“域名 → 本地端口”规则，不设独立站点实体、详情页或生命周期。应用由用户在终端或 IDE 启动；App 不再管理源码目录、应用命令、依赖安装、编排、项目自启或环境导入导出。Node 按目录读取版本声明仍保留，不要求注册目录。保留现有系统服务管理为次要入口。

不做旧项目 schema、文件、任务或配置的兼容和迁移。开发验收使用新建隔离存储；这不授权清空用户真实数据库、卸载共享 Homebrew keg 或处理旧机器上的遗留进程。软件范围为 Node.js、MySQL、PostgreSQL、Redis、MongoDB 五类；PostgreSQL=Homebrew core，其余四类校验归档。删除 PHP 安装、版本选择、仓库添加和终端集成，不操作机器已有 PHP；macOS arm64、Go/MyGo 技术栈不变。界面主入口为 Software / Databases / HTTPS。尚未发布，任何新状态格式不需要迁移器，但各计划必须维持同一新格式并集成新增字段。

## Current state

以下为旧代码基线摘录，不是保留 PHP 或站点管理的目标要求。

`internal/devplatform/diagnostic_report.go:20`：

```go
type DiagnosticCheck struct {
	Layer    string `json:"layer"`
	Resource string `json:"resource"`
	Status   string `json:"status"` // OK, Warning, Failed, Stopped, Disabled
	Summary  string `json:"summary"`
	Remedy   string `json:"remedy,omitempty"`
}

type DiagnosticReport struct {
	SchemaVersion int               `json:"schemaVersion"`
	GeneratedAt   time.Time         `json:"generatedAt"`
	Redacted      bool              `json:"redacted"`
	Checks        []DiagnosticCheck `json:"checks"`
}
```

`internal/devplatform/diagnostic_report.go:302`：

```go
		address := net.JoinHostPort(u.Hostname(), port)
		probeCtx, done := context.WithTimeout(ctx, 2*time.Second)
		conn, err := p.dial(probeCtx, "tcp", address)
		if conn != nil {
			conn.Close()
		}
		done()
		if err != nil {
			add("Upstream", site.Domain, "Failed", "Upstream TCP port is unavailable.", "Start the application and verify its host and port.")
		} else if u.Scheme == "https" {
			probeCtx, done := context.WithTimeout(ctx, 2*time.Second)
			err := p.tls(probeCtx, address, u.Hostname())
			done()
			if err != nil {
				add("Upstream TLS", site.Domain, "Failed", "Upstream TLS certificate or handshake could not be verified.", "Correct the upstream certificate and hostname; keep certificate verification enabled.")
			} else {
				add("Upstream TLS", site.Domain, "OK", "Upstream port and TLS certificate verified; HTTP response was not tested.", "")
			}
		} else {
			add("Upstream", site.Domain, "OK", "Upstream TCP port is reachable; HTTP response was not tested.", "")
		}
		probeCtx, done = context.WithTimeout(ctx, 2*time.Second)
		err = p.tls(probeCtx, net.JoinHostPort("127.0.0.1", fmt.Sprint(site.Port)), site.Domain)
		done()
		if err != nil {
			add("Proxy TLS", site.Domain, "Failed", "Local HTTPS listener or certificate could not be verified.", "Apply proxy, verify CA trust and the HTTPS port, then run diagnostics again.")
		} else {
			add("Proxy TLS", site.Domain, "OK", "Local HTTPS listener and domain certificate verified using system trust.", "")
```

`view_diagnostics.go:105`：

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

- `internal/devplatform/proxy_activation.go`
- `internal/devplatform/proxy_activation_test.go`
- `internal/devplatform/proxy_rules.go`
- `internal/devplatform/diagnostic_report.go`
- `internal/devplatform/diagnostic_report_test.go`
- `view_https_proxy.go`
- `view_proxy_activation.go`
- `view_diagnostics.go`
- `view_development.go`
- `view_authority.go`
- `view_ports.go`
- `https_activation_ui_test.go`

每份计划允许更新自身的 Execution record 和 `plans/README.md` 自己的状态行。新文件只可在列出的命名范围内创建。不要修改其他计划的范围/状态。

## Git workflow

在执行者的隔离 checkout/worktree 实施，分支名 `codex/016-https-activation-diagnostics`；开始记下执行基线与工作区状态，不覆盖用户修改。提交风格沿用仓库 `feat:` / `refactor:` / `test:` / `docs:`。按逻辑单元交付可审查 diff；是否提交由执行任务授权决定，不自动合并、推送、创建 PR 或发布。

## Why this matters

HTTPS 已成为核心功能，用户需要知道保存配置后还差哪一步。现有诊断验证 DNS、TCP、TLS，但没有验证 HTTP 响应；只有文字建议没有设置入口。本计划添加可解释的状态和显式检查，不管理上游应用，也不把检查变成自动修复。

## Target and boundaries

先设计再实现一个有限状态契约，完成标准是 Go 功能与测试上线；仅设计文档或原型不算 DONE。

- HTTPS 页显示整体 Saved/Published（来自 014），已保存规则行内显示 DNS、Proxy TLS、Upstream、HTTP response，含观测时间、检查中/未知/过期。没有规则时不探测；代理停止时仍可显式检查并准确报告不可达。已有配置变更会使旧检查失效；不把旧成功沿用到新域名/上游。
- Saved 与 Published 为配置状态，HTTP 是一次观测。200/204/301/401/404/500 均显示“收到 HTTP 响应 + 状态码”；不宣称应用业务健康，也不因非 2xx 自动关闭代理。连接拒绝、timeout、TLS failure 明确分层。
- `Check HTTPS response` 由用户主动点击，使用已保存规则生成的 URL 的 `GET /`，超时 5 秒、最多读取并丢弃 64 KiB、响应体和 headers 不进入 UI/history/export；仅保留整数 status 和耗时。拒绝重定向跟随；禁用环境代理；不附加 cookie/auth、不开 insecure TLS。不自动轮询 HTTP 请求。
- 连接固定到全局代理的 loopback listener/port，TLS ServerName/Host 使用配置域名，生产使用系统 trust roots；DNS 检查另外报告真实解析，不借固定连接隐藏 DNS 错误。只有配置/解析/TLS/HTTP 各层证据都成立才给“本次访问检查通过”，仍不保证后续可达。
- 规则被删/修改、全局 listener port 改变或新的检查开始时取消或忽略旧结果；以规范化 domain + 整体配置 revision/fingerprint + request generation 识别结果。
- 最多一个相关设置跳转按钮：proxy settings、database、software、CA、ports；私有 typed target 不包含任意 URL/command。跳转需校验当前资源仍存在，保留草稿；取消丢弃停留原处。没有项目 target。
- 复制/导出诊断只走明确 DTO，枚举字段+固定 message code+匿名 alias。不得序列化 target、原始 error、domain、path、header、响应体、command、连接用户名/库名。

**Out of scope**：自动修复、自动 CA 信任/hosts、周期性 HTTP 探测、请求日志/重放、反代控制 API、外部应用启动、公开 deep link、任意用户自定义 HTTP headers/方法。

## Steps

### Step 1: 定义状态矩阵与纯判断测试

在 `proxy_activation.go` 定义本地 observation/state/result DTO，复用 014 publication helper；定义 stale 的规则（配置改变或显式重新检查立即失效，成功观测超过 30 秒显示历史时间/过期，不自动重发请求）。在本文件 Execution record 记录四层检查的依赖和状态真值表。不要用 display text 路由。

**Verify**：新增 `TestProxyActivationStates` 于 `proxy_activation_test.go`，用 table tests 验证 pending/published/unknown、无规则、代理停止、stale、配置变化、HTTP 非 2xx。`go test ./internal/devplatform -list '^TestProxyActivationStates$'` 列出名称；`go test -race ./internal/devplatform -run '^TestProxyActivationStates$' -count=1 -v` → PASS。

### Step 2: 实现有界、显式的 HTTP 检查

新增 `CheckProxyRuleHTTP(ctx,domain,expectedRevision,expectedFingerprint)` 或等价 typed API，读取当前已保存规则集并验证该规范化 domain 的成员关系，执行有限请求并返回 typed error 分类。注入 dial/clock/roots 仅限测试 helper；生产没有关闭证书验证或自信任 CA 的选项。使用固定 loopback dial target，拒绝自动 redirects（包括同域名），清理 body/transport、取消后释放连接。不能把 502 一律标为代理生成错误：应用也能返回 502，只报告 HTTP code 与其它层观察。

**Verify**：新增 `TestProxyRuleHTTPCheck`，使用 `httptest`/私有测试 CA 覆盖 2xx/3xx/401/404/500/502、redirect 接收方零请求、超时/取消、超大 body、不可信 cert、DNS 不正确但 loopback HTTP 可达、旧 fingerprint 拒绝、检查后实例/规则配置文件无写入。`go test -race ./internal/devplatform -run '^(TestProxyActivationStates|TestProxyRuleHTTPCheck)$' -count=1 -v` → PASS；生产 client TLS 配置不得含 InsecureSkipVerify。

### Step 3: 明确脱敏边界与目标解析

给诊断生产者添加私有 target（`json:"-"` 不作为唯一防线），导出改为显式 whitelist DTO。已有诊断 summary/remedy 中含动态资源信息的情况需改为固定 code 到文案的映射，不把原错误复制进 DTO。UI resolver 只支持固定 action kinds，数据库/软件校验 ID 当前 membership；代理目标打开唯一 HTTPS 配置页，可附规范化 domain 高亮现有行，不存在站点目标或详情页；资源失效时显示重新检查，不按名字猜目标。以 011 的独立草稿状态和 014 的整份规则表草稿为基线，本计划在 view_development.go 明确实现 `requestResourceSelection(kind,id)` 或等价单一入口：同资源只聚焦不重载；切页面保留各自草稿；HTTPS 目标只聚焦配置页且不丢弃/重载整表草稿，不以诊断结果覆盖输入；切换不同数据库实例若有脏草稿先 Cancel/Discard；Cancel 留在诊断且不覆盖草稿；Discard 回调重新校验目标和 busy 状态。数据库 guard 的实现责任属于本计划，不能假定 013 已交付它。目标消失则保持原页面并提示重新检查；添加的软件/CA/ports 跳转只导航。

**Verify**：新增 `TestResourceDiagnosticExport` 于 `diagnostic_report_test.go`，用合成 private sentinel 覆盖所有候选字段及未知字段，断言导出不含 sentinel、ID、target；两组不同身份的输入产生相同匿名结构。`go test -race ./internal/devplatform -run '^TestResourceDiagnosticExport$' -count=1 -v` → PASS。

### Step 4: 接入代理与规则检查结果与下一步动作

在 `view_proxy_activation.go` 为 HTTPS 单页和已保存规则行渲染简短检查结果，不创建站点列表或详情页；渲染整体发布状态及当前下一步：保存、Apply、Configure local domains、Local certificates、检查本地端口。调用现有对应页面/动作且保持授权独立；若会写系统必须由该动作显式触发。HTTP 检查按钮对 dirty/missing rule 禁用并给可见原因。上游启动提示指向终端或 IDE，不提供“启动项目”。

新增 `TestUIHTTPSActivation` 于 `https_activation_ui_test.go`，mock 探针和时钟，检查乱序完成不会覆盖新结果、导航取消保留草稿、目标域名消失/删除后重建不沿用旧观测、明确非 2xx 文案、四种布局。`layouts` 子用例支持 `MSM_RESOURCE_SHOTS`。

**Verify**：`go test -race . -run '^TestUIHTTPSActivation$' -count=1 -v` → PASS，零 SKIP；`MSM_RESOURCE_SHOTS=build/resource-shots/016 go test . -run '^TestUIHTTPSActivation/layouts$' -count=1` → 四种布局输出并人工查看；`go vet ./...`、`git diff --check` → exit 0。

## Test plan

新增四个 suite 分别覆盖状态纯逻辑、有界真实 loopback HTTP、导出脱敏、UI/导航。参考现有 `diagnostic_report_test.go` 的注入 probes 和 `proxy_settings_test.go` 的私有 TLS fixtures。明确测试仍不保证 macOS trust/Chrome；015-B 单独执行这些实机路径。新增规则只是配置行，不建立资源详情、生命周期或持久化观测表。

## Done criteria

- [ ] 四个 suite 均存在、实际 PASS、零 SKIP；race 无报告。
- [ ] 没有自动请求、重定向跟随、cookies/authorization、响应体保存或 TLS bypass；由 fake server/request counter 断言。
- [ ] published/dns/tls/http 状态语义不同，配置变更/规则删除或同域重建/过期不复用旧成功。
- [ ] 导出只含匿名 DTO；导航不能修改资源，stale/missing/dirty Cancel 保持状态。
- [ ] `go test ./... -run '^$'`、`go vet ./...`、`git diff --check` exit 0；截图已核对。

## Maintenance notes

检查某个时刻可达不是健康监控。未来增加请求参数必须重新审查副作用、导出和范围；不能通过扩展 action kind 直接把诊断变为命令执行。添加 DTO 字段必须补脱敏反例。

## Non-vacuous verification gate

以下是 Done 的必需补充门槛。`plans/check-test-events.py` 已随计划提供，只读取 stdin 的 Go JSON 事件；拒绝缺失完整测试名、SKIP、FAIL、重复或不完整包事件。不要只凭 exit 0 或顶层 -list 判断子用例通过。先执行 `set -o pipefail`，确保管道前后任一失败都失败。

```sh
set -o pipefail
go test -json -race ./internal/devplatform -run '^(TestProxyActivationStates|TestProxyRuleHTTPCheck|TestResourceDiagnosticExport)$' -count=1 | python3 plans/check-test-events.py --package macos-service-manager/internal/devplatform --require TestProxyActivationStates --require TestProxyRuleHTTPCheck --require TestResourceDiagnosticExport
MSM_RESOURCE_SHOTS=build/resource-shots/016 go test -json -race . -run '^TestUIHTTPSActivation$' -count=1 | python3 plans/check-test-events.py --package macos-service-manager --require TestUIHTTPSActivation --require TestUIHTTPSActivation/layouts --shot-dir build/resource-shots/016
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

016 执行记录：隔离工作树 `/Users/jyeu/code/service-manager-016`，提交 `8991776`（基于 014 `a52d06b`）。九个 Scope 文件；完整 Go/race、vet、聚焦 race 事件/截图门槛 PASS；末尾 JSON tag/测试调整后重跑聚焦门槛，未再跑完整 race。原生系统信任 NOT RUN，留给 015-B。

集成记录：按用户“合并 master”的明确指示，实现提交链已于本轮快进合入 master（`96549ea`）。此前工作树记录保留为历史；未验证的原生安装、外部卷与系统信任项目仍保持原状态。
