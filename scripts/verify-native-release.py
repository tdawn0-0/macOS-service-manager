#!/usr/bin/env python3
"""Require real native software tests before signing. --run needs a disposable hosted Mac."""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import signal
import subprocess
import tempfile
import time
from urllib.parse import urlsplit

PACKAGE = 'devbase'
CORE = ('TestUIStandaloneRuntimeAndHTTPS',
        'TestUIDatabaseInitializationKeepsIndependentDrafts')
CATALOG = 'TestUISoftwareCatalogAllFive'
MATRIX = (CATALOG,) + tuple(CATALOG + '/' + x for x in ('node', 'mysql', 'postgresql', 'redis', 'mongodb'))
NATIVE = ('TestNativeResourceProduct',) + tuple('TestNativeResourceProduct/' + x for x in
          ('node-terminal', 'external-upstream-https', 'resource-quit', 'connection-profiles'))
REQUIRED = CORE + MATRIX + NATIVE
SANDBOX = ('sandbox-exec', '-p', '(version 1)(allow default)(deny file-write* (subpath "/opt/homebrew"))')

HOMEBREW_FORMULAE = {'postgresql': 'postgresql@17', 'redis': 'redis@8.2'}
VERSIONS = (('node', '22.14.0'), ('node', '20.19.0'), ('mysql', '8.4.13'),
            ('redis', '8.2'), ('mongodb', '8.0.38'), ('postgresql', '17'))


def check_events(lines, required=REQUIRED):
    """Accept only completed, passing root-package streams; output is never copied."""
    terminals = {}
    runs = set()
    package_pass = False
    package_start = False
    for number, line in enumerate(lines, 1):
        try:
            event = json.loads(line)
        except (ValueError, TypeError) as error:
            raise ValueError('invalid test event at line ' + str(number)) from error
        if not isinstance(event, dict) or event.get('Package') != PACKAGE:
            raise ValueError('unexpected event package')
        action = event.get('Action')
        if not isinstance(action, str) or action not in {'start', 'run', 'pause', 'cont', 'output', 'pass', 'bench', 'skip', 'fail'}:
            raise ValueError('unexpected event action')
        if package_pass:
            raise ValueError('events after package completion')
        test = event.get('Test')
        if test is not None and (not isinstance(test, str) or not test.startswith('Test')):
            raise ValueError('invalid test identity')
        if action == 'start' and test is None:
            if package_start:
                raise ValueError('duplicate package start')
            package_start = True
        if action in {'skip', 'fail'}:
            raise ValueError('native test skipped or failed: ' + (test or 'package'))
        if action == 'run' and test is not None:
            if test in runs:
                raise ValueError('duplicate test run: ' + test)
            runs.add(test)
        if action == 'pass':
            if test is None:
                package_pass = True
            else:
                if test in terminals:
                    raise ValueError('duplicate test result: ' + test)
                if test not in runs:
                    raise ValueError('test passed without running: ' + test)
                terminals[test] = 'pass'
    if not package_start or not package_pass:
        raise ValueError('incomplete package result')
    missing = set(required) - terminals.keys()
    if missing:
        raise ValueError('missing required tests: ' + ', '.join(sorted(missing)))
    return [{'test': name, 'result': 'pass'} for name in required]


def checked(argv, *, env=None, timeout=900):
    result = subprocess.run(argv, env=env, text=True, capture_output=True, timeout=timeout)
    if result.returncode:
        # Tool output may contain local configuration; never include it in a public error.
        raise RuntimeError('native preparation command failed: ' + Path(argv[0]).name)
    return result.stdout


def preflight(local_existing=False):
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        raise RuntimeError('native gate requires Darwin arm64')
    if os.getuid() == 0:
        raise RuntimeError('native gate must run as a normal user')
    if not local_existing and (os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted'):
        raise RuntimeError('native gate requires a disposable GitHub-hosted runner; no software was installed')
    for tool in ('go', 'brew', 'launchctl') + (('sandbox-exec',) if local_existing else ()):
        if not shutil.which(tool):
            raise RuntimeError('missing required tool: ' + tool)
    if shutil.disk_usage(Path.cwd()).free < 30 * (1 << 30):
        raise RuntimeError('native gate requires 30 GiB free disk space')
    checked(['launchctl', 'print', 'gui/' + str(os.getuid())], timeout=20)


def package_summary(packages):
    rows = []
    for value in packages.values():
        name, version = value.get('name', ''), value.get('version', '')
        digest, arch = value.get('sha256', ''), value.get('arch', '')
        if name not in {x[0] for x in VERSIONS} or not re.fullmatch(r'[A-Za-z0-9._+-]+', version):
            raise ValueError('unexpected package identity')
        if not re.fullmatch(r'[0-9a-f]{64}', digest) or arch not in {'arm64', 'universal'}:
            raise ValueError('invalid package verification metadata')
        url = urlsplit(value.get('url', ''))
        if url.scheme != 'https' or not url.hostname or url.username or url.password:
            raise ValueError('invalid package source')
        # Source domain identifies provenance without leaking URL query/path values.
        formula = value.get('formula', '')
        if name in HOMEBREW_FORMULAE and formula != HOMEBREW_FORMULAE[name]:
            raise ValueError('unexpected Homebrew formula')
        rows.append({'name': name, 'version': version, 'arch': arch, 'sha256': digest, 'source': url.hostname})
    if Counter(row['name'] for row in rows) != Counter(name for name, _ in VERSIONS):
        raise ValueError('missing prepared packages')
    return sorted(rows, key=lambda row: (row['name'], row['version']))


def test_phase(root, env, names, sandbox=()):
    events = root / ('core.jsonl' if names == CORE else 'matrix.jsonl' if names == MATRIX else 'native.jsonl')
    roots = sorted({name.split('/')[0] for name in names})
    argv = list(sandbox) + ['go', 'test', '-json', '-race', '-count=1', '-timeout=25m', '-run', '^(' + '|'.join(roots) + ')$', '.']
    with events.open('w') as output, (root / 'test-stderr.txt').open('w') as errors:
        process = subprocess.Popen(argv, env=env, stdout=output, stderr=errors, start_new_session=True)
        try:
            code = process.wait(timeout=1800)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise RuntimeError('native tests timed out; recycle disposable runner')
    if code:
        raise RuntimeError('native tests failed; recycle disposable runner')
    with events.open() as stream:
        return check_events(stream, names)


def parse_formula_metadata(raw, name):
    try:
        value = json.loads(raw)
        formulae = value['formulae']
        if not isinstance(formulae, list) or len(formulae) != 1:
            raise ValueError('formula count')
        formula = formulae[0]
        full_name = formula['full_name']
        rack = formula['name']
        stable = formula['versions']['stable']
        revision = formula.get('revision', 0)
        racks = set(HOMEBREW_FORMULAE.values())
        if (not isinstance(full_name, str) or not isinstance(rack, str) or rack not in racks
                or not isinstance(stable, str) or not re.fullmatch(r'[0-9]+\.[0-9]+(?:\.[0-9]+)?', stable)
                or type(revision) is not int or revision < 0):
            raise ValueError('formula fields')
        return formula
    except (ValueError, KeyError, TypeError, AttributeError):
        raise RuntimeError('invalid Homebrew formula metadata: ' + name) from None


def validate_existing_kegs():
    env = dict(os.environ, HOMEBREW_NO_AUTO_UPDATE='1')
    for name, requested, tap, binaries in [('postgresql', 'postgresql@17', 'homebrew/core', ['postgres','initdb','pg_dump','pg_restore','psql','createdb']),
                                           ('redis', 'redis@8.2', 'homebrew/core', ['redis-server','redis-cli'])]:
        raw = checked(list(SANDBOX) + ['/opt/homebrew/bin/brew','info','--json=v2','--formula',requested], env=env)
        formula = parse_formula_metadata(raw, name)
        if formula['full_name'] != requested:
            raise RuntimeError('existing formula source differs: ' + name)
        version = formula['versions']['stable']
        if formula.get('revision',0): version += '_' + str(formula['revision'])
        keg = Path('/opt/homebrew/Cellar') / formula['name'] / version
        receipt = keg / 'INSTALL_RECEIPT.json'
        if receipt.is_symlink() or not receipt.is_file():
            raise RuntimeError('exact existing keg is missing: ' + name + ' ' + version)
        try:
            data = json.loads(receipt.read_text())
            source = data['source']
            receipt_matches = source['tap'] == tap and source['versions']['stable'] == formula['versions']['stable']
        except (ValueError, KeyError, TypeError):
            raise RuntimeError('invalid existing keg receipt metadata: ' + name) from None
        if not receipt_matches:
            raise RuntimeError('existing keg receipt differs: ' + name)
        for binary in binaries:
            relative = 'bin/' + binary
            path = keg / relative
            if not path.is_file() or not os.access(path,os.X_OK) or not path.resolve().is_relative_to(keg.resolve()):
                raise RuntimeError('existing keg executable missing: ' + name)


def cleanup_local_jobs(root):
    # TMPDIR confines every native-test t.TempDir to this root, including failure artifacts.
    for file in (root / 'tests').rglob('*.plist'):
        if file.parent.name != 'jobs': continue
        data = plistlib.loads(file.read_bytes())
        label = data.get('Label','')
        if not re.fullmatch(r'xyz\.jyeu\.devbase\.dev\.[a-f0-9]{8}\.[A-Za-z0-9._-]+',label) or file.stem != label:
            raise RuntimeError('local cleanup cannot confirm job ownership; private root retained')
        target='gui/'+str(os.getuid())+'/'+label
        subprocess.run(['launchctl','bootout',target],capture_output=True,timeout=40)
        result=subprocess.run(['launchctl','print',target],capture_output=True,timeout=20)
        answer=result.stdout+result.stderr
        if result.returncode==0 or not (b'Could not find service' in answer or b'Could not find specified service' in answer):
            raise RuntimeError('local cleanup cannot confirm job removal; private root retained')


def run_native(local_existing=False):
    destination = Path('build/native-qa/results.json')
    destination.unlink(missing_ok=True)
    preflight(local_existing)  # Before directory creation, Homebrew or builds.
    started = time.monotonic()
    if local_existing:
        validate_existing_kegs()
    root = Path(tempfile.mkdtemp(prefix='msm-native-release-')).resolve()
    root.chmod(0o700)
    env = dict(os.environ, MSM_DEV_HOME=str(root), HOMEBREW_NO_AUTO_UPDATE='1')
    sandbox = SANDBOX if local_existing else ()
    if local_existing:
        (root / 'tests').mkdir()
        env['TMPDIR'] = str(root / 'tests')
    def run(argv, **kwargs):
        return checked(list(sandbox) + argv, **kwargs)
    harness = root / 'devmgr'
    built = False
    try:
        run(['go', 'build', '-o', str(harness), './cmd/devmgr'])
        built = True
        run([str(harness), 'log-rotation', 'off'], env=env)
        for name, version in VERSIONS:
            run([str(harness), 'install', name, version], env=env, timeout=1200)
        state = json.loads(run([str(harness), 'status'], env=env))
        packages = package_summary(state['packages'])
        env.update(MSM_UI_E2E_PACKAGE_CACHE=str(root / 'downloads'), MSM_UI_E2E_HOMEBREW='1')
        if local_existing:
            validate_existing_kegs()
        # launchd and Redis require normal native process semantics. Sandbox only
        # preparation; revalidate the read-only keg contract before each phase.
        results = test_phase(root, env, CORE)
        if local_existing:
            validate_existing_kegs()
        results += test_phase(root, env, MATRIX)
        if local_existing:
            validate_existing_kegs()
        results += test_phase(root, env, NATIVE)
        summary = {'schemaVersion': 1, 'macOS': checked(['sw_vers', '-productVersion']).strip(),
                   'arch': 'arm64', 'go': checked(['go', 'version']).strip(), 'packages': packages,
                   'tests': results, 'elapsedSeconds': round(time.monotonic() - started, 2)}
    finally:
        # The runner itself is disposable. Never scan or stop jobs outside this root.
        if built:
            run([str(harness), 'log-rotation', 'off'], env=env, timeout=40)
        if local_existing:
            cleanup_local_jobs(root)
        shutil.rmtree(root)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(summary, indent=2) + '\n')
    return summary


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--check-events', type=Path)
    mode.add_argument('--run', action='store_true')
    parser.add_argument('--local-existing-kegs', action='store_true', help='Explicit local sandbox mode: require exact existing kegs; never prepare Homebrew')
    args = parser.parse_args(argv)
    if args.local_existing_kegs and not args.run:
        parser.error('--local-existing-kegs requires --run')
    try:
        if args.check_events:
            with args.check_events.open() as stream:
                result = check_events(stream)
        else:
            result = run_native(args.local_existing_kegs)
        print(json.dumps(result, indent=2))
    except (ValueError, RuntimeError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, json.dumps({'schemaVersion': 1, 'status': 'blocked', 'error': str(error)}) + '\n')


if __name__ == '__main__':
    main()
