#!/usr/bin/env python3
"""Copy license/notice files from the fetched Cargo registry, without network I/O.
Run `cargo fetch --locked` in each workspace first; use --check in a macOS release checkout.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
PREFIXES = ('LICENSE', 'LICENCE', 'UNLICENSE', 'NOTICE', 'COPYING', 'COPYRIGHT', 'AUTHORS', 'PATENTS', 'OFL')
TARGET = 'aarch64-apple-darwin'
CRATES_IO = 'registry+https://github.com/rust-lang/crates.io-index'


def command(*args, cwd):
    environment = dict(os.environ, CARGO_NET_OFFLINE='true', CARGO_TERM_COLOR='never')
    return subprocess.check_output(args, cwd=cwd, text=True, env=environment).strip()


def workspaces():
    """The app workspace and every crate workspace with its own Cargo.lock."""
    roots = [ROOT / 'app'] + sorted((ROOT / 'app' / 'crates').glob('*/Cargo.toml'))
    roots = [root.parent if root.name == 'Cargo.toml' else root for root in roots]
    return [root for root in roots if (root / 'Cargo.lock').is_file()]


def metadata(root):
    raw = command('cargo', 'metadata', '--offline', '--locked', '--filter-platform', TARGET,
                  '--format-version', '1', cwd=root)
    return json.loads(raw)


def production_ids(data):
    """Workspace members and their normal/build dependencies for this target.

    Dev-dependency edges are skipped, as `go list -deps ./...` skipped test-only
    imports in the Go tree.
    """
    nodes = {node['id']: node for node in data['resolve']['nodes']}
    seen = set()
    stack = list(data['workspace_members'])
    while stack:
        package = stack.pop()
        if package in seen:
            continue
        seen.add(package)
        for dependency in nodes.get(package, {}).get('deps', []):
            if any(kind.get('kind') in (None, 'build') for kind in dependency.get('dep_kinds', [])):
                stack.append(dependency['pkg'])
    return seen


def license_notices(source, target):
    """Notice files under `source`, keyed by their path below `target`."""
    files, entries = {}, []
    for item in sorted(source.rglob('*')):
        if not item.name.upper().startswith(PREFIXES) or not item.is_file():
            continue
        if item.is_symlink():
            raise ValueError('Notice files must be plain files: ' + str(item))
        if item.stat().st_size > 4 * 1024 * 1024:
            raise ValueError('Review the unusual notice file: ' + str(item))
        data = item.read_bytes()
        # Empty placeholders and binary files are not notices.
        if not data or b'\0' in data:
            continue
        relative = (target / item.relative_to(source)).as_posix()
        files[relative] = data
        entries.append({'path': relative, 'sha256': hashlib.sha256(data).hexdigest()})
    return files, entries


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
    # The inspector's Rust icons draw Lucide paths; omitting metadata must fail closed.
    icons = root / 'app' / 'src' / 'components' / 'icons.rs'
    icons_path = icons.relative_to(root).as_posix()
    if icons.is_file() and 'lucide' in icons.read_text().lower() and not any(icons_path in asset['usedBy'] for asset in assets):
        raise ValueError('Lucide icons require their distribution license')
    return files, assets


def collect():
    files = {}
    crates = []
    lockfiles = []
    collected = {}
    for root in workspaces():
        data = metadata(root)
        production = production_ids(data)
        lockfile = root / 'Cargo.lock'
        lockfiles.append({'path': lockfile.relative_to(ROOT).as_posix(),
                          'sha256': hashlib.sha256(lockfile.read_bytes()).hexdigest()})
        for package in sorted(data['packages'], key=lambda item: (item['name'], item['version'])):
            if package['source'] is None or package['id'] not in production:
                continue
            if package['source'] != CRATES_IO:
                raise ValueError('Review non-crates.io packages before generating distribution notices: ' + package['name'])
            key = (package['name'], package['version'])
            license = package.get('license') or ''
            if key in collected:
                if collected[key] != license:
                    raise ValueError('Two source packages disagree on the license of ' + package['name'])
                continue
            collected[key] = license
            source = Path(package['manifest_path']).parent
            target = Path('third-party') / (package['name'] + '@' + package['version'])
            if target.is_absolute() or '..' in target.parts:
                raise ValueError('Invalid crate destination')
            crate_files, entries = license_notices(source, target)
            if not license and not entries:
                raise ValueError('No license expression or notice files found for ' + package['name'])
            files.update(crate_files)
            crates.append({'crate': package['name'], 'version': package['version'],
                           'license': license, 'files': entries})
    asset_files, assets = asset_notices(ROOT)
    files.update(asset_files)
    manifest = {'schema': 2, 'scope': 'app and CLI production Rust crates and attributed assets for darwin arm64; copied notices retain their original terms',
                'lockfiles': lockfiles, 'crates': crates, 'assets': assets}
    files['manifest.json'] = (json.dumps(manifest, indent=2, ensure_ascii=False) + '\n').encode()
    lines = ['# Third-party notices', '',
             'Generated from the pinned app/CLI production `Cargo.lock` graphs for macOS arm64.',
             'Each component retains its original license and copyright. Nested notice files are copied as provided.',
             'This inventory does not relicense components or claim that every copied file is linked into the executable.',
             'Crates that publish no license file are attributed by their declared crates.io license expression.',
             'Downloaded development runtimes/databases are separate; see docs/third-party-software.md in the source repository.', '',
             'Regenerate: `python3 scripts/collect-third-party-notices.py`; verify: add `--check`.',
             'Run `cargo fetch --locked` in `app/` and each crate under `app/crates/` first; generation makes no network request.',
             'The SHA-256 inventory is in [manifest.json](manifest.json). No local cache paths are included.', '',
             '| Crate | Version | License | Copied notices |', '| --- | --- | --- | --- |']
    for crate in crates:
        lines.append('| ' + crate['crate'] + ' | ' + crate['version'] + ' | ' + crate['license'] + ' | ' + str(len(crate['files'])) + ' |')
    if assets:
        lines += ['', '| Asset | Upstream license snapshot | License |', '| --- | --- | --- |']
        for asset in assets:
            lines.append('| ' + asset['name'] + ' | [' + asset['licenseRevision'][:12] + '](' + asset['licenseSource'] + ') | [notice](' + asset['files'][0]['path'] + ') |')
    lines.append('')
    files['README.md'] = '\n'.join(lines).encode()
    return files, crates


def verify_inventory(files, target):
    mismatches = [name for name, content in files.items()
                  if not (target / name).is_file() or (target / name).read_bytes() != content]
    extra = ([path.relative_to(target).as_posix() for path in target.rglob('*')
              if path.is_file() and path.relative_to(target).as_posix() not in files]
             if target.exists() else [])
    if mismatches or extra:
        raise ValueError('Notice inventory differs; regenerate and review: ' + ', '.join(mismatches + extra))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    files, crates = collect()
    target = ROOT / 'licenses'
    if args.check:
        verify_inventory(files, target)
    else:
        if target.exists():
            # Do not erase unrecognized files. A maintainer reviews removed crates explicitly.
            extra = [path.relative_to(target).as_posix() for path in target.rglob('*')
                     if path.is_file() and path.relative_to(target).as_posix() not in files]
            if extra:
                raise ValueError('Review obsolete or extra notice files before regeneration: ' + ', '.join(extra))
        for name, content in files.items():
            destination = target / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(content)
    print(('Verified' if args.check else 'Collected') + f' {len(crates)} crates, {len(files)} distribution notice files')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
