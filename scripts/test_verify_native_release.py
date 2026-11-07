import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock


def module():
    spec = importlib.util.spec_from_file_location('native_release', Path(__file__).with_name('verify-native-release.py'))
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def events(names):
    package = 'macos-service-manager'
    rows = [{'Package': package, 'Action': 'start'}]
    for name in names:
        rows += [{'Package': package, 'Test': name, 'Action': 'run'},
                 {'Package': package, 'Test': name, 'Action': 'pass'}]
    rows += [{'Package': package, 'Action': 'pass'}]
    return rows


def lines(rows):
    return [json.dumps(row) + '\n' for row in rows]


class NativeReleaseTests(unittest.TestCase):
    def test_malformed_action_types_are_rejected(self):
        m=module()
        for action in [[], {}, None, 17, True]:
            with self.subTest(action=action), self.assertRaisesRegex(ValueError,'action'):
                m.check_events(lines([{'Package':m.PACKAGE,'Action':action}]))

    def test_malformed_formula_metadata_has_sanitized_error(self):
        m=module()
        valid={'name':'postgresql@17','full_name':'postgresql@17','versions':{'stable':'17.11'},'revision':1}
        bad=[{},None,[],{'formulae':None},{'formulae':{}},{'formulae':[]},{'formulae':[{}]}, {'formulae':[None]}, {'formulae':[valid,valid]}]
        for changes in [{'versions':None},{'versions':[]},{'versions':{'stable':[]}},{'name':'../../private-sentinel'},{'name':{}},{'revision':True},{'revision':-1},{'revision':{}},{'full_name':[]}]:
            bad.append({'formulae':[dict(valid,**changes)]})
        for value in bad:
            with self.subTest(value=value), mock.patch.object(m,'checked',return_value=json.dumps(value)):
                with self.assertRaisesRegex(RuntimeError,'invalid Homebrew formula metadata: postgresql') as error:
                    m.validate_existing_kegs()
                self.assertNotIn('sentinel',str(error.exception))
        with mock.patch.object(m,'checked',return_value='{private-sentinel'):
            with self.assertRaisesRegex(RuntimeError,'invalid Homebrew formula metadata: postgresql') as error:m.validate_existing_kegs()
            self.assertNotIn('sentinel',str(error.exception))

    def test_complete_stream_and_package_events(self):
        m = module()
        rows = events(m.REQUIRED)
        rows.insert(1, {'Package':m.PACKAGE,'Action':'output','Output':'private output must not be copied'})
        result = m.check_events(lines(rows))
        self.assertEqual(len(result), 8)
        self.assertNotIn('private', json.dumps(result))

    def test_missing_skip_fail_duplicate_and_truncated_rejected(self):
        m = module()
        complete = events(m.REQUIRED)
        scenarios = [events(m.REQUIRED[:-1]), complete[:-1], complete + complete,
                     complete[:-1] + [{'Package':m.PACKAGE,'Action':'fail'}],
                     complete[:-1] + [complete[2]] + [complete[-1]],
                     [], [{'Package':'foreign','Action':'pass'}],
                     [{'Package':m.PACKAGE,'Action':'build-fail'}]]
        for action in ['skip', 'fail']:
            scenarios.append([dict(row, Action=action) if row.get('Test')==m.REQUIRED[0] and row['Action']=='pass' else row for row in complete])
        for rows in scenarios:
            with self.subTest(rows=rows), self.assertRaises(ValueError):
                m.check_events(lines(rows))
        for value in ['{', '{}', 'null', '{"Package":']:
            with self.subTest(value=value), self.assertRaises(ValueError):
                m.check_events([value])

    def test_help_and_import_do_not_create_resources(self):
        with mock.patch('subprocess.run') as run, mock.patch('tempfile.mkdtemp') as create:
            m = module()
            with mock.patch('sys.stdout', new_callable=io.StringIO), self.assertRaises(SystemExit) as exit:
                m.main(['--help'])
            self.assertEqual(exit.exception.code,0)
            run.assert_not_called(); create.assert_not_called()

    def test_daily_machine_rejected_before_creation_or_install(self):
        m=module()
        with mock.patch.object(m.platform,'system',return_value='Darwin'), mock.patch.object(m.platform,'machine',return_value='arm64'), mock.patch.object(m.os,'getuid',return_value=501), mock.patch.dict(os.environ,{'GITHUB_ACTIONS':'false','RUNNER_ENVIRONMENT':'local'}), mock.patch.object(m,'checked') as run, mock.patch.object(m.tempfile,'mkdtemp') as create:
            with self.assertRaisesRegex(RuntimeError,'disposable'):
                m.run_native()
            run.assert_not_called(); create.assert_not_called()

    def test_preflight_tools_disk_and_domain(self):
        m=module()
        with mock.patch.object(m.platform,'system',return_value='Darwin'), mock.patch.object(m.platform,'machine',return_value='arm64'), mock.patch.object(m.os,'getuid',return_value=501), mock.patch.dict(os.environ,{'GITHUB_ACTIONS':'true','RUNNER_ENVIRONMENT':'github-hosted'}), mock.patch.object(m.shutil,'which',return_value='/tool'), mock.patch.object(m.shutil,'disk_usage',return_value=mock.Mock(free=40*(1<<30))), mock.patch.object(m,'checked') as run:
            m.preflight()
            run.assert_called_once_with(['launchctl','print','gui/501'],timeout=20)
            with mock.patch.object(m.shutil,'which',return_value=None), self.assertRaisesRegex(RuntimeError,'missing'):
                m.preflight()
            with mock.patch.object(m.shutil,'disk_usage',return_value=mock.Mock(free=1)), self.assertRaisesRegex(RuntimeError,'disk'):
                m.preflight()

    def test_runner_prepares_exact_tools_and_sanitized_summary(self):
        m=module()
        with tempfile.TemporaryDirectory() as directory:
            base=Path(directory).resolve(); root=base/'private'; root.mkdir()
            packages={}
            for index,(name,version) in enumerate(m.VERSIONS):
                formula='postgresql@17' if name=='postgresql' else ''
                packages[str(index)]={'name':name,'version':version,'arch':'arm64','sha256':'a'*64,'url':'https://upstream.example/private?token=fixture','formula':formula,'secret':'not-exported'}
            def command(argv,**kwargs):
                if argv[-1]=='status':return json.dumps({'packages':packages,'private':'do-not-export'})
                return 'fixture-version'
            previous=Path.cwd()
            try:
                os.chdir(base)
                with mock.patch.object(m,'preflight'), mock.patch.object(m.tempfile,'mkdtemp',return_value=str(root)), mock.patch.object(m,'checked',side_effect=command) as calls, mock.patch.object(m,'test_phase',side_effect=lambda root,env,names:[{'test':name,'result':'pass'} for name in names]) as phases:
                    result=m.run_native()
                self.assertFalse(root.exists())
                install=[call.args[0][-2:] for call in calls.call_args_list if 'install' in call.args[0]]
                self.assertEqual(install,[list(row) for row in m.VERSIONS])
                self.assertEqual(len(phases.call_args_list),2)
                for call in phases.call_args_list:
                    self.assertEqual(call.args[1]['MSM_DEV_HOME'],str(root))
                    self.assertEqual(call.args[1]['MSM_UI_E2E_PACKAGE_CACHE'],str(root/'downloads'))
                    self.assertEqual(call.args[1]['MSM_UI_E2E_HOMEBREW'],'1')
                for forbidden in ['not-exported','do-not-export','private?token',str(root)]:
                    self.assertNotIn(forbidden,json.dumps(result))
                self.assertTrue((base/'build/native-qa/results.json').is_file())
                self.assertFalse(any('services' in call.args[0] for call in calls.call_args_list))
            finally:os.chdir(previous)

    def test_failure_cleans_only_own_root_and_no_success_record(self):
        m=module()
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory).resolve()/'private';root.mkdir()
            def command(argv,**kwargs):
                if 'install' in argv:raise RuntimeError('install failed')
                return ''
            with mock.patch.object(m,'preflight'),mock.patch.object(m.tempfile,'mkdtemp',return_value=str(root)),mock.patch.object(m,'checked',side_effect=command) as run:
                with self.assertRaisesRegex(RuntimeError,'install failed'):m.run_native()
            self.assertFalse(root.exists())
            last=run.call_args
            self.assertEqual(last.args[0],[str(root/'devmgr'),'maintenance-stop'])
            self.assertEqual(last.kwargs['env']['MSM_DEV_HOME'],str(root))

    def test_test_phase_validates_events_and_timeout_reaps_group(self):
        m=module()
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            process=mock.Mock(pid=12345)
            process.wait.return_value=0
            def launch(argv,**kwargs):
                kwargs['stdout'].write(''.join(lines(events(m.CORE))))
                return process
            with mock.patch.object(m.subprocess,'Popen',side_effect=launch) as run:
                result=m.test_phase(root,{'MSM_DEV_HOME':str(root)},m.CORE)
                self.assertEqual(len(result),2)
                self.assertTrue(run.call_args.kwargs['start_new_session'])
                self.assertIn('-race',run.call_args.args[0])
            process.wait.side_effect=[subprocess.TimeoutExpired('go',1800),-9]
            with mock.patch.object(m.subprocess,'Popen',return_value=process), mock.patch.object(m.os,'killpg') as kill:
                with self.assertRaisesRegex(RuntimeError,'timed out'):
                    m.test_phase(root,{},m.CORE)
                kill.assert_called_once_with(12345,m.signal.SIGKILL)
                self.assertEqual(process.wait.call_args.args,())
            process.wait.side_effect=None; process.wait.return_value=1
            with mock.patch.object(m.subprocess,'Popen',return_value=process), self.assertRaisesRegex(RuntimeError,'failed'):
                m.test_phase(root,{},m.CORE)

    def test_package_summary_rejects_missing_names_or_wrong_formula(self):
        m=module()
        row={'name':'node','version':'22.14.0','arch':'arm64','sha256':'a'*64,'url':'https://example.test/archive'}
        with self.assertRaisesRegex(ValueError,'missing'):
            m.package_summary({str(i):row for i in range(6)})
        for source in ['https://user:password@example.test','http://example.test']:
            with self.assertRaisesRegex(ValueError,'source'):
                m.package_summary({'one':dict(row,url=source)})
        with self.assertRaisesRegex(ValueError,'identity'):
            m.package_summary({'one':dict(row,name='php',formula='php@8.5')})

    def test_matrix_regex_selects_only_top_level_name(self):
        m=module()
        with tempfile.TemporaryDirectory() as directory:
            process=mock.Mock(); process.wait.return_value=0
            def launch(argv,**kwargs):
                kwargs['stdout'].write(''.join(lines(events(m.MATRIX))))
                return process
            with mock.patch.object(m.subprocess,'Popen',side_effect=launch) as call:
                m.test_phase(Path(directory),{},m.MATRIX)
            argv=call.call_args.args[0]
            self.assertEqual(argv[argv.index('-run')+1],'^(TestUISoftwareCatalogAllFive)$')

    def test_rejected_preflight_invalidates_old_success(self):
        m=module()
        with tempfile.TemporaryDirectory() as directory:
            previous=Path.cwd()
            try:
                os.chdir(directory)
                result=Path('build/native-qa/results.json');result.parent.mkdir(parents=True);result.write_text('stale pass')
                with mock.patch.object(m,'preflight',side_effect=RuntimeError('blocked')):
                    with self.assertRaisesRegex(RuntimeError,'blocked'):m.run_native()
                self.assertFalse(result.exists())
            finally:os.chdir(previous)

    def test_local_exact_receipts_and_missing_keg_rejection(self):
        m=module()
        with tempfile.TemporaryDirectory() as directory:
            cellar=Path(directory)
            for rack,version,tap,stable,binaries in [('postgresql@17','17.11_1','homebrew/core','17.11',['bin/'+x for x in ['postgres','initdb','pg_dump','pg_restore','psql','createdb']])]:
                keg=cellar/rack/version;keg.mkdir(parents=True)
                (keg/'INSTALL_RECEIPT.json').write_text(json.dumps({'source':{'tap':tap,'versions':{'stable':stable}}}))
                for name in binaries:
                    file=keg/name;file.parent.mkdir(exist_ok=True);file.write_text('fixture');file.chmod(0o700)
            def info(argv,**kwargs):
                self.assertEqual(argv[:3],list(m.SANDBOX))
                self.assertNotIn('install',argv);self.assertNotIn('tap',argv)
                return json.dumps({'formulae':[{'name':'postgresql@17','full_name':'postgresql@17','versions':{'stable':'17.11'},'revision':1}]})
            real_path=Path
            with mock.patch.object(m,'Path',side_effect=lambda value:cellar if value=='/opt/homebrew/Cellar' else real_path(value)), mock.patch.object(m,'checked',side_effect=info):
                m.validate_existing_kegs()
                (cellar/'postgresql@17/17.11_1/INSTALL_RECEIPT.json').unlink()
                with self.assertRaisesRegex(RuntimeError,'missing'):m.validate_existing_kegs()

    def test_local_runner_sandboxes_preparation_and_runs_native_phases_directly(self):
        m=module()
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory).resolve()/'private';root.mkdir()
            previous=Path.cwd()
            def command(argv,**kwargs):
                if argv[-1]=='status':return '{"packages":{}}'
                return 'fixture-version'
            try:
                os.chdir(directory)
                with mock.patch.object(m,'preflight') as preflight, mock.patch.object(m,'validate_existing_kegs') as receipts, mock.patch.object(m,'cleanup_local_jobs') as cleanup, mock.patch.object(m.tempfile,'mkdtemp',return_value=str(root)), mock.patch.object(m,'checked',side_effect=command) as calls, mock.patch.object(m,'package_summary',return_value=[]), mock.patch.object(m,'test_phase',return_value=[]) as phases:
                    observed=[]
                    phases.side_effect=lambda *args: observed.append(receipts.call_count) or []
                    m.run_native(True)
                    self.assertEqual(observed,[2,3])
                preflight.assert_called_once_with(True);self.assertEqual(receipts.call_count,3);cleanup.assert_called_once_with(root)
                for call in calls.call_args_list:
                    argv=call.args[0]
                    self.assertNotIn('brew',argv)
                    if str(root/'devmgr') in argv or 'build' in argv:
                        self.assertEqual(argv[:3],list(m.SANDBOX))
                for call in phases.call_args_list:
                    self.assertEqual(len(call.args),3)
                    self.assertEqual(call.args[1]['TMPDIR'],str(root/'tests'))
            finally:os.chdir(previous)

    def test_changed_keg_prevents_native_phase(self):
        m=module()
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory).resolve()/'private';root.mkdir()
            previous=Path.cwd()
            def command(argv,**kwargs):
                return '{"packages":{}}' if argv[-1]=='status' else ''
            try:
                os.chdir(directory)
                with mock.patch.object(m,'preflight'), mock.patch.object(m,'validate_existing_kegs',side_effect=[None,RuntimeError('receipt changed')]), mock.patch.object(m,'cleanup_local_jobs'), mock.patch.object(m.tempfile,'mkdtemp',return_value=str(root)), mock.patch.object(m,'checked',side_effect=command), mock.patch.object(m,'package_summary',return_value=[]), mock.patch.object(m,'test_phase') as phases:
                    with self.assertRaisesRegex(RuntimeError,'receipt changed'):m.run_native(True)
                    phases.assert_not_called()
            finally:os.chdir(previous)

    def test_failure_is_machine_readable(self):
        m=module()
        with mock.patch.object(m,'run_native',side_effect=RuntimeError('exact existing keg is missing: postgresql 17.11_1')), mock.patch('sys.stderr',new_callable=io.StringIO) as errors:
            with self.assertRaises(SystemExit) as exited:m.main(['--run','--local-existing-kegs'])
        self.assertEqual(exited.exception.code,1)
        value=json.loads(errors.getvalue())
        self.assertEqual(value,{'schemaVersion':1,'status':'blocked','error':'exact existing keg is missing: postgresql 17.11_1'})

    def test_subprocess_failure_output_stays_private(self):
        m=module()
        with mock.patch.object(m.subprocess,'run',return_value=subprocess.CompletedProcess([],1,'private output','private secret')):
            with self.assertRaises(RuntimeError) as caught:m.checked(['fixture'])
            self.assertNotIn('private',str(caught.exception))

    def test_workflow_requires_unsigned_native_gate(self):
        folder=Path(__file__).resolve().parent.parent/'.github/workflows'
        release=(folder/'github-actions-publish.yml').read_text()
        gate=release.split('  native-test:\n',1)[1].split('\n  release:',1)[0]
        self.assertIn('needs: native-test',release.split('\n  release:',1)[1])
        self.assertIn('verify-native-release.py --run',gate)
        self.assertIn('contents: read',gate)
        for forbidden in ['secrets.', 'environment: release','continue-on-error','if:']:
            self.assertNotIn(forbidden,gate)
        ci=(folder/'ci.yml').read_text()
        self.assertIn('workflow_dispatch:',ci)
        self.assertIn('verify-native-release.py --run',ci)


if __name__=='__main__':unittest.main()
