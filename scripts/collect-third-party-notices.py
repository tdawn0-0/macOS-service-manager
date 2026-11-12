#!/usr/bin/env python3
"""Copy license/notice files from the resolved Go module cache, without network I/O.
Run after `go mod download`; use --check in a macOS release checkout.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import re
import sys

ROOT = Path(__file__).resolve().parent.parent
PREFIXES = ('LICENSE', 'LICENCE', 'UNLICENSE', 'NOTICE', 'COPYING', 'COPYRIGHT', 'AUTHORS', 'PATENTS', 'OFL')


def command(*args, **kwargs):
    environment = dict(kwargs.pop('env', os.environ), GOPROXY='off', GOTOOLCHAIN='local')
    return subprocess.check_output(args, cwd=ROOT, text=True, env=environment, **kwargs).strip()


def objects(raw):
    decoder = json.JSONDecoder()
    while raw.strip():
        value, length = decoder.raw_decode(raw.lstrip())
        yield value
        raw = raw.lstrip()[length:]


def toolchain_notice(goroot, name):
    candidates = [goroot / name]
    # Homebrew moves the Go LICENSE to the formula prefix, outside libexec.
    if goroot.name == 'libexec' and (goroot.parent / 'INSTALL_RECEIPT.json').is_file():
        candidates.append(goroot.parent / name)
    for path in candidates:
        if path.is_file() and not path.is_symlink():
            data = path.read_bytes()
            if data and b'\0' not in data and len(data) <= 4 * 1024 * 1024:
                return data
    if name == 'LICENSE':
        raise ValueError('Go toolchain LICENSE is unavailable; PATENTS alone is insufficient')
    return None


def asset_notices(root):
    files, assets = {}, []
    for source in sorted((root / 'assets' / 'notices').glob('*/source.json')):
        metadata = json.loads(source.read_text())
        identifier = source.parent.name
        revision = metadata['licenseRevision']
        repository = metadata['repository']
        expected_url = repository.replace('https://github.com/', 'https://raw.githubusercontent.com/') + '/' + revision + '/LICENSE'
        if len(revision) != 40 or any(c not in '0123456789abcdef' for c in revision) or metadata['licenseSource'] != expected_url:
            raise ValueError('Asset license must pin its upstream revision: ' + identifier)
        license_file = source.parent / 'LICENSE'
        if license_file.is_symlink():
            raise ValueError('Asset license must be a plain file: ' + identifier)
        data = license_file.read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        if not data or b'\0' in data or len(data) > 4 * 1024 * 1024 or digest != metadata['licenseSHA256']:
            raise ValueError('Asset license digest differs: ' + identifier)
        uses = metadata['usedBy']
        if not uses:
            raise ValueError('Asset license must identify its usage: ' + identifier)
        for use in uses:
            path = Path(use)
            if path.is_absolute() or '..' in path.parts or not (root / path).is_file():
                raise ValueError('Asset usage is missing or outside the repository: ' + identifier)
        target = 'third-party-assets/' + identifier + '/LICENSE'
        files[target] = data
        assets.append(dict(metadata, files=[{'path': target, 'sha256': digest}]))
    # icons.go explicitly imports Lucide paths; omitting metadata must fail closed.
    if (root / 'icons.go').is_file() and 'lucide.dev' in (root / 'icons.go').read_text() and not any('icons.go' in a['usedBy'] for a in assets):
        raise ValueError('Lucide icons require their distribution license')
    return files, assets


def collect():
    files = {}
    modules = []
    used = set()
    for arch in ('arm64',):
        environment = dict(os.environ, GOOS='darwin', GOARCH=arch)
        for package in objects(command('go', 'list', '-mod=readonly', '-deps', '-json', './...', env=environment)):
            module = package.get('Module')
            if module and not module.get('Main'):
                used.add(module['Path'])
    for module in sorted(objects(command('go', 'list', '-mod=readonly', '-m', '-json', 'all')), key=lambda x: x['Path']):
        if module.get('Main') or module['Path'] not in used:
            continue
        if module.get('Replace'):
            raise ValueError('Review replaced modules before generating distribution notices: ' + module['Path'])
        if not module.get('Dir'):
            raise ValueError('Download the pinned module before collecting notices: ' + module['Path'])
        source = Path(module['Dir'])
        target = Path('third-party') / (module['Path'] + '@' + module['Version'])
        if target.is_absolute() or '..' in target.parts:
            raise ValueError('Invalid module destination')
        entries = []
        for item in sorted(source.rglob('*')):
            if not item.name.upper().startswith(PREFIXES) or not item.is_file():
                continue
            if item.is_symlink() or item.stat().st_size > 4 * 1024 * 1024:
                raise ValueError('Review the unusual notice file: ' + str(item))
            data = item.read_bytes()
            # Empty directory placeholders and binary files are not notices.
            if not data or b'\0' in data:
                continue
            relative = (target / item.relative_to(source)).as_posix()
            files[relative] = data
            entries.append({'path': relative, 'sha256': hashlib.sha256(data).hexdigest()})
        if not entries:
            raise ValueError('No license/notice files found for ' + module['Path'])
        modules.append({'module': module['Path'], 'version': module['Version'], 'sum': module.get('Sum', ''), 'files': entries})
    goroot = Path(command('go', 'env', 'GOROOT'))
    version = command('go', 'env', 'GOVERSION')
    go_mod_path = ROOT / 'go.mod'
    if go_mod_path.is_file():
        match = re.search(r'(?m)^go\s+(\d+\.\d+(?:\.\d+)?)', go_mod_path.read_text())
        if match:
            mod_ver = 'go' + match.group(1)
            if go_minor_series(mod_ver) == go_minor_series(version):
                version = mod_ver
    entries = []
    for name in ('LICENSE', 'PATENTS'):
        data = toolchain_notice(goroot, name)
        if data:
            relative = 'go-toolchain/' + name
            files[relative] = data
            entries.append({'path': relative, 'sha256': hashlib.sha256(data).hexdigest()})
    asset_files, assets = asset_notices(ROOT)
    files.update(asset_files)
    manifest = {'schema': 1, 'go': version, 'scope': 'app and CLI production Go modules and attributed assets for darwin arm64; copied notices retain their original terms', 'toolchain': entries, 'modules': modules, 'assets': assets}
    files['manifest.json'] = (json.dumps(manifest, indent=2, ensure_ascii=False) + '\n').encode()
    lines = ['# Third-party notices', '', 'Generated from the pinned app/CLI production module graph for macOS arm64.',
             'Each component retains its original license and copyright. Nested notice files are copied as provided.',
             'This inventory does not relicense components or claim that every copied file is linked into the executable.',
             'Downloaded development runtimes/databases are separate; see docs/third-party-software.md in the source repository.', '',
             'Regenerate: `python3 scripts/collect-third-party-notices.py`; verify: add `--check`.',
             'The SHA-256 inventory is in [manifest.json](manifest.json). No local cache paths are included.', '',
             '| Module | Version | Copied notices |', '| --- | --- | --- |']
    for module in modules:
        lines.append('| ' + module['module'] + ' | ' + module['version'] + ' | ' + str(len(module['files'])) + ' |')
    if assets:
        lines += ['', '| Asset | Upstream license snapshot | License |', '| --- | --- | --- |']
        for asset in assets:
            lines.append('| ' + asset['name'] + ' | [' + asset['licenseRevision'][:12] + '](' + asset['licenseSource'] + ') | [notice](' + asset['files'][0]['path'] + ') |')
    lines += ['', 'Go toolchain: ' + version + '; see [license](go-toolchain/LICENSE).', '']
    files['README.md'] = '\n'.join(lines).encode()
    return files, modules


def go_minor_series(version: str) -> str | None:
    match = re.match(r'^(go\d+\.\d+)(?:\.\d+)?$', version)
    return match.group(1) if match else None


def verify_inventory(files: dict, target: Path, runtime_go_version: str) -> None:
    expected = dict(files)
    manifest_path = target / 'manifest.json'
    if manifest_path.is_file():
        try:
            recorded_manifest = json.loads(manifest_path.read_text())
            recorded_go = recorded_manifest.get('go')
            if isinstance(recorded_go, str):
                rec_series = go_minor_series(recorded_go)
                run_series = go_minor_series(runtime_go_version)
                if rec_series and run_series and rec_series == run_series:
                    if 'manifest.json' in expected:
                        manifest_data = json.loads(expected['manifest.json'].decode())
                        manifest_data['go'] = recorded_go
                        expected['manifest.json'] = (json.dumps(manifest_data, indent=2, ensure_ascii=False) + '\n').encode()
                    if 'README.md' in expected:
                        readme_text = expected['README.md'].decode()
                        readme_text = re.sub(
                            r'Go toolchain: go\d+\.\d+(?:\.\d+)?;',
                            f'Go toolchain: {recorded_go};',
                            readme_text,
                            count=1,
                        )
                        expected['README.md'] = readme_text.encode()
        except json.JSONDecodeError:
            pass

    mismatches = [name for name, content in expected.items() if not (target / name).is_file() or (target / name).read_bytes() != content]
    extra = [p.relative_to(target).as_posix() for p in target.rglob('*') if p.is_file() and p.relative_to(target).as_posix() not in expected] if target.exists() else []
    if mismatches or extra:
        raise ValueError('Notice inventory differs; regenerate and review: ' + ', '.join(mismatches + extra))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    files, modules = collect()
    target = ROOT / 'licenses'
    if args.check:
        runtime_version = command('go', 'env', 'GOVERSION')
        verify_inventory(files, target, runtime_version)
    else:
        if target.exists():
            # Do not erase unrecognized files. A maintainer reviews removed modules explicitly.
            extra = [p.relative_to(target).as_posix() for p in target.rglob('*') if p.is_file() and p.relative_to(target).as_posix() not in files]
            if extra:
                raise ValueError('Review obsolete or extra notice files before regeneration: ' + ', '.join(extra))
        for name, content in files.items():
            destination = target / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(content)
    print(('Verified' if args.check else 'Collected') + f' {len(modules)} modules, {len(files)} distribution notice files')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
