#!/usr/bin/env python3
"""Prepare a private arm64 App copy and real Node/Redis fixture for desktop QA.
Launch the returned .app through Launch Services. This script never opens UI,
changes hosts or adds/removes keychain trust. --reuse-ca only copies a local CA
into the private disposable fixture; certificate authorization stays in the App.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import socket
import subprocess
import tempfile
import uuid
import time

def validate_harness(value):
    path = value.resolve(strict=True)
    if not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError('--devmgr must be an executable regular file')
    return path


def run_fixture_command(devmgr, root, *values):
    result = subprocess.run([str(devmgr), *map(str, values)],
                            env=dict(os.environ, MSM_DEV_HOME=str(root)),
                            text=True, capture_output=True, timeout=180)
    if result.returncode:
        raise RuntimeError('Fixture command failed: ' + str(values[0]) + '\n' + result.stderr)
    return result.stdout


def cleanup_fixture(base, root, devmgr):
    # Only this freshly allocated root can own the jobs being cleaned up.
    try:
        run_fixture_command(devmgr, root, 'maintenance-stop')
        prefix = 'xyz.jyeu.msm.dev.' + hashlib.sha256(str(root).encode()).hexdigest()[:8] + '.'
        for job in (root / 'jobs').glob('*.plist'):
            if not job.stem.startswith(prefix):
                raise RuntimeError('Unexpected fixture job identity')
            target = f'gui/{os.getuid()}/' + job.stem
            subprocess.run(['launchctl', 'bootout', target], capture_output=True, timeout=40)
            result = subprocess.run(['launchctl', 'print', target], capture_output=True, timeout=40)
            if result.returncode == 0:
                raise RuntimeError('Fixture job remains registered')
            answer = result.stdout + result.stderr
            if b'Could not find service' not in answer and b'Could not find specified service' not in answer:
                raise RuntimeError('Cannot confirm fixture job removal')
        shutil.rmtree(base)
    except Exception as error:
        raise RuntimeError('Fixture cleanup incomplete; retained private directory: ' + str(base)) from error



def stop_external_app(process):
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def start_external_app(binary, directory, environment, port, log_path):
    with log_path.open('ab') as log:
        process = subprocess.Popen([str(binary), 'server.js'], cwd=directory,
                                   env=environment, stdout=log, stderr=subprocess.STDOUT,
                                   start_new_session=True)
    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError('External Node fixture exited before readiness')
            try:
                with socket.create_connection(('127.0.0.1', port), timeout=0.2):
                    return process
            except OSError:
                time.sleep(0.1)
        raise RuntimeError('External Node fixture did not become ready')
    except BaseException:
        stop_external_app(process)
        raise

def _main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--devmgr', type=Path, required=True, help='Test harness built from cmd/devmgr')
    parser.add_argument('--app', type=Path, default=Path('build/darwin-arm64/MacOS Service Manager.app'))
    parser.add_argument('--node-root', type=Path, default=Path('/tmp/msm-development-verification'))
    parser.add_argument('--redis-root', type=Path, default=Path('/tmp/msm-six-verification'))
    parser.add_argument('--reuse-ca', type=Path, required=True, help='Existing development root; never changes its material or trust')
    args = parser.parse_args(argv)
    try:
        devmgr = validate_harness(args.devmgr)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    os.umask(0o077)
    source = args.app.resolve(strict=True)
    base = Path(tempfile.mkdtemp(prefix='msm-native-ui-', dir='/tmp')).resolve()
    external_process = None
    try:
        root = base / 'development'
        root.mkdir(mode=0o700)
        app = base / 'Service Manager Native Verification.app'
        shutil.copytree(source, app)
        info_path = app / 'Contents/Info.plist'
        info = plistlib.loads(info_path.read_bytes())
        info['CFBundleIdentifier'] = 'xyz.jyeu.msm.nativeverification.' + uuid.uuid4().hex
        info['CFBundleName'] = 'Service Manager Native Verification'
        info['CFBundleDisplayName'] = info['CFBundleName']
        info['LSEnvironment'] = {'MSM_DEV_HOME': str(root)}
        info_path.write_bytes(plistlib.dumps(info))
        subprocess.run(['codesign', '--force', '--deep', '--sign', '-', str(app)], check=True, capture_output=True)
        subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
        exe = app / 'Contents/MacOS' / info['CFBundleExecutable']
        def cli(*values):
            return run_fixture_command(devmgr, root, *values)

        def configuration(name, value):
            path = base / (name + '.json')
            path.write_text(json.dumps(value))
            return path

        ports = set()
        def free_port():
            while True:
                with socket.socket() as sock:
                    sock.bind(('127.0.0.1', 0))
                    port = sock.getsockname()[1]
                if port not in ports:
                    ports.add(port)
                    return port

        cli('log-rotation', 'off')
        packages = {}
        manifests = {}
        for name, version, cache in [('node', '22.14.0', args.node_root), ('redis', '7.4.7', args.redis_root)]:
            identity = name + '@' + version + '-arm64'
            manifest = json.loads((cache / 'packages' / identity / '.package.json').read_text())
            archive = cache / 'downloads' / manifest['sha256']
            assert hashlib.sha256(archive.read_bytes()).hexdigest() == manifest['sha256']
            shutil.copyfile(archive, root / 'downloads' / manifest['sha256'])
            cli('install-manifest', configuration(name + '-package', manifest))
            packages[name] = identity
            manifests[name] = manifest
        proxy = root / 'proxy'
        for name in ['ca.pem', 'ca-key.pem']:
            origin = args.reuse_ca.resolve(strict=True) / 'proxy' / name
            if origin.is_symlink() or not origin.is_file():
                raise ValueError('Expected a plain existing CA file')
            shutil.copyfile(origin, proxy / name)
            (proxy / name).chmod(0o600)
        redis_port, web_port, https_port = free_port(), free_port(), free_port()
        instance = dict(id='native-cache', kind='redis', package=packages['redis'], port=redis_port, persistent=True)
        cli('instance', configuration('instance', instance))
        app_dir = base / 'app'
        app_dir.mkdir(mode=0o700)
        (app_dir / 'server.js').write_text(r'''const http = require('node:http');
        const net = require('node:net');
        const fs = require('node:fs');
        const password = fs.readFileSync(process.env.REDIS_PASSWORD_FILE, 'utf8');
        function encode(args) { return '*' + args.length + '\r\n' + args.map(v => '$' + Buffer.byteLength(v) + '\r\n' + v + '\r\n').join(''); }
        function command(args) { return new Promise((resolve, reject) => {
         const socket = net.createConnection({host:'127.0.0.1',port:Number(process.env.REDIS_PORT)}); let data = '', auth = false;
         socket.setTimeout(2000, () => socket.destroy(new Error('Database timed out')));
         socket.on('error', reject); socket.on('connect', () => socket.write(encode(['AUTH',password]) + encode(args)));
         socket.on('data', chunk => {data += chunk.toString(); if(!auth) {const i=data.indexOf('\r\n');if(i<0)return;if(data.slice(0,i)!=='+OK'){socket.destroy();reject(new Error('Database auth failed'));return;}data=data.slice(i+2);auth=true;}
         const end=data.indexOf('\r\n');if(end<0)return;const header=data.slice(0,end);
         if(header[0]==='$'){const n=Number(header.slice(1));if(n<0){socket.end();resolve(null);}else if(data.length>=end+2+n+2){socket.end();resolve(data.slice(end+2,end+2+n));}}
         else{socket.end();if(header[0]==='-')reject(new Error('Database command failed'));else resolve(header.slice(1));}
         });
         }); }
        command(['SET','native-value','42','NX']).then(() => {
         http.createServer(async(req,res) => {try {const value=await command(['GET','native-value']);res.setHeader('Content-Type','text/html');res.end('<h1>Native UI verification</h1><p>Redis value: '+value+'</p><p>Node '+process.version+'</p>');}catch(e){res.writeHead(503);res.end('Database unavailable');}}).listen(Number(process.env.PORT),'127.0.0.1');
        }).catch(() => {console.error('Database preparation failed');process.exit(1);});
        ''')
        # The private fixture saves one complete proxy ruleset.
        state = json.loads(cli('status'))
        state['proxyRules'] = [{'domain':'service-manager-verify.test','localPort':web_port}]
        state['proxyListenerPort'] = https_port
        state['proxyRulesRevision'] = state.get('proxyRulesRevision',0) + 1
        state_path = root / 'state.json'
        staged = root / '.state-native-ui.json'
        staged.write_text(json.dumps(state))
        staged.chmod(0o600)
        staged.replace(state_path)
        cli('db','start',instance['id'])
        node_binary = root / 'packages' / packages['node'] / manifests['node'].get('binaries',{}).get('node','bin/node')
        app_env = dict(os.environ, PORT=str(web_port), REDIS_PORT=str(redis_port),
                       REDIS_PASSWORD_FILE=str(root/'instances/native-cache/password'))
        external_process = start_external_app(node_binary, app_dir, app_env, web_port,
                                              root/'logs/verification-app.log')
        record = dict(harness=str(devmgr),base=str(base),root=str(root),app=str(app),executable=str(exe),instance=instance['id'],
                      externalPID=external_process.pid, externalBinary=str(node_binary), appDirectory=str(app_dir),
                      webPort=web_port,redisPort=redis_port,httpsPort=https_port,url=f'https://service-manager-verify.test:{https_port}/',
                      labelPrefix='xyz.jyeu.msm.dev.'+hashlib.sha256(str(root).encode()).hexdigest()[:8]+'.',identifier=info['CFBundleIdentifier'])
        (base/'native-ui.json').write_text(json.dumps(record,indent=2)+'\n')
        print(json.dumps(record,indent=2))
    except BaseException:
        stop_external_app(external_process)
        cleanup_fixture(base, base / 'development', devmgr)
        raise


def main(argv=None):
    previous_umask = os.umask(0o077)
    try:
        return _main(argv)
    finally:
        os.umask(previous_umask)


if __name__ == '__main__':
    main()
