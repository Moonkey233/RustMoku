import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from common import DATA_HEADER, DATA_MAGIC, DATA_RECORD_PREFIX, save_split_manifest
from dataset import DatasetBundle, describe_shard, file_hash, publish_shard
from import_proof import base_shards, main


class ProofBundleImport(unittest.TestCase):
    def test_interrupted_before_shard_manifest_never_replaces_different_data(self):
        with tempfile.TemporaryDirectory() as directory:
            source, target = [Path(directory) / name for name in ('staged.partial', 'shard.rmd')]
            source.write_bytes(b'complete shard bytes')
            publish_shard(source, target)
            # Simulate interruption before the separate descriptor publication.
            publish_shard(source, target)
            replacement = Path(directory) / 'new.partial'
            replacement.write_bytes(b'different data')
            with self.assertRaisesRegex(ValueError, 'immutable shard'):
                publish_shard(replacement, target)
            self.assertEqual(target.read_bytes(), b'complete shard bytes')

    def test_relative_absolute_mixed_shards_rebase_across_spaced_directories(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            base = root / 'base with spaces'
            merged = root / 'different merged directory'
            base.mkdir()
            merged.mkdir()
            shards = []
            for index in range(2):
                path = base / f'shard {index}.rmd'
                key = bytearray(58)
                key[index] = 0x60
                path.write_bytes(DATA_HEADER.pack(DATA_MAGIC, 1, 0, 1)
                    + DATA_RECORD_PREFIX.pack(0, 2, 0, 100, 100, 2, 0) + key)
                shard = describe_shard(path, {'status': 'fixture'}, str(index))
                if index == 0:
                    shard['path'] = path.name
                shards.append(shard)
            descriptor = base / 'dataset.json'
            save_split_manifest(descriptor, {'version': 1, 'shards': shards})
            before = file_hash(descriptor)
            resolved = base_shards(descriptor)
            save_split_manifest(merged / 'dataset.json', {'version': 1, 'shards': resolved})
            with DatasetBundle(descriptor) as original, DatasetBundle(merged / 'dataset.json') as combined:
                self.assertEqual(list(original), list(combined))
            self.assertEqual(file_hash(descriptor), before)

    def test_failed_import_cannot_damage_prior_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            engine, book = root / 'engine', root / 'book'
            engine.write_bytes(b'fixture executable identity')
            book.write_bytes(b'fixture book identity')
            output = root / 'output'
            output.mkdir()
            (output / 'proof.rmd').write_bytes(b'old published proof')
            (output / 'dataset.json').write_bytes(b'old published dataset')
            args = ['import_proof', '--engine', str(engine), '--book', str(book), '--output', str(output)]
            with patch('sys.argv', args), patch('import_proof.subprocess.run', side_effect=RuntimeError('fixture fail')):
                with self.assertRaises(RuntimeError):
                    main()
            self.assertEqual((output / 'proof.rmd').read_bytes(), b'old published proof')
            self.assertEqual((output / 'dataset.json').read_bytes(), b'old published dataset')


if __name__ == '__main__':
    unittest.main()
