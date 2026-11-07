# Plan 002: 让原生验收准备脚本使用专用测试入口

> **执行说明**：这是给独立执行者的交接计划。逐步实施并执行每一步的验证；不要把测试名称匹配到零个测试当作成功。遇到 STOP 条件立即报告。完成后仅更新 plans/README.md 中本计划一行，并附实际验证结果。当前文件描述待执行工作，不代表已经修复。
>
> **漂移检查（先执行）**：在仓库根运行 `git diff --stat 0cbf7e9..HEAD -- scripts/prepare-native-ui-verification.py scripts/test_prepare_native_ui_verification.py docs/development-ui.md` 和 `git diff -- scripts/prepare-native-ui-verification.py scripts/test_prepare_native_ui_verification.py docs/development-ui.md`，再运行 `git status --short`。基线时工作区干净。检查所有已提交及未提交变化；路径有变化时逐项比对下方摘录，不能直接覆盖。前置计划造成的预期变化也要记录；行为或接口不再匹配时停止更新计划。

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW — 仅验证工具的调用路径变化
- **Depends on**: 无；008 的原生验收工具依赖本计划
- **Category**: dx
- **Planned at**: commit `0cbf7e9`, 2026-11-07
- **Status**: TODO
- **Confidence**: HIGH（源码证据；本计划指定的新增测试尚未实现）

## Why this matters

0cbf7e9 将打包 App 的 --dev 收敛为内部命令，但原生验收准备脚本仍向它发送 log-rotation、install-manifest、instance、project。脚本因此在第一条准备命令失败。使用 cmd/devmgr 完成夹具准备，继续用复制的 App 做真实 UI 验收；保持产品“无对外 CLI”的决定。

## Current state

引用源码也属于漂移检查范围（只读，不扩大 Scope）：`git diff --stat 0cbf7e9..HEAD -- scripts/prepare-native-ui-verification.py internal/devcli/cli.go cmd/devmgr/main.go scripts/test_collect_third_party_notices.py`；另用 `git diff -- scripts/prepare-native-ui-verification.py internal/devcli/cli.go cmd/devmgr/main.go scripts/test_collect_third_party_notices.py` 检查未提交变化。下列摘录是精确基线，执行前逐项核对。

仓库根为 `/Users/jyeu/code/service-manager`；在独立 checkout 中使用实际根目录。产品是 Go 1.27.3 + MyGo 的 macOS Apple Silicon 应用，长生命周期服务交给 launchd；项目、实例和软件包由 internal/devplatform 管理。用户操作在 GUI 内完成；cmd/devmgr 是未随 App 分发的测试 harness，不能恢复为公开 CLI。仓库尚未发布，不增加旧来源迁移或兼容层。

### scripts/prepare-native-ui-verification.py:19 — 当前参数

```python
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--app', type=Path, default=Path('build/darwin-arm64/MacOS Service Manager.app'))
parser.add_argument('--node-root', type=Path, default=Path('/tmp/msm-development-verification'))
parser.add_argument('--redis-root', type=Path, default=Path('/tmp/msm-six-verification'))
parser.add_argument('--reuse-ca', type=Path, required=True, help='Existing development root; never changes its material or trust')
args = parser.parse_args()
```

### scripts/prepare-native-ui-verification.py:37 — App 被误用为全功能 CLI

```python
info['LSEnvironment'] = {'MSM_DEV_HOME': str(root)}
info_path.write_bytes(plistlib.dumps(info))
subprocess.run(['codesign', '--force', '--deep', '--sign', '-', str(app)], check=True, capture_output=True)
subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
exe = app / 'Contents/MacOS' / info['CFBundleExecutable']
env = dict(os.environ, MSM_DEV_HOME=str(root))
def cli(*values):
    result = subprocess.run([str(exe), '--dev', *map(str, values)], env=env, text=True, capture_output=True, timeout=180)
    if result.returncode:
        raise RuntimeError('Fixture command failed: ' + str(values[0]) + '\n' + result.stderr)
    return result.stdout
```

### scripts/prepare-native-ui-verification.py:64 — 第一条被拒绝的命令

```python
cli('log-rotation', 'off')
packages = {}
for name, version, cache in [('node', '22.14.0', args.node_root), ('redis', '7.4.7', args.redis_root)]:
    identity = name + '@' + version + '-arm64'
    manifest = json.loads((cache / 'packages' / identity / '.package.json').read_text())
    archive = cache / 'downloads' / manifest['sha256']
    assert hashlib.sha256(archive.read_bytes()).hexdigest() == manifest['sha256']
    shutil.copyfile(archive, root / 'downloads' / manifest['sha256'])
    cli('install-manifest', configuration(name + '-package', manifest))
```

### internal/devcli/cli.go:21 — 内部入口允许列表与测试 Run 区分

```go
var internalCommands = map[string]bool{
	"serve":           true, // the HTTPS proxy
	"maintain":        true, // log rotation
	"up":              true, // a project that starts at login
	"command-worker":  true, // project commands that outlive the window
	"database-worker": true, // database work that outlives the window
	"node":            true, // the Node.js version for a directory, for the shell setup
}

// RunInternal is what the app executable runs for --dev.
func RunInternal(ctx context.Context, args []string, out io.Writer) error {
	if len(args) == 0 || !internalCommands[args[0]] {
		return errors.New("this program has no command line; use the window")
	}
	return Run(ctx, args, out)
}

// Run is every command, for tests and the verification scripts through
// cmd/devmgr. It is not shipped in the app and not a maintained interface.
```

### cmd/devmgr/main.go:18 — 测试 harness 接受内部验证命令

```go
	defer cancel()
	args := os.Args[1:]
	if len(args) > 0 && args[0] == "--dev" {
		args = args[1:]
	}
	if e := devcli.Run(ctx, args, os.Stdout); e != nil {
		fmt.Fprintln(os.Stderr, e)
		os.Exit(1)
	}
```

### scripts/test_collect_third_party_notices.py:8 — stdlib unittest + importlib + 临时目录范式

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

用 Python 标准库 argparse、pathlib、subprocess、unittest，不增加依赖。将脚本顶层副作用放入 main(argv=None)，保留 if __name__ == "__main__" 入口，让测试可以导入。真实 GUI 仍经 Launch Services 打开复制的 App；不使用测试 harness 替代待验的原生 App。保留私有根目录 0700、显式 reuse-ca、已有 checksum 检查和不修改 hosts/信任边界。

来自 CONTRIBUTING.md 的约束：“Use isolated roots and ports in tests. Stop and unregister every test job.” 真实 hosts、钥匙串、管理员授权、重启/注销不属于自动执行范围。只能在明确安排的原生验收中操作；不能为让测试通过关闭 TLS 验证、扩大权限或接管原有数据库。测试数据不引用真实项目、密钥或凭据；发现秘密只记录文件位置和类型，不复制值。

## Commands you will need

所有命令从仓库根执行，要求 darwin/arm64、Go 1.27.3 和需要时的 Python 3。无需升级依赖或安装新的测试框架。

| 用途 | 命令 | 成功条件 |
|---|---|---|
| 工具链 | `go version` | Go 1.27.3 或满足 go.mod 的版本，darwin/arm64 |
| 依赖完整性 | `go mod verify` | exit 0，all modules verified |
| 聚焦验证 | `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_prepare_native_ui_verification.py' -v` | exit 0，指定新增用例实际执行且无 SKIP |
| 静态检查 | `go vet ./...` | exit 0 |
| 差异格式 | `git diff --check -- scripts/prepare-native-ui-verification.py scripts/test_prepare_native_ui_verification.py docs/development-ui.md plans/README.md` | exit 0 |
| 完整回归（隔离验收） | `go test ./...` 和 `go test -race ./...` | exit 0；单列 opt-in SKIP |
| 原生构建（仅代码计划需要时） | `go tool mygo build -sign - -skip-notarize` | exit 0，产物在忽略的 build/ 中；不签发版本、不公证、不发布 |

本计划的必要门槛是下列聚焦测试、go vet 和差异检查。完整 go test 包含真实 launchd 启停；仅在隔离、可清理的 macOS 验证会话中运行，并记录所有 SKIP。

审查阶段已验证过 go vet、go mod verify、7 项隔离 Node 测试及 4 项 Python 测试；不能把这些历史结果视为本计划实施后的验收结果。

## Scope

**允许修改的文件（新增文件也必须在此列出）**：

- scripts/prepare-native-ui-verification.py
- scripts/test_prepare_native_ui_verification.py
- docs/development-ui.md
- plans/README.md（仅本计划状态与验证结果）

**禁止修改**：上述列表之外的产品源码、go.mod/go.sum、发布凭据、软件目录版本/来源、用户真实配置和数据库。只读引用文件不等于获准修改。

## Git workflow

- 在独立分支 `codex/002-repair-native-qa-harness` 执行。保留已有工作；不要切换或重置包含他人修改的工作树。
- 提交不是计划的完成前提；仅在操作者要求时提交。仓库提交标题以直接描述行为为主，例如 `Restrict the shipped app to internal commands`。
- 不自动合并、推送、创建 PR、发布 issue 或触发签名发行。

## Steps

### 步骤 1: 分离参数与可测试的调用函数

增加必填 --devmgr PATH，解析为已存在的可执行普通文件。在复制 App、签名或创建夹具之前校验该参数；不要静默使用 PATH 上的工具或打包 App。用 main(argv=None) 包装副作用，提取一个接收 harness 路径、root、参数的调用函数。新增 unittest 使用 mock subprocess.run 和 TemporaryDirectory，不启动数据库或 codesign。

**验证**：`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_prepare_native_ui_verification.py' -v` → 导入无副作用，缺失/不可执行 harness 在创建夹具前报错；对应测试通过

### 步骤 2: 切换准备命令并保护清理边界

cli 函数用传入 harness 执行 log-rotation off、install-manifest、instance、project，MSM_DEV_HOME 始终为新隔离 root。harness 可接受 --dev，但建议直接传测试子命令。exe 只表示待启动 App 的原生可执行文件，native-ui.json 保持该含义；如记录 harness 路径，用独立键。保留检查成功后才发布完整 native-ui.json 的顺序。失败清理只处理此次创建的资源；若已初始化后台任务，先用 harness 或已知 root 标签清理，不能扫描并停止其他服务；确认清理失败时保留隔离目录和错误信息，避免删除仍使用中的文件。

**验证**：`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_prepare_native_ui_verification.py' -v` → 模拟四种命令均路由到 harness；App 路径只用于启动记录；失败不留下假成功记录

### 步骤 3: 更新原生验收说明

在 docs/development-ui.md 记录先运行 go build -o build/qa-devmgr ./cmd/devmgr，再带 --devmgr build/qa-devmgr 执行脚本。--app、--node-root、--redis-root、--reuse-ca 必须使用验收者确认的隔离缓存与 CA 来源；不要把本机私人路径写成通用默认成功步骤。指出 --help 不进行签名/复制/下载，真实夹具准备不纳入普通 unit tests。

**验证**：`PYTHONDONTWRITEBYTECODE=1 python3 scripts/prepare-native-ui-verification.py --help` → exit 0，帮助包含 --devmgr 且没有创建夹具/调用 codesign

### 步骤 4: 验证现有 Python 工具不回归

检查标准 unittest 全集。不要更改 RunInternal 允许列表来迁就脚本，也不要发布 cmd/devmgr。原生 App 的构建/签名/数据库演练只在后续隔离会话执行；本计划应附用于该会话的完整命令模板。

**验证**：`PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py' -v` → 既有 4 项 notice 测试与新增验收脚本测试全部通过

## Test plan

新增脚本测试至少覆盖：导入无副作用、harness 参数缺失/无执行权限、四条准备命令的 executable 与 MSM_DEV_HOME、原生 executable 记录不被 harness 替换、返回码失败不写成功记录、异常清理仅覆盖本次 root。mock 使用 subprocess.CompletedProcess，不依赖本机 build/ 或缓存存在。

## Done criteria

以下全部满足才可记为 DONE：

- [ ] 新脚本测试至少覆盖上述六类情形且全部通过。
- [ ] --help 显示 --devmgr，执行不创建目录或子进程。
- [ ] main.go 和 internal/devcli/cli.go 没有修改；App 的允许列表保持限制。
- [ ] 文档含独立 harness 构建命令与完整参数示例，并区分模拟验证和实机验收。
- [ ] 聚焦验证命令实际执行新增用例且通过；无零用例假通过。
- [ ] `go vet ./...` 与 `git diff --check` 通过；未解决的原生验收限制如实记录。
- [ ] `git status --short` 和相对执行起点的 diff 仅包含 Scope 内文件；不得顺手格式化整个仓库。
- [ ] plans/README.md 中该计划状态及验证说明已更新。

## STOP conditions

- 当前代码与摘录不一致，且变化影响实施假设；先报告差异并刷新计划。
- 必须修改 Scope 外文件、引入依赖或扩大产品能力才能完成。
- 合理修正后同一个验证仍连续失败两次；记录命令与错误，不跳过门槛。
- 准备流程要求增加 App 的对外入口，或必须修改产品运行时才能准备夹具。
- 缺少确认可用的真实 package cache/CA，停止实机部分，不下载不明包或借用用户现有服务。
- 清理无法确认任务归属；保留私有目录并报告，不扩大停止范围。

## Maintenance notes

以后新增内部命令限制时，脚本仍应依赖测试 harness。测试不要只检查字符串里有 devmgr，要断言实际 subprocess argv 与环境。008 若接入此工具，应使用独立 root 和已验证缓存。
