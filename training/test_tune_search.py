import copy
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
import tune_search as tuning

BASE = 'RMPROFILE1,0,0,10000,600,1200,3000,4000,2000,3000,8,14,22,3,8,7,1000,8,1,0,0,7,12,10,24'

class SearchTuningTests(unittest.TestCase):
    def test_profile_flags_and_spsa_are_explicit_and_deterministic(self):
        base = tuning.parse_profile(BASE)
        self.assertEqual(tuning.parse_profile(tuning.serialize_profile(base)), base)
        for name in ('lmr_v2','improving','iid','policy_lmr','policy_pruning','singular','competitive_tt','null_move','qsearch_threes'):
            pair, _ = tuning.feature_pair({}, base, name)
            values = [tuning.parse_profile(p) for p in pair]
            self.assertEqual([key for key in base if values[0][key] != values[1][key]], [name])
        with self.assertRaisesRegex(ValueError, 'explicitly enabled'):
            tuning.feature_pair({}, base, 'lmr_v2_policy')
        theta={'lmr_divisor':4.0}; bounds={'lmr_divisor':[2,10]}
        plus,minus=tuning.perturb(theta,bounds,17,4,.1)
        self.assertEqual((plus,minus),tuning.perturb(theta,bounds,17,4,.1))
        next_theta=tuning.update(theta,plus,minus,bounds,.75,4,.01)
        self.assertTrue(2<=next_theta['lmr_divisor']<=10)
        self.assertEqual(theta,tuning.update(theta,plus,minus,bounds,.5,4,.01))

    def test_resume_identity_and_canonical_suite_separation(self):
        with tempfile.TemporaryDirectory() as raw:
            root=Path(raw); profile=root/'base.profile'; profile.write_text(BASE)
            arena=root/'arena.exe';arena.write_bytes(b'fake executable - never executed')
            train=root/'train.rmg';train.write_text('train')
            confirm=root/'confirm.rmg';confirm.write_text('confirm')
            config={'version':1,'mode':'spsa','stage':'lmr','arena':str(arena),
                    'base_profile':str(profile),'nodes':100,'pairs':1,'steps':2,'seed':3,
                    'bounds':{'lmr_divisor':[2,10]},'tuning_openings':[str(train)],
                    'confirmation_openings':[str(confirm)]}
            def describe(_arena,args):
                path=Path(args[args.index('--opening-record')+1])
                return {'openings':[path.read_text()], 'inputs_sha256':{str(arena):tuning.file_hash(arena),str(path):tuning.file_hash(path)}}
            def run_match(configuration, output):
                self.assertEqual(configuration['suite_role'],'tuning')
                self.assertEqual(configuration['stop_rule'],'fixed_pairs')
                tuning.save_manifest(output/'manifest.json',configuration)
                return {'pairs':1,'mean_score':.75}
            with patch.object(tuning.experiment,'describe',side_effect=describe), patch.object(tuning.experiment,'run',side_effect=run_match), patch.object(tuning,'telemetry',return_value=[]):
                output=root/'output'
                tuning.run(config,output,True)
                before={p.name:p.read_bytes() for p in output.glob('*') if p.is_file()}
                tuning.run(config,output,True)
                self.assertEqual(before,{p.name:p.read_bytes() for p in output.glob('*') if p.is_file()})
                changed=copy.deepcopy(config);changed['seed']=4
                with self.assertRaises(ValueError):tuning.run(changed,output)
                arena.write_bytes(b'changed')
                with self.assertRaises(ValueError):tuning.run(config,output)
                confirm.write_text('train')
                with self.assertRaisesRegex(ValueError,'leakage'):tuning.run(config,root/'leak')

if __name__=='__main__':unittest.main()
