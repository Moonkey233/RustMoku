"""Small deterministic regressions; no training or match runs at import time."""

import dataclasses
import tempfile
import unittest
from pathlib import Path

from common import (
    DATA_HEADER, DATA_MAGIC, DATA_RECORD_PREFIX, DataRecord, EVALUATION_LIMIT, eligible_label, make_split_manifest,
    save_split_manifest, split_indices, validate_split_manifest,
)
from dataset import DatasetBundle, describe_shard
from export import calibrated_divisor
from train import make_example


def record(game, marker, source=2):
    key = bytes([marker]) + bytes(57)
    return DataRecord(game, 1, 0, None, 100, source, False, key)


class DataContracts(unittest.TestCase):
    def test_lineage_descendants_and_opening_holdout(self):
        data = [dataclasses.replace(record(i, i), lineage_id=f'root-{i}', opening_family=f'family-{i}')
                for i in range(10)]
        data[3] = dataclasses.replace(data[3], lineage_id='root-2')
        for seed in range(10):
            splits = split_indices(data, seed)
            membership = {i: name for name, indices in splits.items() for i in indices}
            self.assertEqual(membership[2], membership[3])
            self.assertEqual(membership[9], 'opening_heldout')

    def test_bundle_namespaces_shards_and_checks_content(self):
        import json
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shards = []
            for marker in (1, 2):
                path = root / f'{marker}.rmd'
                r = record(0, marker)
                path.write_bytes(DATA_HEADER.pack(DATA_MAGIC, 1, 0, 1) +
                                 DATA_RECORD_PREFIX.pack(0, 1, 0, 255, 100, 2, 0) + r.position_key)
                shards.append(describe_shard(path, {'status': 'unknown'}, str(marker)))
            descriptor = root / 'dataset.json'
            descriptor.write_text(json.dumps({'version': 1, 'shards': shards}), encoding='utf-8')
            with DatasetBundle(descriptor) as data:
                self.assertNotEqual(data[0].game_id, data[1].game_id)
                self.assertIsNone(data[0].completed_depth)
                self.assertEqual(sum(map(len, split_indices(data, 7).values())), 2)
            shards[1]['games']['0']['identity_content'][0][0] = 2
            descriptor.write_text(json.dumps({'version': 1, 'shards': shards}), encoding='utf-8')
            with self.assertRaisesRegex(ValueError, 'content mismatch'):
                DatasetBundle(descriptor)

    def test_duplicate_trajectories_and_relabels_stay_together(self):
        data = [record(i, i // 2) for i in range(20)]
        data[1] = dataclasses.replace(data[1], value=200, policy_move=5)
        for seed in range(10):
            splits = split_indices(data, seed)
            membership = {i: name for name, indices in splits.items() for i in indices}
            for i in range(0, 20, 2):
                self.assertEqual(membership[i], membership[i + 1])

    def test_checkpoint_split_cannot_be_redefined_by_evaluate_seed(self):
        data = [record(i, i) for i in range(20)]
        manifest = make_split_manifest(data, 7)
        self.assertEqual(validate_split_manifest(data, manifest), manifest['indices'])
        with self.assertRaisesRegex(ValueError, 'seed conflicts'):
            validate_split_manifest(data, manifest, 1)
        with self.assertRaisesRegex(ValueError, 'fingerprint'):
            validate_split_manifest(data + [record(20, 20)], manifest)
        with self.assertRaisesRegex(ValueError, 'manifest'):
            validate_split_manifest(data, None)

    def test_shard_game_id_collision_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'game_id/ply'):
            split_indices([record(0, 1), record(0, 2)], 7)

    def test_manifest_is_immutable(self):
        data = [record(i, i) for i in range(20)]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'split.json'
            save_split_manifest(path, make_split_manifest(data, 7))
            save_split_manifest(path, make_split_manifest(data, 7))
            with self.assertRaisesRegex(ValueError, 'immutable'):
                save_split_manifest(path, make_split_manifest(data, 1))

    def test_fallback_and_analysis_are_not_supervision(self):
        for source in (0, 1):
            r = record(0, 0, source)
            self.assertFalse(eligible_label(r))
            with self.assertRaisesRegex(ValueError, 'supervision'):
                make_example(r, 0, 'cpu')
        r = dataclasses.replace(record(0, 0), completed_depth=2, termination=1)
        self.assertTrue(eligible_label(r))
        self.assertFalse(eligible_label(dataclasses.replace(r, completed_depth=0)))

    def test_v1_low_scale_gain_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'gain error'):
            calibrated_divisor(1024 * 1024, EVALUATION_LIMIT)
        divisor = calibrated_divisor(16384 * 16384, EVALUATION_LIMIT)
        self.assertLessEqual(abs(16384 ** 2 / divisor / EVALUATION_LIMIT - 1), .01)

    def test_float_to_integer_bias_units_sign_and_clamp(self):
        import array
        import torch
        from common import LocalPatternModel, QuantizedModel, MODEL_FEATURE_COUNT, MODEL_HIDDEN
        from calibrate import calibration
        model = LocalPatternModel()
        with torch.no_grad():
            for parameter in model.parameters():
                parameter.zero_()
        combined = 16384 ** 2
        integers = QuantizedModel(array.array('h', [0]) * (MODEL_FEATURE_COUNT * MODEL_HIDDEN),
                                  (0,) * MODEL_HIDDEN, (0,) * MODEL_HIDDEN, 0,
                                  calibrated_divisor(combined, EVALUATION_LIMIT), 65536)
        for bias in (-1.5, -.9, -.1, 0, .1, .9, 1.5):
            with torch.no_grad():
                model.value_head.bias.fill_(bias)
            quantized = dataclasses.replace(integers, value_bias=round(bias * combined))
            report = calibration(model, quantized, [record(0, 0)])
            self.assertLess(report['value_max_absolute_error'], .006)
            self.assertEqual(report['value_sign_errors_margin_001'], 0)


if __name__ == '__main__':
    unittest.main()
