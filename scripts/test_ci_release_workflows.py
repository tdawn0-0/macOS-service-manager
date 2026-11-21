import json
from pathlib import Path
import shutil
import subprocess
import unittest


ROOT = Path(__file__).resolve().parent.parent
CI_WORKFLOW = ROOT / '.github/workflows/ci.yml'
RELEASE_WORKFLOW = ROOT / '.github/workflows/github-actions-publish.yml'
PACKAGE_LINUX = ROOT / 'scripts/package-linux.sh'


def load_yaml(path: Path):
    """Parse a YAML file using PyYAML when installed, or Ruby's stdlib YAML."""
    try:
        import yaml  # type: ignore
    except ModuleNotFoundError:
        yaml = None

    if yaml is not None:
        return yaml.safe_load(path.read_text(encoding='utf-8'))

    ruby = shutil.which('ruby')
    if ruby is not None:
        raw = subprocess.check_output(
            [ruby, '-ryaml', '-rjson', '-e', 'puts JSON.generate(YAML.load_file(ARGV[0]))', str(path)],
            text=True,
        )
        return json.loads(raw)

    raise unittest.SkipTest('neither PyYAML nor ruby is available to parse YAML')


def step_runs(job):
    return '\n'.join(step.get('run', '') for step in job.get('steps', []) if isinstance(step, dict))


def step_uses(job):
    return [step['uses'] for step in job.get('steps', []) if isinstance(step, dict) and 'uses' in step]


class WorkflowValidationTests(unittest.TestCase):
    def test_ci_workflow_keeps_macos_and_adds_parallel_linux_job(self):
        doc = load_yaml(CI_WORKFLOW)
        jobs = doc['jobs']

        self.assertIn('test', jobs)
        self.assertIn('linux', jobs)

        macos = jobs['test']
        self.assertEqual(macos['runs-on'], 'macos-26')
        macos_script = step_runs(macos)
        self.assertIn('nsh package --target macos', macos_script)
        self.assertIn('verify-macos-requirements.py', macos_script)
        self.assertIn('collect-third-party-notices.py --check', macos_script)

        linux = jobs['linux']
        self.assertEqual(linux['runs-on'], 'ubuntu-24.04')
        self.assertNotIn('needs', linux)

        uses = step_uses(linux)
        self.assertTrue(any(u.startswith('actions/checkout@') for u in uses))
        self.assertIn('dtolnay/rust-toolchain@stable', uses)
        self.assertIn('Swatinem/rust-cache@v2', uses)

        cache_steps = [s for s in linux['steps'] if s.get('uses') == 'Swatinem/rust-cache@v2']
        workspaces = cache_steps[0]['with']['workspaces']
        for ws in ('app -> target', 'app/crates/devcore -> target', 'app/crates/devproxy -> target', 'app/crates/devcli -> target'):
            self.assertIn(ws, workspaces)

        linux_script = step_runs(linux)
        for pkg in (
            'libgtk-4-dev',
            'libpango1.0-dev',
            'libcairo2-dev',
            'libgdk-pixbuf-2.0-dev',
            'libgraphene-1.0-dev',
            'liblzma-dev',
            'pkg-config',
            'build-essential',
        ):
            self.assertIn(pkg, linux_script)
        self.assertIn('cargo fetch --locked', linux_script)
        self.assertIn('cargo test --locked', linux_script)
        self.assertIn('cargo clippy --locked --all-targets -- -D warnings', linux_script)
        self.assertIn('cargo fmt --check', linux_script)
        self.assertIn('scripts/package-linux.sh --skip-tests', linux_script)
        self.assertIn('collect-third-party-notices.py --check', linux_script)

    def test_release_workflow_builds_macos_and_linux_in_parallel_and_publishes_both(self):
        doc = load_yaml(RELEASE_WORKFLOW)
        jobs = doc['jobs']

        self.assertEqual(set(jobs.keys()), {'build-macos', 'build-linux', 'release'})

        macos = jobs['build-macos']
        self.assertEqual(macos['runs-on'], 'macos-26')
        self.assertEqual(macos['environment'], 'release')
        self.assertNotIn('needs', macos)
        macos_script = step_runs(macos)
        self.assertIn('scripts/package.sh --skip-tests', macos_script)
        self.assertIn('xcrun notarytool store-credentials', macos_script)
        self.assertIn('xcrun stapler validate "$dmg"', macos_script)
        self.assertIn('security delete-keychain', macos_script)
        self.assertIn('actions/upload-artifact@v4', step_uses(macos))

        linux = jobs['build-linux']
        self.assertEqual(linux['runs-on'], 'ubuntu-24.04')
        self.assertNotIn('needs', linux)
        linux_script = step_runs(linux)
        self.assertIn('DEVBASE_VERSION="$VERSION" scripts/package-linux.sh --skip-tests', linux_script)
        self.assertIn('sha256sum -c', linux_script)
        self.assertIn('actions/upload-artifact@v4', step_uses(linux))

        release = jobs['release']
        self.assertEqual(set(release['needs']), {'build-macos', 'build-linux'})
        self.assertEqual(release['environment'], 'release')
        self.assertEqual(release['permissions']['contents'], 'write')
        self.assertIn('actions/download-artifact@v4', step_uses(release))
        release_script = step_runs(release)
        self.assertIn('nsh update-manifest --entry app --key "$key_file" --out dist/manifest "$mac_archive" "$linux_archive"', release_script)
        self.assertIn('platforms["macos-aarch64"]', release_script)
        self.assertIn('platforms["linux-x86_64"]', release_script)
        self.assertIn('gh release create "$TAG" "$dmg" "$mac_archive" "$linux_archive" "$linux_checksum" "$manifest" "$manifest_sig"', release_script)

    def test_package_linux_script_syntax_and_cli_contract(self):
        subprocess.run(['bash', '-n', str(PACKAGE_LINUX)], check=True)
        help_out = subprocess.check_output(['bash', str(PACKAGE_LINUX), '--help'], text=True)
        self.assertIn('--skip-tests', help_out)
        self.assertIn('--out', help_out)
        self.assertIn('DEVBASE_VERSION', help_out)

        bad = subprocess.run(
            ['bash', str(PACKAGE_LINUX), '--unknown-flag'],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(bad.returncode, 0)
        self.assertIn('unknown argument', bad.stderr)

        script = PACKAGE_LINUX.read_text(encoding='utf-8')
        self.assertIn('version=${DEVBASE_VERSION#v}', script)
        self.assertIn('checksum="${archive}.sha256"', script)
        self.assertIn('nsh-updater.json', script)
        self.assertIn('app.json', script)


if __name__ == '__main__':
    unittest.main()
