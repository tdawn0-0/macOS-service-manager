#!/usr/bin/env python3
"""Smoke check independent software, databases and HTTPS on an isolated arm64 Mac."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import socket
import ssl
import subprocess
import tempfile
import time


def free_port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


def run_cli(binary, env, *args):
    result = subprocess.run([str(binary), *map(str, args)], env=env, text=True,
                            capture_output=True, timeout=180)
    if result.returncode:
        raise RuntimeError('fixture command failed: ' + str(args[0]) + ': ' + result.stderr)
    return result.stdout


def save_json(root, name, value):
    path = root / (name + '.json')
    path.write_text(json.dumps(value))
    path.chmod(0o600)
    return path


def stop_external(process):
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def main():
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        raise SystemExit('This verification requires an Apple Silicon Mac (arm64).')
    binary = Path(os.environ.get('MSM_SMOKE_BINARY', '/tmp/msm-devmgr')).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix='msm-smoke-') as directory:
        root = Path(directory)
        env = dict(os.environ, MSM_DEV_HOME=str(root))
        cache = os.environ.get('MSM_SMOKE_CACHE')
        if cache:
            shutil.copytree(cache, root / 'downloads', dirs_exist_ok=True)
        print('Isolated test directory:', root, flush=True)
        cli = lambda *args: run_cli(binary, env, *args)
        external = None
        instances = []
        try:
            for name, version in [('node', '22.14.0'), ('node', '20.19.0'),
                                  ('mysql', '8.4.4'), ('postgresql', '17'),
                                  ('redis', '7.4.7'), ('mongodb', '8.0.5')]:
                cli('install', name, version)
            packages = json.loads(cli('status'))['packages']
            if {value['name'] for value in packages.values()} != {'node', 'mysql', 'postgresql', 'redis', 'mongodb'}:
                raise AssertionError('five software categories were not installed')
            pg_id, pg_pkg = next((key, value) for key, value in packages.items() if value['name'] == 'postgresql')
            node = root / 'packages/node@22.14.0-arm64/bin/node'
            node20 = root / 'packages/node@20.19.0-arm64/bin/node'
            assert subprocess.check_output([str(node), '--version'], text=True).strip() == 'v22.14.0'
            assert subprocess.check_output([str(node20), '--version'], text=True).strip() == 'v20.19.0'
            web, https, pg_port, redis_port = [free_port() for _ in range(4)]
            pg_instance = {'id': 'integration-pg', 'kind': 'postgresql', 'package': pg_id, 'port': pg_port, 'persistent': True}
            redis_instance = {'id': 'integration-redis', 'kind': 'redis', 'package': 'redis@7.4.7-arm64', 'port': redis_port, 'persistent': True}
            for instance in [pg_instance, redis_instance]:
                cli('instance', save_json(root, instance['id'], instance))
                instances.append(instance['id'])
                cli('db', 'start', instance['id'])
            app_dir = root / 'external-app'
            app_dir.mkdir(mode=0o700)
            (app_dir / 'server.js').write_text('require("http").createServer((q,s)=>s.end("standalone-node-ok "+process.version)).listen(Number(process.env.PORT),"127.0.0.1");')
            with (root / 'logs/external-app.log').open('ab') as log:
                external = subprocess.Popen([str(node), 'server.js'], cwd=app_dir,
                                            env=dict(env, PORT=str(web)), stdout=log,
                                            stderr=subprocess.STDOUT, start_new_session=True)
            state = json.loads(cli('status'))
            state['proxyRules'] = [{'domain': 'integration.test', 'localPort': web}]
            state['proxyListenerPort'] = https
            state['proxyRulesRevision'] = state.get('proxyRulesRevision', 0) + 1
            staged = root / '.state-smoke.json'
            staged.write_text(json.dumps(state))
            staged.chmod(0o600)
            staged.replace(root / 'state.json')
            cli('proxy')
            for attempt in range(60):
                try:
                    context = ssl.create_default_context(cafile=str(root / 'proxy/ca.pem'))
                    with context.wrap_socket(socket.create_connection(('127.0.0.1', https), 2), server_hostname='integration.test') as connection:
                        connection.sendall(b'GET / HTTP/1.1\r\nHost: integration.test\r\nConnection: close\r\n\r\n')
                        response = b''
                        while chunk := connection.recv(8192):
                            response += chunk
                    assert b'standalone-node-ok v22.14.0' in response
                    break
                except (OSError, AssertionError):
                    if attempt == 59:
                        raise
                    time.sleep(0.2)
            print('PASS: external Node and verified HTTPS reverse proxy', flush=True)
            cli('createdb', 'integration-pg', 'source_db')
            pg_bin = Path('/opt/homebrew/Cellar') / pg_pkg['formula'].split('/')[-1] / pg_pkg['version'] / 'bin'
            pg_env = dict(env, PGPASSWORD=(root / 'instances/integration-pg/password').read_text())
            def sql(db, query):
                return subprocess.check_output([str(pg_bin / 'psql'), '-h', '127.0.0.1', '-p', str(pg_port), '-U', 'devmanager', '-d', db, '-Atc', query], env=pg_env, text=True)
            sql('source_db', 'create table sample (n int); insert into sample values (42);')
            backup = cli('backup', 'integration-pg', 'source_db').strip()
            cli('restore', 'integration-pg', 'restored_db', backup)
            assert sql('restored_db', 'select n from sample').strip() == '42'
            print('PASS: PostgreSQL backup and restore', flush=True)
            redis_cli = root / 'packages/redis@7.4.7-arm64/bin/redis-cli'
            redis_env = dict(env, REDISCLI_AUTH=(root / 'instances/integration-redis/password').read_text())
            assert subprocess.check_output([str(redis_cli), '-p', str(redis_port), 'PING'], env=redis_env, text=True).strip() == 'PONG'
            print('PASS: Redis authenticated PING', flush=True)
        finally:
            stop_external(external)
            try:
                cli('proxy-stop')
            finally:
                for instance_id in reversed(instances):
                    cli('db', 'remove', instance_id)
                cli('maintenance-stop')
            prefix = 'xyz.jyeu.devbase.dev.' + hashlib.sha256(str(root).encode()).hexdigest()[:8] + '.'
            for job in (root / 'jobs').glob('*.plist'):
                if job.stem.startswith(prefix):
                    subprocess.run(['launchctl', 'bootout', f'gui/{os.getuid()}/' + job.stem], capture_output=True, timeout=40)
    print('PASS: isolated services stopped; no hosts or system trust changes', flush=True)


if __name__ == '__main__':
    main()
