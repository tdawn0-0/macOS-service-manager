# Plan 014: 将 HTTPS 收缩为域名到本地端口的规则配置

> 执行者：本文件是独立交接计划。按步骤实施并验证；仅更新自己的状态记录。先读完 Scope 与 STOP conditions。计划作者本轮没有实施源码。
>
> 漂移检查：`git diff --stat 2ec48ab..HEAD -- 'internal/devplatform/proxy_rules.go' 'internal/devplatform/model.go' 'internal/devplatform/proxy.go' 'internal/devplatform/proxy_routes.go' 'internal/devplatform/resource_locks.go' 'internal/devplatform/proxy_rules_test.go' 'internal/devplatform/proxy_settings_test.go' 'internal/devplatform/diagnostic_report.go' 'view_development.go' 'view_https_proxy.go' 'view_diagnostics.go' 'view_resource_lifecycle.go' 'https_proxy_ui_test.go' 'development_e2e_test.go'`，另运行 `git status --short`。本文件摘录描述规划基线。前置计划造成的预期变化须逐项记录在本文件“Execution record”，对照本文明确的前置契约继续；其他无法解释的漂移停止报告。不要为恢复摘录而撤销前置计划。

## Status

- Priority: P1
- Effort: L（包含测试的粗估，L 为多天）
- Risk: MED
- Depends on: 011、013 DONE；015-A 已定义 AC-PROXY
- Category: direction
- Planned at: commit `2ec48ab`, 2026-11-08
- Status: DONE（隔离工作树）

## Product contract

用户已决定取消注册项目：只管理软件版本、独立数据库实例和 HTTPS 反代。HTTPS 页面仅配置“域名 → 本地端口”规则，不设独立站点实体、详情页或生命周期。应用由用户在终端或 IDE 启动；App 不再管理源码目录、应用命令、依赖安装、编排、项目自启或环境导入导出。Node 按目录读取版本声明仍保留，不要求注册目录。保留现有系统服务管理为次要入口。

不做旧项目 schema、文件、任务或配置的兼容和迁移。开发验收使用新建隔离存储；这不授权清空用户真实数据库、卸载共享 Homebrew keg 或处理旧机器上的遗留进程。软件范围为 Node.js、MySQL、PostgreSQL、Redis、MongoDB 五类；PostgreSQL=Homebrew core，其余四类校验归档。删除 PHP 安装、版本选择、仓库添加和终端集成，不操作机器已有 PHP；macOS arm64、Go/MyGo 技术栈不变。界面主入口为 Software / Databases / HTTPS。尚未发布，任何新状态格式不需要迁移器，但各计划必须维持同一新格式并集成新增字段。

## Current state

以下为旧代码基线摘录，不是保留 PHP 或站点管理的目标要求。

`internal/devplatform/sites.go:8`：

```go
// EditSite changes only routing declarations. Applying them is a separate
// explicit action, allowing routes to change without restarting project jobs.
func (m *Manager) EditSite(ctx context.Context, oldOwner, oldID, owner string, site Site) error {
	if e := site.Validate(); e != nil {
		return e
	}
	release, e := m.resourceLeases(ctx, projectResourceKeys(oldOwner, owner)...)
	if e != nil {
		return e
	}
	defer release()
	return m.withState(ctx, func(s *State) error {
		if oldID != "" && !removeSite(s, oldOwner, oldID) {
			return errors.New("site changed or was removed; select it again")
		}
		if owner == "" {
			if _, exists := s.Sites[site.ID]; exists {
				return errors.New("a site with this name already exists")
			}
			s.Sites[site.ID] = site
```

`internal/devplatform/proxy_routes.go:68`：

```go
// Every published snapshot has passed full validation. A corrupt or missing
// file leaves the last good routing table and transports available to requests.
func (routes *liveProxyRoutes) snapshot() *proxySnapshot {
	routes.mu.Lock()
	defer routes.mu.Unlock()
	b, e := os.ReadFile(filepath.Join(routes.manager.Root, "proxy", "sites.json"))
	if e != nil {
		return routes.current
	}
	digest := sha256.Sum256(b)
	if digest == routes.digest {
		return routes.current
	}
	var sites []Site
	if json.Unmarshal(b, &sites) != nil || validateSites(sites) != nil {
		return routes.current
	}
	routes.current = routes.build(sites, routes.current)
	routes.digest = digest
	return routes.current
```

`internal/devplatform/model.go:400`：

```go
	u, e := url.Parse(s.Upstream)
	if e != nil || u.User != nil || (u.Scheme != "http" && u.Scheme != "https") || u.Host == "" || u.RawQuery != "" || u.Fragment != "" || (u.Path != "" && u.Path != "/") {
		return errors.New("upstream must be an HTTP(S) origin")
	}
	host := u.Hostname()
	ip := net.ParseIP(host)
	if host != "localhost" && (ip == nil || !ip.IsLoopback()) {
		return errors.New("upstream must be a loopback address")
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

- `internal/devplatform/proxy_rules.go`
- `internal/devplatform/model.go`
- `internal/devplatform/proxy.go`
- `internal/devplatform/proxy_routes.go`
- `internal/devplatform/resource_locks.go`
- `internal/devplatform/proxy_rules_test.go`
- `internal/devplatform/proxy_settings_test.go`
- `internal/devplatform/diagnostic_report.go`
- `view_development.go`
- `view_https_proxy.go`
- `view_diagnostics.go`
- `view_resource_lifecycle.go`
- `https_proxy_ui_test.go`
- `development_e2e_test.go`

每份计划允许更新自身的 Execution record 和 `plans/README.md` 自己的状态行。新文件只可在列出的命名范围内创建。不要修改其他计划的范围/状态。

## Git workflow

在执行者的隔离 checkout/worktree 实施，分支名 `codex/014-simple-https-proxy`；开始记下执行基线与工作区状态，不覆盖用户修改。提交风格沿用仓库 `feat:` / `refactor:` / `test:` / `docs:`。按逻辑单元交付可审查 diff；是否提交由执行任务授权决定，不自动合并、推送、创建 PR 或发布。

## Why this matters

用户需要将本机已有 HTTP 服务通过 HTTPS 域名访问，不需要再创建和管理一个“站点”。011 已删除 Site 模型并交付规则集 API。本计划用一个 HTTPS 配置页管理整份转发规则，保留明确的保存、发布、系统域名和证书操作。

## Target and boundaries

- 页面只含一张规则表：每行 Domain、Local port、移除按钮；表尾 Add rule。没有站点导航列表、站点名称/ID、详情页、启停开关、目录、运行时绑定或每条规则的生命周期。
- `ProxyRule` 仅 Domain、LocalPort。Domain trim/lowercase 后按现有合法精确域名规则验证，不接受 URL、IP、路径、通配符；LocalPort 为 1–65535 整数。规范化域名重复即拒绝整份保存，同一端口可被多个域名使用。目标固定 `http://127.0.0.1:<LocalPort>`；用户的 IPv6-only 或 HTTPS-only 服务不属于本版输入范围。
- 代理 listener port 是独立的全局字段，默认 443，范围 1–65535；不为规则单独设置入站端口。不接受任一 LocalPort 与 listener port 相同，防止反代自身。保留全局代理安全默认、Host 策略和超时，不在行内扩展高级选项。
- `ReplaceProxyRules(ctx, expectedRevision, listenerPort, rules)` 在 withState 内比较当前 revision，验证整份输入后原子替换并递增；全局 listener 设置与规则变更使用同一修订检查事务，避免端口校验竞态。表格只有一份草稿；Save 保存全部、Discard 丢弃全部。切页面保留草稿，后台刷新不覆盖；外部修改后保存冲突需用户重新加载或保留草稿，不静默覆盖。
- Save 不改 hosts、信任或进程。Apply proxy 发布全部已保存规则，作用域明确；有未保存修改时禁用 Apply 并说明先 Save/Discard。规则删除也要 Save 再 Apply。Configure local domains 和 Trust local CA 保持独立显式操作。
- 页面级 `ProxyPublicationState` 为 Pending/Published/Unknown，比较规范化保存配置与 `proxy/rules.json`（含 listener port）；Published 只表示发布记录一致，不代表 worker 加载、系统信任或浏览器就绪。DNS/TLS/HTTP 的实际检查归 016。
- 不对规则增加 enabled 字段。暂不使用一条转发时从表内移除并 Save/Apply；代理整体 Stop 保留已保存规则。可从已保存行 Open HTTPS URL，但它只是打开浏览器，不创建站点资源或启动上游。

**Out of scope**：站点管理、PHP/PHP-FPM/FastCGI、静态文件托管、任意远程地址或自定义 upstream URL、规则级高级配置、HTTP 检查（016）、系统 DNS server、隧道、请求日志/重放、外部应用生命周期、自动 CA 信任。

## Steps

### Step 1: 固定规则集与发布状态契约

核对 011 的 ProxyRules/ProxyListenerPort/ProxyRulesRevision、整体替换 API、全局 listener port 和 rules.json。发布记录固定为 `{schemaVersion: 1, listenerPort: N, rules: [{domain, localPort}]}`，空表 rules=`[]`；严格拒绝未知字段，与 011 一致。在 `proxy_rules.go` 补足规范化、重复/端口/自回环校验、expectedRevision 冲突。按域名排序比较规则集，忽略表格显示顺序；listener port 变化必须使发布状态 Pending。缺失/坏发布记录为 Unknown/Pending，不能当成功。UI 临时行 key 仅用于输入，不持久化站点 ID。

**Verify**：新增 `TestProxyRules` 于 `proxy_rules_test.go`；`go test ./internal/devplatform -list '^TestProxyRules$'` 列出它；`go test -race ./internal/devplatform -run '^TestProxyRules$' -count=1 -v` → 规范化、重复域名、端口上下界、listener 冲突、同时修改 listener port/规则失败时两者均不提交、修订冲突、排序比较、空表和坏发布文件 PASS。

### Step 2: 实现单页规则表与整份草稿

实现 `view_https_proxy.go`，由 HTTPS 主入口直接展示规则表、全局 listener port、Save/Discard/Apply、域名配置/证书入口。空态提供 Add rule。添加/删除行只修改草稿；不在每行提供 Save 或独立启停。Save 对全部行做原子验证，在具体错误行显示原因；任何一行失败不得部分保存。重新加载脏草稿需 Cancel/Discard，切去数据库再返回保持原输入。

**Verify**：新增 `TestUIHTTPSProxyRules` 于 `https_proxy_ui_test.go`，纯 mock Run；`go test -race . -run '^TestUIHTTPSProxyRules$' -count=1 -v` → 多行增删、整表取消、部分无效全部不保存、冲突、切页保留、无站点列表/详情/开关、Apply dirty 禁用均 PASS。

### Step 3: 应用规则并保留有效路由

复用现有 ApplyProxy/startProxy/network 授权路径，所有路由从保存规则生成，不重写反代协议。Save 后 Apply 失败保留 Pending/Unknown，不回写旧草稿掩盖失败。坏发布文件仍沿用 last-good。空规则集 Apply 时先停止并验证代理停止，再原子发布合法空规则记录；停止失败保留原发布记录，写入失败不得 Published。应用、信任或域名授权取消不得报告成功。

空记录的格式必须与 011 的 rules.json parser 一致，包含有效空规则数组和全局配置，不能误写成旧 sites.json 的格式。hosts 更新只使用已保存规则；同一动作若等待授权，提交时重新核对 revision，过期则拒绝。删除规则后提醒域名配置尚待单独同步，不宣称 Save 已清理系统 hosts。

**Verify**：`go test -race ./internal/devplatform -run '^(TestProxyRules|TestProxyRulesForwardHeadersAndKeepLastGoodRouting|TestProxyHeaderTimeoutAndLocalPortRules)$' -count=1 -v` → loopback/private CA 测试 PASS；原测试如含旧 Site 夹具已由 011 改写，改名后的超时测试保留原超时断言，另验证拒绝自定义 HTTPS upstream。`rg -n 'Start the project|check the project status' internal/devplatform/proxy_routes.go` → 无匹配。

### Step 4: 验证协议与单页布局

`TestUIHTTPSProxyRules/layouts` 检查四种尺寸/主题下长域名、多行、端口错误、水平/垂直溢出和底部 Save/Apply 可见可点击。Open 使用全局 listener port 构造 URL，非 443 不省略端口；地址不受未保存草稿影响。菜单栏只有一个代理设置入口，不恢复每条规则菜单。保留真实 loopback 的 WebSocket/SSE/upload/Host/timeout 回归，不要求启动应用。

**Verify**：`MSM_RESOURCE_SHOTS=build/resource-shots/014 go test . -run '^TestUIHTTPSProxyRules/layouts$' -count=1` → 四张截图和 hit-test PASS；`go test ./... -run '^$'`、`go vet ./...`、`git diff --check` → exit 0。

## Test plan

后端使用 `proxy_settings_test.go` 的 loopback 夹具。`TestProxyRules` 覆盖两域名指向两个端口、同端口复用、改端口发布、删最后规则→空发布、停止失败保留旧记录、写空记录失败不得 Published、revision 并发变更、未知字段/任意 upstream/旧 Site 输入拒绝。协议旧断言迁到新规则，不能整组删除。UI 覆盖一份草稿和全局 Apply，不测试站点 CRUD。真实 hosts/信任仍仅在 015-B 一次性机器运行。

## Done criteria

- [ ] 两个新增 suite 实际 PASS、零 SKIP，四种截图可读可操作。
- [ ] 所有输入只形成 Domain→127.0.0.1:LocalPort；无 Site 类型、单站点 CRUD、站点详情/生命周期或 PHP 控件。
- [ ] Save、Apply、hosts、信任各自结果准确；空表、坏配置、权限取消和外部变更不伪装成功。
- [ ] Published 与 worker/HTTPS 可达分开；代理 Stop 不控制上游，也不删除保存规则。
- [ ] 编译、vet、diff 检查通过；代码改动在 Scope 内。

## Maintenance notes

016 复用 ProxyPublicationState，以规范化域名与配置指纹定位本次规则观测，不引入站点 ID。新需求若要求规则目录、运行时或独立生命周期，需重新做产品决策；不能把规则表演变回站点管理。新增全局配置字段必须同步 revision、发布比较与失效规则。

## Non-vacuous verification gate

以下是 Done 的必需补充门槛。`plans/check-test-events.py` 已随计划提供，只读取 stdin 的 Go JSON 事件；拒绝缺失完整测试名、SKIP、FAIL、重复或不完整包事件。不要只凭 exit 0 或顶层 -list 判断子用例通过。先执行 `set -o pipefail`，确保管道前后任一失败都失败。

```sh
set -o pipefail
go test -json -race ./internal/devplatform -run '^TestProxyRules$' -count=1 | python3 plans/check-test-events.py --package macos-service-manager/internal/devplatform --require TestProxyRules
MSM_RESOURCE_SHOTS=build/resource-shots/014 go test -json -race . -run '^TestUIHTTPSProxyRules$' -count=1 | python3 plans/check-test-events.py --package macos-service-manager --require TestUIHTTPSProxyRules --require TestUIHTTPSProxyRules/layouts --shot-dir build/resource-shots/014
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

014 执行记录：隔离工作树 `/Users/jyeu/code/service-manager-014`，提交 `a52d06b`（基于 013 `65cc61e`）。六个 Scope 文件；Go 全量/race、vet、聚焦事件门槛与四张截图检查 PASS。011 将旧代理测试改名为 `TestProxyRulesForwardHeadersAndKeepLastGoodRouting`，本计划 Step3 已同步实际名字；实机系统信任留给 015-B。

集成记录：按用户“合并 master”的明确指示，实现提交链已于本轮快进合入 master（`96549ea`）。此前工作树记录保留为历史；未验证的原生安装、外部卷与系统信任项目仍保持原状态。
