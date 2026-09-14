import json
import tempfile
import unittest
from pathlib import Path
from common import DATA_HEADER, DATA_RECORD_PREFIX, DatasetFile, make_split_manifest, validate_split_manifest
from compose import compose
from dataset import DatasetBundle, describe_shard
from test_compact import fixture

class CompositionTests(unittest.TestCase):
    def test_empirical_weight_dedupe_lineage_and_idempotent_resume(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);raw=root/'raw.rmd';fixture(raw)
            source=root/'source.json';shard=describe_shard(raw,{'fixture':True},'synthetic',compact=True)
            source.write_text(json.dumps({'version':2,'shards':[shard]}))
            config=root/'config.json';config.write_text(json.dumps({'version':1,'seed':7,'sources':[{'kind':'opening','path':'source.json','weight':.5}]}))
            output=compose(config,root/'out');before=output.read_bytes()
            with DatasetBundle(output) as data:
                self.assertTrue(all(not record.exact for record in data))
                self.assertEqual(data[0].sample_weight,.5)
                split=validate_split_manifest(data,make_split_manifest(data,7),7)
                train=[data[i] for i in split['train']]
                self.assertEqual(len(train),len({r.position_key for r in train}))
                owners={}
                for name,indices in split.items():
                    for i in indices:
                        lineage=data[i].lineage_id
                        self.assertEqual(owners.setdefault(lineage,name),name)
            compose(config,root/'out');self.assertEqual(output.read_bytes(),before)
            changed=json.loads(config.read_text());changed['sources'][0]['weight']=2
            config.write_text(json.dumps(changed))
            with self.assertRaises(ValueError):compose(config,root/'out')
            self.assertEqual(output.read_bytes(),before)

    def test_synthetic_proof_provenance_retained_and_cannot_be_opening(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);raw=root/'raw.rmd';fixture(raw,games=3,plies=2)
            with DatasetFile(raw) as data:records=list(data)
            payload=bytearray(DATA_HEADER.pack(b'RMDATA01',1,0,len(records)))
            for r in records:
                payload.extend(DATA_RECORD_PREFIX.pack(r.game_id,r.ply,0,255,r.value,7,1));payload.extend(r.position_key)
            raw.write_bytes(payload)
            shard=describe_shard(raw,{'source':'independently-verified-book','book_sha256':'synthetic'},'synthetic',compact=True)
            for meta in shard['games'].values():meta['proof']={'lineage_id':meta['lineage_id'],'fixture':True}
            source=root/'source.json';source.write_text(json.dumps({'version':2,'shards':[shard]}))
            config=root/'config.json';settings={'version':1,'sources':[{'kind':'verified-proof','path':'source.json','keep':.5,'weight':.25}]}
            config.write_text(json.dumps(settings));output=compose(config,root/'out')
            with DatasetBundle(output) as data:
                self.assertTrue(all(r.exact and r.source==7 for r in data))
                self.assertEqual(data[0].sample_keep,.5)
                self.assertEqual(data.descriptor['shards'][0]['games']['0']['proof'],shard['games']['0']['proof'])
            settings['sources'][0].update(kind='opening',keep=1);config.write_text(json.dumps(settings))
            with self.assertRaisesRegex(ValueError,'empirical'):compose(config,root/'bad')
            self.assertFalse((root/'bad/dataset.json').exists())

if __name__=='__main__':unittest.main()
