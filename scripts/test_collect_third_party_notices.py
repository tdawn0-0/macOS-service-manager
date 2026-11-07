import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('notices', Path(__file__).with_name('collect-third-party-notices.py'))
notices = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notices)


class NoticeCollectionTests(unittest.TestCase):
    def test_patents_does_not_satisfy_required_toolchain_license(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'PATENTS').write_text('Patent grant only')
            with self.assertRaisesRegex(ValueError, 'PATENTS alone is insufficient'):
                notices.toolchain_notice(root, 'LICENSE')

    def test_homebrew_formula_license_is_used_only_for_its_libexec(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = Path(directory)
            goroot = prefix / 'libexec'
            goroot.mkdir()
            (prefix / 'LICENSE').write_bytes(b'Formula distribution Go license')
            with self.assertRaises(ValueError):
                notices.toolchain_notice(goroot, 'LICENSE')
            (prefix / 'INSTALL_RECEIPT.json').write_text('{}')
            self.assertEqual(notices.toolchain_notice(goroot, 'LICENSE'), b'Formula distribution Go license')
            (goroot / 'LICENSE').write_bytes(b'Official toolchain license')
            self.assertEqual(notices.toolchain_notice(goroot, 'LICENSE'), b'Official toolchain license')

    def test_lucide_notice_is_required_and_modified_bytes_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'icons.go').write_text('// Lucide: https://lucide.dev\n')
            with self.assertRaisesRegex(ValueError, 'require their distribution license'):
                notices.asset_notices(root)
            folder = root / 'assets/notices/lucide'
            folder.mkdir(parents=True)
            license_bytes = b'Upstream ISC and Feather MIT notices\n'
            (folder / 'LICENSE').write_bytes(license_bytes)
            revision = 'a' * 40
            metadata = dict(name='Lucide', repository='https://github.com/lucide-icons/lucide',
                            licenseRevision=revision, licenseSource='https://raw.githubusercontent.com/lucide-icons/lucide/' + revision + '/LICENSE',
                            licenseSHA256=hashlib.sha256(license_bytes).hexdigest(), usedBy=['icons.go'])
            (folder / 'source.json').write_text(json.dumps(metadata))
            files, assets = notices.asset_notices(root)
            self.assertEqual(files['third-party-assets/lucide/LICENSE'], license_bytes)
            self.assertEqual(assets[0]['files'][0]['sha256'], metadata['licenseSHA256'])
            (folder / 'LICENSE').write_bytes(b'Incomplete notice')
            with self.assertRaisesRegex(ValueError, 'digest differs'):
                notices.asset_notices(root)

    def test_asset_notice_rejects_unpinned_source_and_missing_usage(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            folder = root / 'assets/notices/lucide'
            folder.mkdir(parents=True)
            data = b'ISC notice'
            (folder / 'LICENSE').write_bytes(data)
            metadata = dict(name='Lucide', repository='https://github.com/lucide-icons/lucide', licenseRevision='main',
                            licenseSource='https://raw.githubusercontent.com/lucide-icons/lucide/main/LICENSE',
                            licenseSHA256=hashlib.sha256(data).hexdigest(), usedBy=['missing.go'])
            (folder / 'source.json').write_text(json.dumps(metadata))
            with self.assertRaisesRegex(ValueError, 'pin its upstream revision'):
                notices.asset_notices(root)
            metadata.update(licenseRevision='b' * 40, licenseSource='https://raw.githubusercontent.com/lucide-icons/lucide/' + 'b' * 40 + '/LICENSE')
            (folder / 'source.json').write_text(json.dumps(metadata))
            with self.assertRaisesRegex(ValueError, 'usage is missing'):
                notices.asset_notices(root)


if __name__ == '__main__':
    unittest.main()
