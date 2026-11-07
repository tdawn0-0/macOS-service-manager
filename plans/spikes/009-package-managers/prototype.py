"""Offline decision model. No command is executed and no project is read."""
import re

LOCKS = {'package-lock.json': 'npm', 'npm-shrinkwrap.json': 'npm',
         'pnpm-lock.yaml': 'pnpm', 'yarn.lock': 'yarn'}
ALLOWED = {'package', 'locks', 'nodeAvailable', 'tools', 'offline',
           'workspace', 'workspaceReviewed', 'busy', 'dirty'}


def resolve_request(fixture):
    def result(status, reason, manager=None, version=None, argv=None):
        return dict(status=status, manager=manager, version=version, argv=argv or [],
                    cwdPolicy='reviewed-workspace-root' if fixture.get('workspace') else 'project-root',
                    reason=reason, requiresConfirmation=True)
    if not isinstance(fixture, dict):
        return dict(status='blocked', manager=None, version=None, argv=[],
                    cwdPolicy='project-root', reason='invalid-input', requiresConfirmation=True)
    if set(fixture) - ALLOWED:
        return result('blocked', 'unknown-input')
    if fixture.get('busy') or fixture.get('dirty'):
        return result('blocked', 'save-and-finish-tasks')
    package = fixture.get('package')
    locks = fixture.get('locks', [])
    tools = fixture.get('tools', [])
    if not isinstance(package, dict) or not isinstance(locks, list) or not all(isinstance(x, str) for x in locks) or not isinstance(tools, list):
        return result('blocked', 'invalid-manifest')
    if fixture.get('nodeAvailable') is not True:
        return result('blocked', 'missing-node')
    if fixture.get('workspace') and fixture.get('workspaceReviewed') is not True:
        return result('needs-choice', 'review-workspace-root')
    if any(x not in LOCKS for x in locks):
        return result('blocked', 'unsupported-lockfile')
    families = {LOCKS[x] for x in locks}
    if len(families) > 1 or len(locks) != len(set(locks)):
        return result('needs-choice', 'conflicting-lockfiles')
    declaration = package.get('packageManager')
    manager = version = None
    if declaration is not None:
        if not isinstance(declaration, str):
            return result('blocked', 'invalid-manager-declaration')
        match = re.fullmatch(r'(npm|pnpm|yarn)@(\d+\.\d+\.\d+)', declaration)
        if not match:
            return result('blocked', 'unsupported-manager-declaration')
        manager, version = match.groups()
        if families and families != {manager}:
            return result('needs-choice', 'declaration-lock-conflict')
    elif len(families) == 1:
        manager = next(iter(families))
    else:
        return result('needs-choice', 'choose-manager')
    if not families:
        return result('blocked', 'missing-lockfile', manager, version)
    candidates = [x for x in tools if isinstance(x, dict) and x.get('manager') == manager
                  and isinstance(x.get('version'), str) and re.fullmatch(r'\d+\.\d+\.\d+', x['version'])
                  and x.get('verified') is True and x.get('source') in {'selected-node-bundle', 'managed-exact'}
                  and (version is None or x['version'] == version)]
    if len(candidates) != 1:
        return result('blocked', 'exact-tool-unavailable', manager, version)
    version = candidates[0]['version']
    if manager == 'npm' and candidates[0]['source'] != 'selected-node-bundle':
        return result('blocked', 'npm-runtime-mismatch', manager, version)
    major = int(version.split('.')[0])
    if manager == 'npm':
        argv = ['npm', 'ci']
    elif manager == 'pnpm':
        argv = ['pnpm', 'install', '--frozen-lockfile']
    elif major == 1:
        argv = ['yarn', 'install', '--frozen-lockfile']
    elif major in {2, 3, 4}:
        argv = ['yarn', 'install', '--immutable']
    else:
        return result('blocked', 'unsupported-manager-major', manager, version)
    if fixture.get('offline'):
        if manager != 'pnpm':
            return result('blocked', 'offline-policy-unverified', manager, version)
        argv.append('--offline')
    return result('ready', 'review-command', manager, version, argv)
