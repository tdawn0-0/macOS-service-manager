import ast
import copy
import importlib.util
import json
from pathlib import Path
import unittest

ROOT=Path(__file__).parent
spec=importlib.util.spec_from_file_location('diagnostic_navigation_model',ROOT/'prototype.py')
model=importlib.util.module_from_spec(spec)
spec.loader.exec_module(model)


class DiagnosticNavigationContractTests(unittest.TestCase):
    def test_decision_cases(self):
        data=json.loads((ROOT/'cases.json').read_text())
        self.assertEqual(data['schemaVersion'],1)
        self.assertGreaterEqual(len(data['cases']),15)
        self.assertEqual(len({x['id'] for x in data['cases']}),len(data['cases']))
        for case in data['cases']:
            with self.subTest(case=case['id']):
                before=copy.deepcopy(case['input'])
                got=model.resolve_action(case['input'])
                self.assertEqual(got,case['expected'])
                self.assertEqual(got['mutations'],[])
                self.assertEqual(case['input'],before)

    def test_redaction_is_explicit_projection(self):
        report={'secret':'fixture-private','action':{'id':'local-id'},'checks':[
            {'resource':'/private/fixture','layer':'Runtime','status':'Failed','code':'missing-runtime','summary':'fixture-private','action':{'id':'local-id'}},
            {'resource':'/private/fixture','layer':'Runtime','status':'Failed','code':'missing-runtime'},
            {'resource':'private.example','layer':'private.example','status':'fixture-private','code':'unknown','unknown':'fixture-private'}]}
        before=copy.deepcopy(report)
        got=model.redact_report(report)
        text=json.dumps(got)
        for private in ['fixture-private','local-id','/private/fixture','private.example','action','unknown']:
            self.assertNotIn(private,text)
        self.assertEqual(set(got),{'schemaVersion','redacted','checks'})
        self.assertEqual(got['checks'][0]['resource'],got['checks'][1]['resource'])
        self.assertNotEqual(got['checks'][0]['resource'],got['checks'][2]['resource'])
        self.assertEqual(report,before)
        changed=copy.deepcopy(report)
        for item in changed['checks']: item['resource']='replacement-a' if item['resource']=='/private/fixture' else 'replacement-b'
        self.assertEqual(got,model.redact_report(changed))
        for row in got['checks']:self.assertEqual(set(row),{'layer','resource','status','summary','remedy'})

    def test_malformed_data_and_unknown_fields(self):
        self.assertEqual(model.resolve_action({'action':{'kind':'ca','url':'https://fixture'},'currentState':{}})['status'],'unavailable')
        for data in [None,[],{}, {'action':{'kind':'instance','id':'db'},'currentState':{'instances':4}}]:
            self.assertEqual(model.resolve_action(data)['status'],'unavailable')
        for data in [None,[],{'checks':None},{'checks':[{},None,{'resource':[], 'layer':{},'status':[], 'code':{}}]}]:
            self.assertTrue(model.redact_report(data)['redacted'])

    def test_malformed_action_kinds_are_unavailable(self):
        for kind in [[], {}, None, ['project-service'], {'kind': 'ca'}]:
            with self.subTest(kind=kind):
                fixture = {'action': {'kind': kind}, 'currentState': {'caAvailable': True}}
                before = copy.deepcopy(fixture)
                result = model.resolve_action(fixture)
                self.assertEqual(result, {'status': 'unavailable', 'target': None,
                                          'messageKey': 'unsupported-action', 'mutations': []})
                self.assertEqual(fixture, before)

    def test_no_external_side_effect_apis(self):
        tree=ast.parse((ROOT/'prototype.py').read_text())
        imports={n.names[0].name for n in ast.walk(tree) if isinstance(n,ast.Import)}
        self.assertEqual(imports,{'re'})
        calls={n.func.id for n in ast.walk(tree) if isinstance(n,ast.Call) and isinstance(n.func,ast.Name)}
        self.assertFalse(calls & {'open','exec','eval','compile','__import__'})
        self.assertNotIn('cases.json',(ROOT/'prototype.py').read_text())


if __name__ == '__main__':unittest.main()
