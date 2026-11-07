"""Pure local action resolution and explicit export projection."""
import re

KINDS = {'project-service', 'instance', 'site', 'package', 'ca', 'back'}
ID = re.compile(r'[A-Za-z0-9][A-Za-z0-9._@+-]{0,199}')


def resolve_action(fixture):
    def out(status, key, target=None):
        return dict(status=status, target=target, messageKey=key, mutations=[])
    if not isinstance(fixture, dict) or set(fixture) - {'action', 'currentState', 'dirtyDraft', 'stale', 'cancelled'}:
        return out('unavailable', 'invalid-input')
    if fixture.get('cancelled'):
        return out('unavailable', 'navigation-cancelled')
    action = fixture.get('action')
    state = fixture.get('currentState')
    dirty = fixture.get('dirtyDraft', {})
    if not isinstance(action, dict) or not isinstance(state, dict) or not isinstance(dirty, dict):
        return out('unavailable', 'no-target')
    if set(action) - {'kind', 'id', 'owner', 'service'} or not isinstance(action.get('kind'), str) or action['kind'] not in KINDS:
        return out('unavailable', 'unsupported-action')
    if any(not isinstance(state.get(k, []), list) for k in ['instances', 'packages', 'sites']) or not isinstance(state.get('projects', {}), dict):
        return out('unavailable', 'invalid-input')
    kind = action['kind']
    if kind == 'back':
        return out('navigate', 'keep-workspace', {'kind': 'back', 'preserveDraft': True})
    if fixture.get('stale'):
        return out('unavailable', 'refresh-diagnostics')
    if kind == 'ca':
        return out('navigate', 'open-settings', {'kind': 'ca'}) if state.get('caAvailable') is True else out('unavailable', 'target-missing')
    ident = action.get('id')
    if not isinstance(ident, str) or not ID.fullmatch(ident):
        return out('unavailable', 'invalid-target')
    target = {'kind': kind, 'id': ident}
    if kind == 'project-service':
        service = action.get('service')
        projects = state.get('projects', {})
        project = projects.get(ident) if isinstance(projects, dict) else None
        if not isinstance(project, dict) or not isinstance(project.get('services', []), list) or not isinstance(service, str) or not ID.fullmatch(service) or service not in project.get('services', []):
            return out('unavailable', 'target-missing')
        target['service'] = service
        same = dirty.get('project') == ident
        target['preserveDraft'] = same
        if dirty.get('dirty') is True and not same:
            return out('confirm-discard', 'confirm-draft-discard', target)
    elif kind == 'site':
        owner = action.get('owner', '')
        if not isinstance(owner, str) or (owner and not ID.fullmatch(owner)):
            return out('unavailable', 'invalid-target')
        if {'id': ident, 'owner': owner} not in state.get('sites', []):
            return out('unavailable', 'target-missing')
        target['owner'] = owner
    else:
        field = {'instance': 'instances', 'package': 'packages'}[kind]
        if ident not in state.get(field, []):
            return out('unavailable', 'target-missing')
    return out('navigate', 'open-settings', target)


LAYERS = {'Runtime', 'Readiness', 'Database', 'DNS', 'Trust', 'Proxy TLS', 'Upstream TLS', 'Software'}
STATUSES = {'OK', 'Warning', 'Failed', 'Stopped', 'Disabled'}
MESSAGES = {'missing-runtime': ('Required runtime is unavailable.', 'Open software settings.'),
            'unready-service': ('Service readiness check did not pass.', 'Open project settings.'),
            'tls-failed': ('TLS verification did not pass.', 'Open HTTPS settings.')}


def redact_report(report):
    """Only report-owned enums and fixed text cross the export boundary."""
    checks = report.get('checks', []) if isinstance(report, dict) else []
    aliases = {}
    projected = []
    if not isinstance(checks, list):
        checks = []
    for check in checks:
        if not isinstance(check, dict):
            continue
        resource = check.get('resource')
        key = resource if isinstance(resource, str) else ''
        if key not in aliases:
            aliases[key] = 'resource-' + str(len(aliases) + 1)
        summary, remedy = MESSAGES.get(check.get('code') if isinstance(check.get('code'), str) else None,
                                       ('Review diagnostic result.', 'Run diagnostics again.'))
        layer, status = check.get('layer'), check.get('status')
        projected.append(dict(layer=layer if isinstance(layer, str) and layer in LAYERS else 'Other',
                              resource=aliases[key], status=status if isinstance(status, str) and status in STATUSES else 'Warning',
                              summary=summary, remedy=remedy))
    return dict(schemaVersion=1, redacted=True, checks=projected)
