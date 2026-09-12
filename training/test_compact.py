import dataclasses
import json
import tempfile
import unittest
from pathlib import Path

from common import DATA_HEADER, DATA_MAGIC, DATA_RECORD_PREFIX, make_split_manifest, validate_split_manifest
from compact import RangeIndices, partitions
from dataset import DatasetBundle, describe_shard
from audit import audit


def fixture(path, games=12, plies=6):
    payload = bytearray(DATA_HEADER.pack(DATA_MAGIC, 1, 0, games * plies))
    for game in range(games):
        for ply in range(plies):
            key = bytearray(58)
            for at in range(ply):
                cell = (game * 11 + at) % 225
                key[cell // 4] |= (1 + at % 2) << ((3 - cell % 4) * 2)
            key[-1] = ply % 2
            payload.extend(DATA_RECORD_PREFIX.pack(game, ply, 0, 255, game * 100 + 25, 2, 0))
            payload.extend(key)
    path.write_bytes(payload)


class CompactTests(unittest.TestCase):
    def test_bundle_range_lookup_and_split_audit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            raw = root / 'data.rmd'
            fixture(raw)
            shard = describe_shard(raw, {}, 'fixture', compact=True)
            self.assertNotIn('identity_content', shard['games']['0'])
            descriptor = root / 'dataset.json'
            descriptor.write_text(json.dumps({'version': 2, 'shards': [shard]}))
            with DatasetBundle(descriptor) as data:
                self.assertEqual(len(data.ranges), 12)
                self.assertFalse(hasattr(data, 'rows'))
                self.assertEqual(data[-1], data[len(data)-1])
                self.assertEqual(data[6].game_id, 1)
                with self.assertRaises(IndexError): data[len(data)]
                manifest = make_split_manifest(data, 7)
                self.assertEqual(manifest['version'], 2)
                self.assertEqual(sum(map(len, partitions(manifest).values())), len(data))
                valid = validate_split_manifest(data, manifest)
                report = audit(data, manifest)
                self.assertEqual(report['records'], 72)
                self.assertEqual(sum(map(len, valid.values())), 72)
                altered = json.loads(json.dumps(manifest))
                altered['ranges']['train'][0][1] += 1
                with self.assertRaisesRegex(ValueError, 'modified'): validate_split_manifest(data, altered)
            shard['games']['1']['start'] += 1
            descriptor.write_text(json.dumps({'version': 2, 'shards': [shard]}))
            with self.assertRaisesRegex(ValueError, 'content mismatch'): DatasetBundle(descriptor)

    def test_ranges_reject_overlap_overflow_and_negative(self):
        for ranges in ([[0, 0]], [[-1, 2]], [[0, 3], [2, 1]], [[10_000_000, 1]], [[True, 1]]):
            with self.assertRaises(ValueError): RangeIndices(ranges)
        indices = RangeIndices([[2, 3], [9, 2]])
        self.assertEqual(list(indices), [2, 3, 4, 9, 10])
        self.assertEqual(indices[::-1], [10, 9, 4, 3, 2])

    def test_duplicate_trajectories_union_lineage_transitively(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            raw = root / 'data.rmd'
            fixture(raw)
            shard = describe_shard(raw, {}, 'fixture', compact=True)
            shard['games']['1']['lineage_id'] = shard['games']['2']['lineage_id']
            descriptor = root / 'dataset.json'
            descriptor.write_text(json.dumps({'version': 2, 'shards': [shard]}))
            with DatasetBundle(descriptor) as data:
                manifest = make_split_manifest(data, 3)
                names = {data[i].game_id: name for name, indices in partitions(manifest).items() for i in indices}
                self.assertEqual(names[1], names[2])


if __name__ == '__main__': unittest.main()
