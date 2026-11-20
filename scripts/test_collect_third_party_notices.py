import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('notices', Path(__file__).with_name('collect-third-party-notices.py'))
notices = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notices)


class NoticeCollectionTests(unittest.TestCase):
    def write_asset(self, root, used_by=None, revision='a' * 40):
        folder = root / 'assets/notices/lucide'
        folder.mkdir(parents=True)
        license_bytes = b'Upstream ISC and Feather MIT notices\n'
        (folder / 'LICENSE').write_bytes(license_bytes)
        metadata = dict(name='Lucide', repository='https://github.com/lucide-icons/lucide',
                        licenseRevision=revision,
                        licenseSource='https://raw.githubusercontent.com/lucide-icons/lucide/' + revision + '/LICENSE',
                        licenseSHA256=hashlib.sha256(license_bytes).hexdigest(),
                        usedBy=used_by or ['app/src/components/icons.rs'])
        (folder / 'source.json').write_text(json.dumps(metadata))
        return metadata

    def test_lucide_notice_is_required_and_modified_bytes_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            icons = root / 'app/src/components/icons.rs'
            icons.parent.mkdir(parents=True)
            icons.write_text('//! The Lucide drawings the inspector uses.\n')
            with self.assertRaisesRegex(ValueError, 'require their distribution license'):
                notices.asset_notices(root)
            metadata = self.write_asset(root)
            files, assets = notices.asset_notices(root)
            self.assertEqual(files['third-party-assets/lucide/LICENSE'], b'Upstream ISC and Feather MIT notices\n')
            self.assertEqual(assets[0]['files'][0]['sha256'], metadata['licenseSHA256'])
            (root / 'assets/notices/lucide/LICENSE').write_bytes(b'Incomplete notice')
            with self.assertRaisesRegex(ValueError, 'digest differs'):
                notices.asset_notices(root)

    def test_asset_notice_rejects_unpinned_source_and_missing_usage(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            metadata = self.write_asset(root, used_by=['missing.rs'])
            metadata.update(licenseRevision='main',
                            licenseSource='https://raw.githubusercontent.com/lucide-icons/lucide/main/LICENSE')
            (root / 'assets/notices/lucide/source.json').write_text(json.dumps(metadata))
            with self.assertRaisesRegex(ValueError, 'pin its upstream revision'):
                notices.asset_notices(root)
            metadata.update(licenseRevision='b' * 40,
                            licenseSource='https://raw.githubusercontent.com/lucide-icons/lucide/' + 'b' * 40 + '/LICENSE')
            (root / 'assets/notices/lucide/source.json').write_text(json.dumps(metadata))
            with self.assertRaisesRegex(ValueError, 'usage is missing'):
                notices.asset_notices(root)

    def test_crate_notice_files_are_collected_and_binary_placeholders_skipped(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            (source / 'LICENSE').write_bytes(b'MIT license\n')
            (source / 'LICENSE-APACHE').write_bytes(b'Apache license\n')
            (source / 'src').mkdir()
            (source / 'src/LICENSE.md').write_bytes(b'Nested notice\n')
            (source / 'NOTICE').write_bytes(b'')
            (source / 'COPYING').write_bytes(b'\0binary')
            (source / 'README.md').write_bytes(b'not a notice\n')

            files, entries = notices.license_notices(source, Path('third-party/crate@1.0.0'))

            self.assertEqual(sorted(files), ['third-party/crate@1.0.0/LICENSE',
                                             'third-party/crate@1.0.0/LICENSE-APACHE',
                                             'third-party/crate@1.0.0/src/LICENSE.md'])
            self.assertEqual(entries[0], {'path': 'third-party/crate@1.0.0/LICENSE',
                                          'sha256': hashlib.sha256(b'MIT license\n').hexdigest()})

    def test_crate_notice_rejects_symlink_and_oversized_files(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            (source / 'target').write_bytes(b'license text\n')
            os.symlink(source / 'target', source / 'LICENSE')
            with self.assertRaisesRegex(ValueError, 'plain files'):
                notices.license_notices(source, Path('third-party/crate@1.0.0'))
            (source / 'LICENSE').unlink()
            (source / 'LICENSE').write_bytes(b'x' * (4 * 1024 * 1024 + 1))
            with self.assertRaisesRegex(ValueError, 'unusual notice file'):
                notices.license_notices(source, Path('third-party/crate@1.0.0'))

    def test_production_graph_excludes_dev_dependencies(self):
        data = {
            'workspace_members': ['root'],
            'resolve': {'nodes': [
                {'id': 'root', 'deps': [
                    {'pkg': 'normal', 'dep_kinds': [{'kind': None}]},
                    {'pkg': 'build', 'dep_kinds': [{'kind': 'build'}]},
                    {'pkg': 'dev', 'dep_kinds': [{'kind': 'dev'}]},
                    {'pkg': 'normal', 'dep_kinds': [{'kind': 'dev'}]},
                ]},
                {'id': 'normal', 'deps': []},
                {'id': 'build', 'deps': []},
                {'id': 'dev', 'deps': []},
            ]},
        }
        self.assertEqual(notices.production_ids(data), {'root', 'normal', 'build'})

    def test_check_rejects_drift_and_extra_files(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            files = {'manifest.json': b'{}\n', 'third-party/crate/LICENSE': b'MIT\n'}
            for name, content in files.items():
                (target / name).parent.mkdir(parents=True, exist_ok=True)
                (target / name).write_bytes(content)
            notices.verify_inventory(files, target)

            (target / 'third-party/crate/LICENSE').write_bytes(b'Modified\n')
            with self.assertRaisesRegex(ValueError, 'Notice inventory differs'):
                notices.verify_inventory(files, target)

            (target / 'third-party/crate/LICENSE').write_bytes(b'MIT\n')
            (target / 'third-party/obsolete').mkdir()
            (target / 'third-party/obsolete/LICENSE').write_bytes(b'stale\n')
            with self.assertRaisesRegex(ValueError, 'third-party/obsolete/LICENSE'):
                notices.verify_inventory(files, target)


if __name__ == '__main__':
    unittest.main()
