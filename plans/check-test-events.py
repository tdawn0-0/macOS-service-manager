"""Read-only validation of planned Go test gates; no application code is executed."""
import argparse
from datetime import datetime
import json
from pathlib import Path
import struct
import sys


def check(lines, package, required, shot_dir=None):
    required = set(required)
    if not required:
        raise ValueError('at least one required test is necessary')
    ran, passed = set(), set()
    complete = False
    started = None
    for line in lines:
        event = json.loads(line)
        if not isinstance(event, dict):
            raise ValueError('invalid event')
        if event.get('Package') != package:
            continue
        if complete:
            raise ValueError('event after package completion')
        action, name = event.get('Action'), event.get('Test')
        if action in ('fail', 'skip'):
            raise ValueError('a test failed or skipped')
        if action == 'start' and name is None:
            if started is not None:
                raise ValueError('duplicate package start')
            started = datetime.fromisoformat(event['Time'].replace('Z', '+00:00')).timestamp()
        if action == 'run' and name:
            if name in ran:
                raise ValueError('duplicate test run')
            ran.add(name)
        if action == 'pass':
            if name is None:
                complete = True
            else:
                if name in passed or name not in ran:
                    raise ValueError('pass without unique run')
                passed.add(name)
    if started is None or not complete or not required <= passed:
        raise ValueError('missing package completion or required test PASS')
    if shot_dir is not None:
        for theme in ('light', 'dark'):
            for width, height in ((900, 520), (1120, 720)):
                path = Path(shot_dir) / f'{theme}-{width}x{height}.png'
                if path.is_symlink() or not path.is_file():
                    raise ValueError('missing screenshot')
                with path.open('rb') as file:
                    head = file.read(24)
                if (head[:8] != b'\x89PNG\r\n\x1a\n' or head[12:16] != b'IHDR'
                        or len(head) != 24 or struct.unpack('>II', head[16:24]) != (width, height)):
                    raise ValueError('wrong screenshot format or dimensions')
                if path.stat().st_mtime < started:
                    raise ValueError('stale screenshot')
    return len(required)


def self_test():
    package = 'example'
    events = [
        {'Action': 'start', 'Package': package, 'Time': '2026-11-08T00:00:00Z'},
        {'Action': 'run', 'Package': package, 'Test': 'TestA'},
        {'Action': 'run', 'Package': package, 'Test': 'TestA/child'},
        {'Action': 'pass', 'Package': package, 'Test': 'TestA/child'},
        {'Action': 'pass', 'Package': package, 'Test': 'TestA'},
        {'Action': 'pass', 'Package': package},
    ]
    encode = lambda rows: [json.dumps(row) for row in rows]
    assert check(encode(events), package, ['TestA', 'TestA/child']) == 2
    invalid = [
        [], events[:-1],
        [row for row in events if row.get('Test') != 'TestA/child'],
        [dict(row, Action='skip') if row.get('Test') == 'TestA/child' and row['Action'] == 'pass' else row for row in events],
        [dict(row, Action='fail') if row.get('Test') == 'TestA' and row['Action'] == 'pass' else row for row in events],
        [row for row in events if row['Action'] != 'run'],
        events + [events[-1]], events[:-1] + [events[3], events[-1]],
        [dict(row, Package='other') for row in events],
    ]
    for rows in invalid:
        try:
            check(encode(rows), package, ['TestA/child'])
        except (ValueError, KeyError, TypeError):
            continue
        raise AssertionError('invalid stream accepted')
    print('PASS: complete stream and 9 negative event cases')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--package')
    parser.add_argument('--require', action='append', default=[])
    parser.add_argument('--shot-dir', type=Path)
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if not args.package or not args.require:
        parser.error('--package and --require are required')
    try:
        count = check(sys.stdin, args.package, args.require, args.shot_dir)
    except (ValueError, KeyError, TypeError, OSError):
        parser.exit(1, 'FAIL: incomplete/failed test events or invalid/stale screenshots\n')
    print(f'PASS: {count} required test results verified')


if __name__ == '__main__':
    main()
