#!/usr/bin/env python3
"""Reject a bundle whose declared minimum macOS contradicts its executable."""
from pathlib import Path
import plistlib
import re
import subprocess
import sys


def version(value):
    if not isinstance(value, str) or not re.fullmatch(r'\d+(?:\.\d+){0,2}', value):
        raise ValueError('Invalid macOS version: ' + repr(value))
    parts = tuple(map(int, value.split('.')))
    return parts + (0,) * (3 - len(parts))


def verify(app):
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
        raise ValueError(f'Bundle declares macOS {declared}, but the executable requires {binary_minimum}; correct minimumSystemVersion before distribution')
    print(f'Verified arm64 requirements: bundle macOS {declared}, executable macOS {binary_minimum}')


if __name__ == '__main__':
    try:
        if len(sys.argv) != 2:
            raise ValueError('Usage: verify-macos-requirements.py <signed.app>')
        verify(Path(sys.argv[1]))
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
