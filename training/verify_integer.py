"""Executable Rust/Python differential over fixed legal records and candidates."""

import argparse
import subprocess
import tempfile
from pathlib import Path

from common import parse_game_record, parse_move, read_quantized_model
from provenance import read_export, write_check, file_identity, check_file


def verify(engine, model_path, target_backend_policy='portable'):
    exported = read_export(model_path)
    engine_identity = file_identity(engine)
    model = read_quantized_model(model_path)
    if target_backend_policy not in ('portable','avx2'): raise ValueError('unsupported target backend policy')
    v3 = exported['architecture']['architecture_id'] == 4
    info = None
    if v3:
        result = subprocess.run([str(engine.resolve()),'backend-info'],capture_output=True,text=True,check=True,timeout=10)
        info = dict(line.split('=',1) for line in result.stdout.splitlines())
        if info.get('avx2') not in ('true','false') or info.get('auto') != ('avx2' if info['avx2']=='true' else 'scalar'):
            raise ValueError('inconsistent backend availability report')
        if target_backend_policy=='avx2' and info['avx2']!='true': raise ValueError('target requires unavailable AVX2')
    backends = (['scalar','auto'] + (['avx2'] if target_backend_policy=='avx2' else [])) if v3 else [None]
    checks = 0
    with tempfile.TemporaryDirectory() as directory:
        record = Path(directory) / 'position.rmg'
        for moves in ('', 'H8', 'H8 I8', 'H8 I8 H9', 'A1 O15 A2 O14 B1 N15'):
            record.write_text(f'RustMoku 1\nrules=freestyle\nmoves={moves}\n', encoding='utf-8')
            board, side = parse_game_record(record)
            for at, backend in ((at, backend) for at in ('A15', 'O1', 'G7') for backend in backends):
                result = subprocess.run([str(engine.resolve()), 'model-check', '--model', str(model_path.resolve()),
                                         '--record', str(record), '--move', at] + (['--backend', backend] if backend else []),
                                        capture_output=True, text=True, check=True, timeout=10)
                actual = dict(line.split('=', 1) for line in result.stdout.splitlines())
                expected = {'value': str(model.value(board, side)),
                            'policy': str(model.policy(board, side, parse_move(at)))}
                if actual != expected:
                    raise ValueError(f'integer mismatch: {moves}, {at}: {actual} != {expected}')
                checks += 1
    print(f'integer_differential_checks={checks} passed')
    check_file(engine_identity)
    check_file(exported['model'])
    report = {'checks': checks}
    if v3:
        report.update(checks=30,backends=['scalar','auto'],architecture=exported['architecture'],
            target_backend_policy='portable',backend_evidence_version=1,cpu=info,
            exercised={'scalar':'scalar','auto':info['auto']})
    write_check(model_path, 'integer', report, engine=engine_identity,
                producer=file_identity(__file__), fixture='five-positions-three-candidates-v1')
    if v3 and target_backend_policy=='avx2':
        write_check(model_path,'simd-avx2',dict(checks=15,architecture=exported['architecture'],
            target_backend_policy='avx2',cpu=info,exercised='avx2'),engine=engine_identity,producer=file_identity(__file__))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--model', type=Path, required=True)
    parser.add_argument('--target-backend-policy',choices=('portable','avx2'),default='portable')
    args = parser.parse_args()
    verify(args.engine, args.model,args.target_backend_policy)


if __name__ == '__main__':
    main()
