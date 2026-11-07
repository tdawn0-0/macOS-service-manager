# 运行环境与 HTTPS 反代：新计划索引

当前集成状态：用户已授权合并，`master` 已快进至 `96549ea`，包含 011–018 的全部实现提交。以下各阶段记录中的“隔离工作树／未合并”是实施时的历史状态；当前代码已在主工作树。015 的原生 runner、签名／系统信任／Chrome 实机验收仍未完成，合并不代表验收通过。

由 improve 技能于 2026-11-08 生成。规划基线 `2ec48ab`；开始时工作区干净。用户已选择全部 7 项新计划，并明确清理旧计划、不考虑旧项目兼容性。本轮只编写计划，未实施源码、运行应用测试或发布。

## 已确定的产品边界

- 三个主入口：**Software / Databases / HTTPS**。软件管理安装版本与终端使用；数据库管理独立实例；HTTPS 仅配置域名到本地 HTTP 端口的规则。没有独立站点实体、列表/详情或生命周期。
- 应用由用户在终端或 IDE 启动。删除注册项目、服务编排、应用命令执行、项目依赖安装、自启动、环境导入导出和项目锁文件。
- 保留 Node 按目录声明选版本；它不需要注册项目。移除 PHP 安装、版本选择、仓库添加与终端集成。
- 数据库和 HTTPS proxy 具有各自生命周期；规则条数不作为运行进程数。旧系统服务管理保留为次要入口。
- 软件主界面调整为 Node.js、MySQL、PostgreSQL、Redis、MongoDB 五类；来源和 macOS arm64/Go/MyGo 技术栈不变。
- 无旧 schema/项目配置兼容层、迁移器或双模型过渡。使用新隔离状态验收。删除旧计划和旧模型不等于授权删除本机真实数据库、共享软件或未知遗留进程。
- 不新增项目替代实体、通用应用进程管理、团队模板、云端同步、隧道、SQL IDE 或 FastCGI。

## 执行顺序与状态

编号从 011 延续，避免旧审查编号被复用。审视编号 1–7 分别对应 011–017。每份文件都包含产品契约、基线摘录、允许文件、步骤、测试和停止条件，执行者无需读取旧计划。

| 计划 | 目标 | 优先级 | 工作量 | 硬依赖 | 状态 |
|---|---|---|---|---|---|
| [011](011-remove-project-model.md) | 删除项目、PHP 和站点模型，建立反代规则基础 | P1 | L | 015-A 完成 | DONE（隔离工作树） |
| [012](012-standalone-runtime-terminal.md) | Node.js 独立终端使用、默认版本和启用状态 | P1 | M | 011 | DONE（隔离工作树） |
| [013](013-independent-resource-lifecycle.md) | 数据库/代理独立生命周期、菜单栏和退出 | P1 | L | 011 | DONE（隔离工作树） |
| [014](014-simple-https-proxy.md) | 单页域名→本地端口规则表、整体保存与应用 | P1 | L | 011、013 | DONE（隔离工作树） |
| [015](015-resource-product-acceptance.md) | A：先定义验收契约；B：完整验收与文档收口 | P1 | L | A 无；B 依赖 011–014、016、017 | BLOCKED：B 实机验收环境缺失 |
| [016](016-https-activation-diagnostics.md) | 代理/规则分层检查、显式 HTTP 检查和诊断跳转 | P2 | L | 011、013、014 | DONE（隔离工作树） |
| [017](017-database-connection-profiles.md) | 独立实例的无密码连接配置和 Copy/Open | P2 | M–L | 011、013 | DONE（隔离工作树） |

工作量为粗估，M 约一天、L 多天，包含测试；不是交付承诺。012/014/016 含新产品契约，先完成各计划内的状态/输入矩阵再接 UI，DONE 表示实现与验证完成，不是只有设计或原型。

**默认顺序：015-A → 011 → 012 → 013 → 014 → 016 → 017 → 015-B。**

015-A 只创建产品范围与验收契约文档，完成后将 015 标记为 `IN PROGRESS — A DONE; B waiting` 并记录提交；不需要等待 015 整体 DONE 即可启动 011。015-B 完成所有验证才将整项标 DONE。因此依赖关系没有循环。

## 交接边界与重叠文件

- **011 是一次完整删除**：包括 Site/Sites/单站点 CRUD、PHP catalog/安装路径、数据库项目依赖与原生夹具同步；一次替换为 ProxyRules 整表 API。它必须交付可编译基础，不用空壳 Project/兼容接口让下游承担断裂。
- **013/014 不重复删除模型**：013 增强独立生命周期和菜单栏；014 完成单页规则表、整表草稿与 Published/Pending/Unknown；规则没有持久化站点 ID 或独立启停。011 交接时记录真实 API 和 helper 移动位置。
- **014 → 016**：014 的 Published 只表示配置已发布；016 分别验证 DNS、证书、上游及 HTTP。不能把发布记录当浏览器可达证明。
- **013 → 017**：017 在独立实例生命周期事务里清理 metadata profile。只删 profile 不删除真实数据库或用户；不存业务密码。
- **012/013/014/016/017 共享 view_development.go/model.go 等文件**：推荐串行。若以后并行，执行者必须使用隔离 worktree，且在当前依赖提交上重核范围；本索引不授权自动并行、合并或发布。
- **011 与 015-B 的工具改动分工**：011 同步原生 fixture、required 测试新名及五软件矩阵，删除 PHP 准备/验证步骤；015-B 添加新全链路要求、收紧 parser 和最终文档，保留其余五类软件的真实性要求。
- 允许将共享 helper 从待删项目文件移入通用内部文件；这属于删除闭包，不是对整个系统服务模块的大重构。

## 验证口径

`check-test-events.py` 是只读验收辅助工具，不是第八份实施计划。它检查 Go JSON 完整 PASS、子用例及本次截图，防止空跑。可运行 `python3 plans/check-test-events.py --self-test` 检查工具自身。

每份计划列出的新测试名是未来实现契约，本轮没有创建这些测试，也没有宣称通过。执行前先用 go test -list 确认名称；空匹配、仅编译或 SKIP 不计通过。

现有检查入口：

```sh
go test ./... -run '^$'
go vet ./...
go test ./...
go test -race ./...
go mod verify
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/collect-third-party-notices.py --check
go tool mygo build -sign - -skip-notarize
```

完整 Go 套件存在真实 launchd/数据库操作。先跑各计划的隔离无特权聚焦测试；完整与原生验收在可清理的隔离 macOS 环境执行。`scripts/verify-native-release.py --run` 默认只在一次性 GitHub-hosted runner 运行，必须所有 required 父/子项 PASS、零 SKIP。

实际系统信任/443 授权/Chrome 测试是独立验收层：mock、私有测试 CA、ad-hoc build 都不能代替；没有实测保持 NOT RUN，015-B 不得 DONE。日志/截图使用合成数据，原始机器日志留在 build/ 或私有临时目录，不进计划。

## 旧计划清理记录

按用户要求，已移除旧 001–010 计划文件及 `spikes/009-package-managers`、`spikes/010-diagnostic-navigation` 的旧设计/原型，不在当前目录保留归档副本。旧内容可从 Git 基线 `2ec48ab` 查看，例如 `git show 2ec48ab:plans/README.md`。

- 旧计划中已经落地的权限、数据安全、shell symlink 和验证改进仍属于现有代码；清理文档不撤销它们。
- 旧 009 的项目依赖安装方向停止；不转成新软件管理器或命令执行器。
- 旧 010 的诊断导航思路在 016 中按独立资源重新完整说明；不引用旧文件或原型作为执行前提。
- 旧项目相关验证的保留/删除映射由 011 记录；新完整门槛由 015 负责。旧版本 PASS 不计入新产品验收。

## 已考虑但不纳入本轮

- 只隐藏 Projects 页面：后端项目状态、worker 和所有权仍在，不能实现收缩。
- 把 Project 换名 Environment/Stack：仍会恢复目录、命令、依赖编排负担。
- 自动迁移旧项目/原地清库：用户不要求兼容，且清理计划不是清理真实数据。
- PHP 全部版本管理、独立站点管理、数据库登录自启：前两项已按本次决定移除，后者不在本轮范围。
- 新包管理器/Composer/FastCGI/隧道：超出本轮明确的产品责任。
- 立即删除 Go/Valkey 的底层支持：不作为取消项目的必要条件，避免把未选择的软件目录变更混入。
- 全面代码安全/性能/供应链重审：本轮是产品方向与实施交接，未重新完成这些审计。

## 状态与执行记录

状态使用 TODO、IN PROGRESS、DONE、BLOCKED（具体原因）、REJECTED（具体理由）。每个执行者更新自己的行与文件末尾 Execution record。记录实际基线、预期前置漂移、验证命令、PASS/SKIP、截图及未验证边界。

2026-11-08 计划质量检查：7 份计划的模板章节、逐步验证门槛、基线、Markdown 链接和 shell 代码块语法通过静态检查；`git diff --check` 通过。`check-test-events.py --self-test` 通过，包含完整事件流及 9 个失败模式。独立只读复核提出的问题已修订，包括此前 PHP 默认失效行为（现已取消）、草稿跳转、异步连接配置、空站点发布、测试空跑和实机验收隔离。

本轮仅修改 `plans/`；旧 001–010 与两组旧原型已移除。未运行源码测试、构建、原生验收或系统修改，当前没有任何源码实施结果。此轮聚焦产品方向，未重新进行全面安全、性能或供应链审计。

2026-11-08 范围修订：按用户确认移除 PHP 版本管理和独立站点管理，保留 HTTPS 的域名→本地端口规则配置。012 缩为 Node；014 已改写并重命名为 simple-https-proxy；011/013/015/016/017 的契约同步更新。本次已重新进行独立只读复核并修正验收 ID、整表 API 与 listener port 原子保存契约。7 份计划的结构、链接、逐步验证、shell 代码块语法及旧引用检查通过；`git diff --check` 通过。未运行源码测试或应用。

015-A 已在隔离工作树 `/Users/jyeu/code/service-manager-015a` 提交 `ad9f773`：新增 `docs/product-scope.md` 与九项全为 NOT RUN 的 `docs/resources-acceptance.md`。复核了完整 diff、范围和 A1 命令，结果 PASS；未合并到用户主工作树。011 可从该提交继续。

011 已在隔离工作树 `/Users/jyeu/code/service-manager-011` 提交至 `f987c0c`（包含 015-A）；全包 Go/race、vet、Python 31 项、MyGo 构建和新增聚焦测试通过。原生 runner 完整执行留给 015-B，未作为 011 的实机通过宣称。代码尚未合并到用户主工作树。

012 已在隔离工作树 `/Users/jyeu/code/service-manager-012` 提交 `60c1b3a`；Go 全量、vet、新增聚焦 race 与四张布局截图门槛通过；复核了差异范围和核心 UI/测试实现。原生 runner 留给 015-B。

013 已在隔离工作树 `/Users/jyeu/code/service-manager-013` 提交 `65cc61e`；Go 全量、完整 race 第二次、vet、聚焦事件门槛和四张截图通过。首次完整 race 的 Node UI 临时目录清理失败在单项重复与完整复跑中未重现，保留记录。实机原生留给 015-B。

014 已在隔离工作树 `/Users/jyeu/code/service-manager-014` 提交 `a52d06b`；Go 全量/race、vet、聚焦事件与截图门槛 PASS；复核空表发布、发布状态比较及 Scope。015-B 实机仍 NOT RUN。

016 已在隔离工作树 `/Users/jyeu/code/service-manager-016` 提交 `8991776`；Go 全量/race、vet、聚焦事件/截图门槛 PASS；最后 JSON tag/测试调整后聚焦门槛重跑通过，完整 race 是此前的通过结果。系统信任实测留给 015-B。

017 已在隔离工作树 `/Users/jyeu/code/service-manager-017` 提交 `d76d545`；Go 全量、race（末尾仅布局测试扩展前）、vet、扩展后聚焦 race/截图门槛 PASS；实机原生留给 015-B。

015-B 的代码、文档及截图已在隔离工作树 `/Users/jyeu/code/service-manager-015b` 提交 `c4246ae`；Go 全量/race、vet、mod verify、Python 31 项、notice、MyGo ad-hoc 构建及五组 UI 截图门槛通过。完整原生 `verify-native-release.py --run` 未运行：当前非一次性 GitHub-hosted macOS runner。AC-NATIVE-TRUST 未运行：缺同提交 Developer ID 签名 App 与获准使用的一次性 macOS/Chrome 测试机。九项 AC 继续 NOT RUN，015 整体 BLOCKED。任何实际 hosts/系统信任更改均未执行。

## 新增范围：运行时与数据库目录

用户已授权 [018](018-runtimes-and-database-locations.md)：Java/Go 安装版本管理、uv 驱动的 Python 管理，以及数据库多版本实例与自定义数据目录。基于最终功能提交 `c4246ae` 实施，取代旧五软件产品边界；原 015 实机验收仍 BLOCKED。状态：DONE（隔离工作树；原生安装/外部卷实机未验证）。

018 最终提交 `96549ea`，工作树 `/Users/jyeu/code/service-manager-018`，含此前所有功能提交。Java/Go 安装与默认版本、uv Python 管理、数据库外部目录与版本隔离已实施；完整 Go/race与聚焦门槛、构建通过。详见该工作树 `docs/development-runtimes-and-data.md` 的实际能力和验证边界。
