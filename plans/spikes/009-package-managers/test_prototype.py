import ast
import copy
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).parent
spec = importlib.util.spec_from_file_location('package_manager_model', ROOT / 'prototype.py')
model = importlib.util.module_from_spec(spec)
spec.loader.exec_module(model)


class PackageManagerContractTests(unittest.TestCase):
    def test_decision_cases(self):
        data = json.loads((ROOT / 'cases.json').read_text())
        self.assertEqual(data['schemaVersion'], 1)
        self.assertGreaterEqual(len(data['cases']), 16)
        self.assertEqual(len({x['id'] for x in data['cases']}), len(data['cases']))
        for case in data['cases']:
            with self.subTest(case=case['id']):
                before = copy.deepcopy(case['input'])
                got = model.resolve_request(case['input'])
                self.assertEqual(got, case['expected'])
                self.assertEqual(case['input'], before)
                self.assertTrue(got['requiresConfirmation'])
                if got['status'] != 'ready': self.assertEqual(got['argv'], [])

    def test_untrusted_and_unknown_inputs_do_not_escape(self):
        for data in [None, [], {}, {'privatePath':'/private/fixture'}, {'package':[], 'nodeAvailable':True},
                     {'package':{'packageManager':'npm@1.2.3;untrusted'},'nodeAvailable':True}]:
            with self.subTest(data=data):
                out = model.resolve_request(data)
                self.assertEqual(out['status'], 'blocked')
                self.assertEqual(out['argv'], [])
                self.assertNotIn('/private/fixture', json.dumps(out))
                self.assertNotIn('untrusted', json.dumps(out))

    def test_no_implicit_tool_install_or_case_id_dispatch(self):
        case=json.loads((ROOT/'cases.json').read_text())['cases'][0]['input']
        case['tools']=[]
        self.assertEqual(model.resolve_request(case)['reason'],'exact-tool-unavailable')
        tree=ast.parse((ROOT/'prototype.py').read_text())
        imports={n.names[0].name for n in ast.walk(tree) if isinstance(n, ast.Import)}
        self.assertEqual(imports, {'re'})
        calls={n.func.id for n in ast.walk(tree) if isinstance(n,ast.Call) and isinstance(n.func,ast.Name)}
        self.assertFalse(calls & {'open','exec','eval','compile','__import__'})
        self.assertNotIn('cases.json',(ROOT/'prototype.py').read_text())


if __name__ == '__main__': unittest.main()
