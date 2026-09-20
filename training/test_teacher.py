import unittest
from contextlib import closing
import torch
from teacher import comparison, explore, masked_loss


class TeacherTests(unittest.TestCase):
    def test_common_horizon_and_unknown_mask(self):
        candidates = [dict(move=i, score=i * 100, completed_depth=3, nominal_depth_valid=True,
                           bound='Exact', source='AlphaBeta') for i in range(4)]
        candidates[2]['bound'] = 'Unknown'
        candidates[3]['completed_depth'] = 2
        result = comparison(dict(perspective='root-side-to-move', completed_depth=3,
                                 candidates=candidates), 100)
        self.assertEqual(result['moves'], [0, 1])
        self.assertAlmostEqual(sum(result['probabilities']), 1)
        self.assertEqual(explore(result, 123), explore(result, 123))
        self.assertEqual(explore(result, 123, protected_move=7), 7)
        logits = torch.zeros(4, requires_grad=True)
        masked_loss(logits, result['moves'], result['probabilities']).backward()
        self.assertEqual(logits.grad[2:].tolist(), [0, 0])
        self.assertLess(logits.grad[1].item(), 0)

    def test_ranking_does_not_train_ties_or_unobserved_moves(self):
        logits = torch.zeros(4, requires_grad=True)
        masked_loss(logits, [0, 1], [.5, .5], ranking_scores=[2, 2]).backward()
        self.assertEqual(logits.grad.tolist(), [0, 0, 0, 0])

    def test_comparison_sidecar_is_bound_and_loaded_without_row_index(self):
        import json
        import sqlite3
        import tempfile
        from pathlib import Path
        from test_compact import fixture
        from dataset import DatasetBundle, describe_shard, file_hash
        from common import DatasetFile
        from train import make_example
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            raw = root / 'data.rmd'
            fixture(raw)
            with DatasetFile(raw) as records:
                record = records[0]
            candidates = [dict(move=i, score=i * 100, completed_depth=3, nominal_depth_valid=True,
                               bound='Exact', source='AlphaBeta') for i in (100, 101)]
            value = comparison(dict(perspective='root-side-to-move', completed_depth=3, candidates=candidates), 100)
            sidecar = root / 'comparisons.sqlite'
            connection = sqlite3.connect(sidecar)
            connection.execute('CREATE TABLE comparisons(position TEXT PRIMARY KEY, payload TEXT NOT NULL)')
            connection.execute('INSERT INTO comparisons VALUES (?,?)', (record.position_key.hex(), json.dumps(value)))
            connection.commit()
            connection.close()
            bundle = root / 'dataset.json'
            bundle.write_text(json.dumps({'version': 2, 'shards': [describe_shard(raw, {}, 'fixture', compact=True)],
                                          'comparisons': {'path': str(sidecar), 'sha256': file_hash(sidecar)}}))
            with DatasetBundle(bundle) as dataset:
                self.assertEqual(dataset[0].comparison, value)
                example = make_example(dataset[0], 1, 'cpu')
                self.assertEqual(len(example[5][0]), 2)
            sidecar.write_bytes(b'corrupt')
            with self.assertRaisesRegex(ValueError, 'identity mismatch'):
                DatasetBundle(bundle)

    def test_declared_domain_is_preserved_and_unknown_leaf_rejected(self):
        metadata = dict(version=4, root_universe='all-legal', descendant_universe='production-radius-two',
                        leaf_policy='four-q6-immediate-v1', search_domain='distillation',
                        selectivity='candidate-domain-only-no-depth-pruning')
        analysis = dict(metadata, perspective='root-side-to-move', completed_depth=1,
                        candidates=[dict(move=i, score=i, bound='DomainExact', completed_depth=1,
                                         nominal_depth_valid=True, source='AlphaBeta') for i in range(2)])
        self.assertEqual(comparison(analysis, 1)['search_metadata']['search_domain'], 'distillation')
        analysis['leaf_policy'] = 'unknown'
        with self.assertRaisesRegex(ValueError, 'search domain'):
            comparison(analysis, 1)

    def test_comparison_is_candidate_order_invariant(self):
        metadata = dict(
            version=4,
            root_universe='all-legal',
            descendant_universe='production-radius-two',
            leaf_policy='four-q6-immediate-v1',
            search_domain='distillation',
            selectivity='candidate-domain-only-no-depth-pruning',
            perspective='root-side-to-move',
            completed_depth=3,
        )
        candidates = [
            dict(move=100, score=300, completed_depth=3,
                nominal_depth_valid=True, bound='DomainExact',
                source='AlphaBeta'),
            dict(move=20, score=100, completed_depth=3,
                nominal_depth_valid=True, bound='DomainExact',
                source='AlphaBeta'),
            dict(move=50, score=200, completed_depth=3,
                nominal_depth_valid=True, bound='DomainExact',
                source='AlphaBeta'),
        ]

        import itertools
        import math
        from teacher import validate_comparison

        expected = comparison(dict(metadata, candidates=candidates), 1000)
        self.assertEqual(expected['moves'], [20, 50, 100])
        self.assertEqual(expected['scores'], [100, 200, 300])
        weights = [math.exp(-.2), math.exp(-.1), 1.0]
        for probability, weight in zip(expected['probabilities'], weights, strict=True):
            self.assertAlmostEqual(probability, weight / sum(weights))
        for order in itertools.permutations(candidates):
            actual = comparison(dict(metadata, candidates=order), 1000)
            self.assertEqual(validate_comparison(actual), expected)
            self.assertEqual(explore(actual, 123), explore(expected, 123))

class SidecarConflictTests(unittest.TestCase):
    @staticmethod
    def analysis(key, depth=3, scores=(100, 200), moves=(100, 101)):
        return dict(version=4, position_key=key, perspective='root-side-to-move',
                    root_universe='all-legal', descendant_universe='production-radius-two',
                    leaf_policy='four-q6-immediate-v1', search_domain='distillation',
                    selectivity='candidate-domain-only-no-depth-pruning', completed_depth=depth,
                    candidates=[dict(move=at, score=score, completed_depth=depth,
                        nominal_depth_valid=True, bound='DomainExact', source='AlphaBeta')
                        for at,score in zip(moves,scores,strict=True)])

    @staticmethod
    def write_rows(path, rows):
        import json
        path.write_text(''.join(json.dumps(row)+'\n' for row in rows), encoding='utf-8')
        return path

    def test_identical_payload_is_deduplicated(self):
        import json
        import sqlite3
        import tempfile
        from pathlib import Path
        from teacher import build_sidecar
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); key=bytes(58).hex(); row=self.analysis(key)
            source=self.write_rows(root/'rows.jsonl',[row,row,row])
            output=root/'comparisons.sqlite'
            metadata=build_sidecar([source],output,1000)
            self.assertEqual(metadata['conflicted_positions'],0)
            with closing(sqlite3.connect(output)) as connection:
                rows=connection.execute('SELECT position,payload FROM comparisons').fetchall()
                self.assertEqual(len(rows),1)
                self.assertEqual(rows[0][0],key)
                self.assertEqual(json.loads(rows[0][1]),comparison(row,1000))
                self.assertEqual(connection.execute('SELECT count(*) FROM conflicts').fetchone()[0],0)

    def test_conflict_is_permanent_across_files_and_bundle_keeps_hard_labels(self):
        import dataclasses
        import json
        import sqlite3
        import tempfile
        from pathlib import Path
        from common import DatasetFile
        from dataset import DatasetBundle, describe_shard
        from teacher import build_sidecar
        from test_compact import fixture
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); raw=root/'data.rmd';fixture(raw,games=1,plies=3)
            raw_before=raw.read_bytes()
            with DatasetFile(raw) as records:
                conflicted,unrelated=records[1].position_key.hex(),records[2].position_key.hex()
            first=self.analysis(conflicted)
            second=self.analysis(conflicted,depth=4,scores=(150,250))
            other=self.analysis(unrelated)
            paths=[self.write_rows(root/'first.jsonl',[first,first,other]),
                   self.write_rows(root/'second.jsonl',[second,other]),
                   self.write_rows(root/'later.jsonl',[first,second,first,other])]
            metadata=build_sidecar(paths,root/'comparisons.sqlite',1000)
            self.assertEqual(metadata['conflicted_positions'],1)
            with closing(sqlite3.connect(metadata['path'])) as connection:
                self.assertEqual(connection.execute('SELECT position FROM conflicts').fetchall(),[(conflicted,)])
                self.assertEqual(connection.execute('SELECT position FROM comparisons').fetchall(),[(unrelated,)])
            descriptor={'version':2,'shards':[describe_shard(raw,{},'fixture',compact=True)]}
            baseline=root/'baseline.json'; baseline.write_text(json.dumps(descriptor))
            attached=root/'dataset.json'; attached.write_text(json.dumps({**descriptor,'comparisons':metadata}))
            with DatasetBundle(baseline) as before, DatasetBundle(attached) as after:
                self.assertIsNone(after[1].comparison)
                self.assertEqual(after[1],before[1])
                self.assertEqual(after[2].comparison,comparison(other,1000))
                self.assertEqual(dataclasses.replace(after[2],comparison=None),before[2])
            self.assertEqual(raw.read_bytes(),raw_before)

    def test_invalid_rows_are_not_hidden_by_a_conflict(self):
        import tempfile
        from pathlib import Path
        from teacher import build_sidecar
        key=bytearray(58);key[0]=64;key[-1]=1;key=bytes(key).hex()
        first=self.analysis(key);second=self.analysis(key,depth=4,scores=(150,250))
        invalids=[(self.analysis(key,moves=(0,101)),'legal empty'),
                  (self.analysis(key,moves=(100,100)),'duplicate candidate'),
                  (self.analysis(key,depth=256),'invalid teacher comparison'),
                  ({**first,'leaf_policy':'bad'},'search domain'),
                  ({**first,'position_key':'00','candidates':[]},'key length'),
                  ({**first,'position_key':key[:-2]+'02'},'invalid side')]
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            for index,(bad,message) in enumerate(invalids):
                with self.subTest(message=message):
                    source=self.write_rows(root/f'{index}.jsonl',[first,second,bad])
                    output=root/f'{index}.sqlite'
                    with self.assertRaisesRegex(ValueError,message):build_sidecar([source],output,1000)
                    self.assertFalse(output.exists())
