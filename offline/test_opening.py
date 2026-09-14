import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ENGINE=Path(__file__).resolve().parents[1]/('target/debug/rustmoku-book.exe' if os.name=='nt' else 'target/debug/rustmoku-book')

class OpeningCliTests(unittest.TestCase):
    def test_two_ply_resume_matches_build_and_unknown_merge_is_atomic(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);record=root/'root.rmg'
            record.write_text('RustMoku 1\nrules=freestyle\nmoves=H8 G8\n')
            common=['--record',str(record),'--engine-build','fixture','--max-plies','2','--top-k','1','--score-margin','0','--depth','1','--nodes','2000']
            def run(*args,ok=True):
                result=subprocess.run([str(ENGINE),*args],capture_output=True,text=True,timeout=30)
                self.assertEqual(result.returncode==0,ok,result.stderr)
                return result
            full=root/'full.db';resumed=root/'resume.db'
            run('build',*common,'--max-positions','4','--output',str(full))
            run('build',*common,'--max-positions','1','--output',str(resumed))
            run('resume',*common,'--max-positions','4','--output',str(resumed))
            self.assertEqual(full.read_bytes(),resumed.read_bytes())
            before=resumed.read_bytes()
            run('merge','--left',str(full),'--right',str(full),'--output',str(resumed),'--unknown','x',ok=False)
            self.assertEqual(resumed.read_bytes(),before)
            data_engine=ENGINE.with_name('rustmoku-data.exe' if os.name=='nt' else 'rustmoku-data')
            importer=Path(__file__).resolve().parents[1]/'training/import_opening.py'
            subprocess.run([sys.executable,'-X','utf8',str(importer),'--engine',str(data_engine),'--opening-db',str(full),'--output',str(root/'data'),'--max-positions','4'],check=True,timeout=30,capture_output=True)
            import json
            descriptor=json.loads((root/'data/dataset.json').read_text())
            self.assertEqual(descriptor['shards'][0]['teacher']['exact'],False)
            self.assertEqual(len({m['lineage_id'] for m in descriptor['shards'][0]['games'].values()}),1)

if __name__=='__main__':unittest.main()
