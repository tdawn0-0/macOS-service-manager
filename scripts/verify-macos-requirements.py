#!/usr/bin/env python3
"""Verify a packaged macOS Devbase.app bundle's architecture, minimum OS,
Info.plist hygiene, icon, and (with --release-bundle) bundled notices and
single-binary CLI dispatch.
"""
import argparse
import json
from pathlib import Path
import plistlib
import re
import subprocess
import sys

FORBIDDEN_PRIVACY_KEYS = (
    'NSMicrophoneUsageDescription',
    'NSSpeechRecognitionUsageDescription',
    'NSCameraUsageDescription',
)


def version(value):
    if not isinstance(value, str) or not re.fullmatch(r'\d+(?:\.\d+){0,2}', value):
        raise ValueError('Invalid macOS version: ' + repr(value))
    parts = tuple(map(int, value.split('.')))
    return parts + (0,) * (3 - len(parts))


def verify(app, release_bundle=False):
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    executable = info['CFBundleExecutable']
    if not executable or Path(executable).name != executable:
        raise ValueError('CFBundleExecutable must name a file inside Contents/MacOS')
    binary = app / 'Contents/MacOS' / executable
    architectures = subprocess.check_output(['lipo', '-archs', str(binary)], text=True, timeout=10).strip()
    if architectures != 'arm64':
        raise ValueError('Only Apple Silicon (arm64) is supported: ' + architectures)
    output = subprocess.check_output(['otool', '-l', str(binary)], text=True, timeout=10)
    requirements = re.findall(r'cmd LC_BUILD_VERSION\s+cmdsize \d+\s+platform (\d+)\s+minos ([\d.]+)', output)
    if len(requirements) != 1 or requirements[0][0] != '1':
        raise ValueError('Expected one macOS LC_BUILD_VERSION in the arm64 executable')
    binary_minimum = requirements[0][1]
    declared = info['LSMinimumSystemVersion']
    if version(declared) < version(binary_minimum):
        raise ValueError(
            f'Bundle declares macOS {declared}, but the executable requires {binary_minimum}; '
            'correct minimumSystemVersion before distribution'
        )

    icon_name = info.get('CFBundleIconFile')
    if not isinstance(icon_name, str) or not icon_name:
        raise ValueError('CFBundleIconFile is missing from Info.plist')
    icon_file = icon_name if icon_name.endswith('.icns') else f'{icon_name}.icns'
    icon_path = app / 'Contents/Resources' / icon_file
    if not icon_path.is_file() or icon_path.stat().st_size == 0:
        raise ValueError(f'App icon is missing or empty: {icon_path}')

    forbidden_present = [key for key in FORBIDDEN_PRIVACY_KEYS if key in info]
    if forbidden_present:
        raise ValueError('Unexpected privacy usage descriptions in Info.plist: ' + ', '.join(forbidden_present))

    if release_bundle:
        for rel in ('Contents/Resources/LICENSE', 'Contents/Resources/NOTICE', 'Contents/Resources/licenses/manifest.json'):
            path = app / rel
            if not path.is_file() or path.stat().st_size == 0:
                raise ValueError(f'Required bundled license notice is missing or empty: {path}')

        ver = subprocess.run(
            [str(binary), 'version', '--json'],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        if ver.returncode != 0:
            raise ValueError(f'Packaged CLI `version --json` failed ({ver.returncode}): {ver.stderr.strip()}')
        payload = json.loads(ver.stdout)
        if not isinstance(payload, dict) or not payload.get('version'):
            raise ValueError(f'Unexpected `version --json` output: {ver.stdout.strip()!r}')

        dev = subprocess.run(
            [str(binary), '--dev'],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        if dev.returncode == 0 or not dev.stderr.strip():
            raise ValueError(
                f'Packaged internal `--dev` dispatch did not fail with usage error '
                f'(exit {dev.returncode}, stderr={dev.stderr.strip()!r})'
            )

    summary = f'Verified arm64 requirements: bundle macOS {declared}, executable macOS {binary_minimum}, icon {icon_file}'
    if release_bundle:
        summary += ', bundled notices & CLI/--dev dispatch OK'
    print(summary)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', type=Path, help='Path to the .app bundle')
    parser.add_argument(
        '--release-bundle',
        action='store_true',
        help='Also verify bundled LICENSE/NOTICE/licenses and single-binary CLI/--dev dispatch',
    )
    args = parser.parse_args()
    try:
        verify(args.app, release_bundle=args.release_bundle)
    except (OSError, ValueError, KeyError, json.JSONDecodeError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
