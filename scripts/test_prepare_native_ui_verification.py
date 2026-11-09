import importlib.util
import json
import hashlib
from pathlib import Path
import subprocess
import tempfile
import plistlib
import io
import unittest
from unittest import mock


def load_module():
    spec = importlib.util.spec_from_file_location('native_qa', Path(__file__).with_name('prepare-native-ui-verification.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class NativePreparationTests(unittest.TestCase):
    def test_import_and_help_have_no_side_effects(self):
        with mock.patch('subprocess.run') as run, mock.patch('tempfile.mkdtemp') as create:
            module = load_module()
            with self.assertRaises(SystemExit) as result:
                module.main(['--help'])
            self.assertEqual(result.exception.code, 0)
            run.assert_not_called()
            create.assert_not_called()

    def test_missing_and_invalid_harness_fail_before_fixture(self):
        module = load_module()
        with tempfile.TemporaryDirectory() as directory:
            candidate = Path(directory) / 'not-executable'
            candidate.write_text('fixture')
            for argv in [[], ['--devmgr', str(candidate)], ['--devmgr', str(candidate / 'missing')]]:
                with self.subTest(argv=argv), mock.patch('tempfile.mkdtemp') as create, mock.patch('subprocess.run') as run:
                    with self.assertRaises(SystemExit):
                        module.main([*argv, '--reuse-ca', directory])
                    create.assert_not_called()
                    run.assert_not_called()

    def test_commands_use_only_harness_and_private_root(self):
        module = load_module()
        with mock.patch.object(module.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, 'ok', '')) as run:
            for values in [('log-rotation', 'off'), ('install-manifest', 'fixture.json'), ('instance', 'fixture.json'), ('db', 'start', 'fixture')]:
                self.assertEqual(module.run_fixture_command(Path('/fixture/harness'), Path('/fixture/root'), *values), 'ok')
                self.assertEqual(run.call_args.args[0], ['/fixture/harness', *values])
                self.assertEqual(run.call_args.kwargs['env']['MSM_DEV_HOME'], '/fixture/root')

    def test_command_failure_is_not_success(self):
        module = load_module()
        with mock.patch.object(module.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1, '', 'fixture failure')):
            with self.assertRaisesRegex(RuntimeError, 'Fixture command failed'):
                module.run_fixture_command(Path('/fixture/harness'), Path('/fixture/root'), 'db')

    def test_cleanup_restricts_jobs_and_retains_uncertain_resources(self):
        module = load_module()
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory) / 'private'
            root = base / 'development'
            jobs = root / 'jobs'
            jobs.mkdir(parents=True)
            (jobs / 'external-job.plist').write_text('fixture')
            with mock.patch.object(module, 'run_fixture_command'), mock.patch.object(module.subprocess, 'run') as run:
                with self.assertRaisesRegex(RuntimeError, 'retained private directory'):
                    module.cleanup_fixture(base, root, Path('/fixture/harness'))
                run.assert_not_called()
                self.assertTrue(base.exists())
            (jobs / 'external-job.plist').unlink()
            prefix = 'xyz.jyeu.msm.dev.' + hashlib.sha256(str(root).encode()).hexdigest()[:8] + '.'
            (jobs / (prefix + 'test.plist')).write_text('fixture')
            with mock.patch.object(module, 'run_fixture_command'), mock.patch.object(module.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1, b'', b'observation denied')):
                with self.assertRaisesRegex(RuntimeError, 'retained private directory'):
                    module.cleanup_fixture(base, root, Path('/fixture/harness'))
                self.assertTrue(base.exists())
            with mock.patch.object(module, 'run_fixture_command') as harness, mock.patch.object(module.subprocess, 'run', return_value=subprocess.CompletedProcess([], 113, b'', b'Could not find service')) as run:
                module.cleanup_fixture(base, root, Path('/fixture/harness'))
                harness.assert_called_once_with(Path('/fixture/harness'), root, 'maintenance-stop')
                self.assertFalse(base.exists())
                for call in run.call_args_list:
                    self.assertTrue(call.args[0][-1].endswith('/' + prefix + 'test'))

    def test_failure_does_not_publish_success_record(self):
        module = load_module()
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            harness = parent / 'harness'
            harness.write_text('fixture')
            harness.chmod(0o700)
            app = parent / 'source.app'
            app.mkdir()
            private = parent / 'private'
            private.mkdir()
            with mock.patch.object(module.tempfile, 'mkdtemp', return_value=str(private)), mock.patch.object(module.shutil, 'copytree', side_effect=RuntimeError('copy failed')), mock.patch.object(module, 'cleanup_fixture') as cleanup:
                with self.assertRaisesRegex(RuntimeError, 'copy failed'):
                    module.main(['--devmgr', str(harness), '--app', str(app), '--reuse-ca', directory])
                cleanup.assert_called_once_with(private.resolve(), (private / 'development').resolve(), harness.resolve())
                self.assertFalse((private / 'native-ui.json').exists())

    def test_success_records_native_app_not_harness(self):
        module = load_module()
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory).resolve()
            harness = parent / 'harness'; harness.write_text('fixture'); harness.chmod(0o700)
            app = parent / 'source.app'; (app / 'Contents').mkdir(parents=True)
            (app / 'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable':'NativeApp'}))
            cache = parent / 'cache'; (cache / 'downloads').mkdir(parents=True)
            archive = b'fixture-archive'; digest = hashlib.sha256(archive).hexdigest()
            (cache / 'downloads' / digest).write_bytes(archive)
            for name, version in [('node','22.14.0'),('redis','7.4.7')]:
                folder = cache / 'packages' / (name + '@' + version + '-arm64'); folder.mkdir(parents=True)
                (folder / '.package.json').write_text(json.dumps({'sha256':digest}))
            ca = parent / 'ca/proxy'; ca.mkdir(parents=True)
            for name in ['ca.pem','ca-key.pem']: (ca / name).write_text('fixture-only')
            private = parent / 'private'; private.mkdir()
            def run(argv, **kwargs):
                if argv[0] == str(harness):
                    root = Path(kwargs['env']['MSM_DEV_HOME'])
                    (root / 'downloads').mkdir(exist_ok=True)
                    (root / 'proxy').mkdir(exist_ok=True)
                if argv[0] == str(harness) and len(argv)>1 and argv[1]=='status':
                    return subprocess.CompletedProcess(argv,0,json.dumps({'schemaVersion':2,'packages':{},'instances':{},'proxyRules':[],'proxyListenerPort':443}), '')
                return subprocess.CompletedProcess(argv, 0, '', '')
            with mock.patch.object(module.tempfile, 'mkdtemp', return_value=str(private)), mock.patch.object(module.subprocess, 'run', side_effect=run) as calls, mock.patch.object(module,'start_external_app',return_value=mock.Mock(pid=12345)), mock.patch('sys.stdout', new_callable=io.StringIO):
                module.main(['--devmgr',str(harness),'--app',str(app),'--reuse-ca',str(ca.parent),'--node-root',str(cache),'--redis-root',str(cache)])
            record = json.loads((private / 'native-ui.json').read_text())
            self.assertEqual(record['harness'],str(harness))
            self.assertEqual(record['executable'],str(private / 'Devbase Native Verification.app/Contents/MacOS/NativeApp'))
            self.assertEqual(record['externalPID'],12345)
            self.assertNotIn('project',record)
            commands = [x.args[0][1] for x in calls.call_args_list if x.args[0][0] == str(harness)]
            self.assertEqual(commands,['log-rotation','install-manifest','install-manifest','instance','status','db'])

    def test_external_app_failure_and_cancellation_stop_only_owned_process(self):
        module=load_module()
        for failure in [RuntimeError('exited'), KeyboardInterrupt()]:
            process=mock.Mock(pid=12345)
            process.poll.return_value=None
            with tempfile.TemporaryDirectory() as directory, mock.patch.object(module.subprocess,'Popen',return_value=process), mock.patch.object(module.socket,'create_connection',side_effect=failure), mock.patch.object(module.time,'monotonic',side_effect=[0,0]):
                with self.assertRaises(type(failure)):
                    module.start_external_app(Path('/fixture/node'),Path(directory),{},3000,Path(directory)/'app.log')
            process.terminate.assert_called_once()
            process.wait.assert_called_once_with(timeout=5)


if __name__ == '__main__':
    unittest.main()
