import unittest
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
