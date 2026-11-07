#!/usr/bin/env python3
import os,json,subprocess,socket,tempfile,pathlib,ssl,time,atexit
import shutil,hashlib,platform
root=tempfile.mkdtemp(prefix='msm-smoke-');env=dict(os.environ,MSM_DEV_HOME=root);cli=os.environ.get('MSM_SMOKE_BINARY','/tmp/msm-devmgr')
if platform.system() != 'Darwin' or platform.machine() != 'arm64':
 raise SystemExit('This verification requires an Apple Silicon Mac (arm64).')
arch='arm64'
cache=os.environ.get('MSM_SMOKE_CACHE')
if cache:shutil.copytree(cache,pathlib.Path(root,'downloads'),dirs_exist_ok=True)
print('Isolated test directory:',root,flush=True)
def cleanup_jobs():
 if not pathlib.Path(root).exists():return
 subprocess.run([cli,'maintenance-stop'],env=env,capture_output=True,timeout=40)
 prefix='xyz.jyeu.msm.dev.'+hashlib.sha256(root.encode()).hexdigest()[:8]+'.'
 for path in pathlib.Path(root,'jobs').glob('*.plist'):
  if path.stem.startswith(prefix):subprocess.run(['launchctl','bootout',f'gui/{os.getuid()}/'+path.stem],capture_output=True,timeout=40)
atexit.register(cleanup_jobs)
def run(*args):
 p=subprocess.run([cli,*args],env=env,text=True,capture_output=True,timeout=180)
 if p.returncode:raise RuntimeError(str(args)+' '+p.stderr+p.stdout)
 return p.stdout

def free():
 s=socket.socket();s.bind(('127.0.0.1',0));n=s.getsockname()[1];s.close();return n

def config(name,data):
 path=f'{root}/{name}.json';pathlib.Path(path).write_text(json.dumps(data));return path
for name,version in [('node','22.14.0'),('node','20.19.0'),('go','1.24.1'),('postgresql','17-2.9.6'),('valkey','9.1.2')]:print(run('install',name,version),flush=True)
port,https,pg,vk=[free() for _ in range(4)]
projectdir=tempfile.mkdtemp(prefix='msm-project-')
pathlib.Path(projectdir,'package.json').write_text('{"scripts":{"dev":"node server.js"}}')
pathlib.Path(projectdir,'server.js').write_text(f'require("http").createServer((q,s)=>s.end("managed-node-ok "+process.version)).listen({port},"127.0.0.1");')
p=json.loads(run('import',projectdir));pid=p['id']
p['services']=[dict(id='web',package='node@22.14.0-'+arch,command=['node','server.js'],health=f'tcp://127.0.0.1:{port}')]
p['instances']=['integration-pg','integration-valkey']
p['sites']=[dict(id='integration-site',domain='integration.test',upstream=f'http://127.0.0.1:{port}',port=https)]
run('instance',config('pg',dict(id='integration-pg',kind='postgresql',package='postgresql@17-2.9.6-universal',port=pg,persistent=True)))
run('instance',config('vk',dict(id='integration-valkey',kind='valkey',package='valkey@9.1.2-universal',port=vk,persistent=True)))
run('project',pid,config('project',p))
try:
 print(run('up',pid),flush=True)
 for n in range(60):
  try:
   ctx=ssl.create_default_context(cafile=f'{root}/proxy/ca.pem');sock=ctx.wrap_socket(socket.create_connection(('127.0.0.1',https)),server_hostname='integration.test');sock.sendall(b'GET / HTTP/1.1\r\nHost: integration.test\r\nConnection: close\r\n\r\n');body=b''
   while True:
    chunk=sock.recv(8192)
    if not chunk:break
    body+=chunk
   sock.close();assert b'managed-node-ok v22.14.0' in body,body;break
  except (OSError,AssertionError):
   if n==59:raise
   time.sleep(.2)
 print('PASS: launchctl + Node + verified HTTPS reverse proxy',flush=True)
 run('down',pid)
 p['services'][0]['package']='node@20.19.0-'+arch
 run('project',pid,config('project',p));assert 'v20.19.0' in run('exec',pid,'node','--version')
 p['services'][0]['package']='go@1.24.1-'+arch
 run('project',pid,config('project',p));assert 'go1.24.1' in run('exec',pid,'go','version')
 p['services'][0]['package']='node@22.14.0-'+arch
 run('project',pid,config('project',p));run('up',pid)
 print('PASS: Node versions coexist; Go selected per project',flush=True)
 run('createdb','integration-pg','source_db')
 pgbin=f'{root}/packages/postgresql@17-2.9.6-universal/Postgres.app/Contents/Versions/17/bin'
 pgenv=dict(env,PGPASSWORD=pathlib.Path(root,'instances/integration-pg/password').read_text())
 def sql(db,query):return subprocess.check_output([pgbin+'/psql','-h','127.0.0.1','-p',str(pg),'-U','devmanager','-d',db,'-Atc',query],env=pgenv,text=True)
 sql('source_db','create table sample (n int); insert into sample values (42);')
 backup=run('backup','integration-pg','source_db').strip();run('restore','integration-pg','restored_db',backup);assert sql('restored_db','select n from sample').strip()=='42'
 print('PASS: PostgreSQL create/write/backup/restore/query',flush=True)
 vcli=root+'/packages/valkey@9.1.2-universal/bin/valkey-cli';venv=dict(env,VALKEYCLI_AUTH=pathlib.Path(root,'instances/integration-valkey/password').read_text())
 assert subprocess.check_output([vcli,'-p',str(vk),'PING'],env=venv,text=True).strip()=='PONG'
 print('PASS: Valkey authenticated PING',flush=True)
finally:
 print(run('down',pid),flush=True)
 run('maintenance-stop')
 subprocess.run(['launchctl','bootout',f'gui/{os.getuid()}/xyz.jyeu.msm.dev.{hashlib.sha256(root.encode()).hexdigest()[:8]}.proxy'],capture_output=True)
print('PASS: stopped test services; data retained; no hosts or system trust changes')

shutil.rmtree(root)
shutil.rmtree(projectdir)
