"""Model export and validation receipts. No receipt is a strength claim."""
import hashlib
import json
from pathlib import Path

from manifest import read_manifest, save_manifest
from dataset import file_hash, open_dataset

SCORE_CONTRACT = 'ordinary-limit-normalized-exact-sign-v1'
ARCHITECTURE = 'local-pattern-linear-v1'


def object_hash(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()).hexdigest()


def sidecar(model, kind):
    return Path(str(model) + f'.{kind}.json')


def file_identity(path):
    path = Path(path).resolve()
    return {'path': str(path), 'sha256': file_hash(path)}


def check_file(identity):
    if file_hash(identity['path']) != identity['sha256']:
        raise ValueError(f'evidence input changed: {identity["path"]}')
    return Path(identity['path'])


def export_identity(checkpoint_path, dataset_path):
    from checkpoint import load_checkpoint
    from common import validate_split_manifest
    checkpoint_id = file_identity(checkpoint_path)
    checkpoint = load_checkpoint(checkpoint_path)
    config = checkpoint.get('configuration', {})
    if config.get('architecture') != ARCHITECTURE or config.get('value_contract') != SCORE_CONTRACT:
        raise ValueError('checkpoint architecture/score contract is not supported for export evidence')
    split = checkpoint['split_manifest']
    dataset_id = file_identity(dataset_path)
    with open_dataset(Path(dataset_path)) as dataset:
        validate_split_manifest(dataset, split)
        shards = []
        if hasattr(dataset, 'descriptor'):
            for shard in dataset.descriptor['shards']:
                shards.append(file_identity(Path(dataset_path).resolve().parent / shard['path']))
    check_file(checkpoint_id)
    check_file(dataset_id)
    return {'checkpoint': checkpoint_id, 'dataset': {**dataset_id, 'fingerprint': split['dataset_sha256'],
            'shards': shards}, 'split': {'sha256': object_hash(split), 'manifest': split},
            'architecture': {'format_version': 1, 'architecture_id': 1, 'name': ARCHITECTURE},
            'score_contract': SCORE_CONTRACT}


def write_export(model, identity, quantization):
    receipt = {'schema': 1, 'kind': 'export', 'model': file_identity(model), **identity,
               'quantization': quantization, 'producer': file_identity(Path(__file__).with_name('export.py')),
               'required_checks': ['calibration', 'integer']}
    save_manifest(sidecar(model, 'export'), receipt)
    return receipt


def read_export(model):
    receipt = read_manifest(sidecar(model, 'export'))
    if receipt.get('schema') != 1 or receipt.get('kind') != 'export':
        raise ValueError('missing supported export evidence')
    if receipt['model']['sha256'] != file_hash(model):
        raise ValueError('model/export hash mismatch')
    if receipt['score_contract'] != SCORE_CONTRACT or receipt['architecture']['name'] != ARCHITECTURE:
        raise ValueError('unsupported export architecture/score contract')
    if object_hash(receipt['split']['manifest']) != receipt['split']['sha256']:
        raise ValueError('split evidence hash mismatch')
    return receipt


def write_check(model, kind, report, **details):
    receipt = read_export(model)
    check = {'schema': 1, 'kind': kind, 'status': 'passed',
             'model_sha256': receipt['model']['sha256'],
             'export_sha256': file_hash(sidecar(model, 'export')),
             'report': report, **details}
    save_manifest(sidecar(model, kind), check)
    seal(model)


def seal(model):
    paths = [sidecar(model, kind) for kind in ('calibration', 'integer')]
    if not all(path.exists() for path in paths):
        return
    export = read_export(model)
    export_hash = file_hash(sidecar(model, 'export'))
    for kind, path in zip(('calibration', 'integer'), paths, strict=True):
        check = read_manifest(path)
        if (check.get('schema') != 1 or check.get('kind') != kind or check.get('status') != 'passed'
                or check.get('model_sha256') != export['model']['sha256']
                or check.get('export_sha256') != export_hash):
            raise ValueError('validation receipt does not describe the exported model')
    save_manifest(sidecar(model, 'evidence'), {'schema': 1, 'kind': 'model-evidence',
        'model_sha256': export['model']['sha256'], 'export': file_identity(sidecar(model, 'export')),
        'checks': {kind: file_identity(sidecar(model, kind)) for kind in ('calibration', 'integer')}})


def validate_evidence(model, supplied_dataset=None):
    """Re-read the checkpoint-bound corpus; a caller cannot substitute training data."""
    evidence_path = sidecar(model, 'evidence')
    evidence = read_manifest(evidence_path)
    if evidence.get('schema') != 1 or evidence.get('kind') != 'model-evidence':
        raise ValueError('missing supported completed model evidence')
    model_hash = file_hash(model)
    if evidence['model_sha256'] != model_hash:
        raise ValueError('completed evidence belongs to another model')
    export_path = check_file(evidence['export'])
    if file_hash(sidecar(model, 'export')) != evidence['export']['sha256']:
        raise ValueError('local export receipt differs from bound evidence')
    export = read_export(model)
    inputs = {str(evidence_path.resolve()): file_hash(evidence_path), str(export_path): file_hash(export_path),
              str(Path(model).resolve()): model_hash}
    for name in ('checkpoint', 'dataset', 'producer'):
        identity = export[name]
        path = check_file(identity)
        inputs[str(path)] = identity['sha256']
    for identity in export['dataset']['shards']:
        inputs[str(check_file(identity))] = identity['sha256']
    dataset_path = Path(export['dataset']['path'])
    if supplied_dataset is not None and file_hash(supplied_dataset) != export['dataset']['sha256']:
        raise ValueError('supplied training corpus does not belong to the candidate model')
    current = export_identity(Path(export['checkpoint']['path']), dataset_path)
    for key in ('checkpoint', 'dataset', 'split', 'architecture', 'score_contract'):
        if current[key] != export[key]:
            raise ValueError(f'export provenance changed: {key}')
    for kind in ('calibration', 'integer'):
        identity = evidence['checks'][kind]
        check_path = check_file(identity)
        if file_hash(sidecar(model, kind)) != identity['sha256']:
            raise ValueError('local validation receipt differs from bound evidence')
        check = read_manifest(check_path)
        if (check.get('schema') != 1 or check.get('kind') != kind or check.get('status') != 'passed'
                or check.get('model_sha256') != model_hash
                or check.get('export_sha256') != evidence['export']['sha256']):
            raise ValueError('check/model/export identity mismatch')
        inputs[str(check_path)] = identity['sha256']
        producer = check_file(check['producer'])
        inputs[str(producer)] = check['producer']['sha256']
        if kind == 'calibration':
            if (check['checkpoint_sha256'] != export['checkpoint']['sha256']
                    or check['dataset_fingerprint'] != export['dataset']['fingerprint']
                    or check['split_sha256'] != export['split']['sha256']):
                raise ValueError('calibration used different training provenance')
            report, gates = check['report'], check['gates']
            if (report['samples'] < 1 or report['value_max_absolute_error'] > gates['max_value_error']
                    or report['policy_max_absolute_error'] > gates['max_policy_error']
                    or report['value_sign_errors_margin_001'] != 0):
                raise ValueError('calibration did not pass its declared gates')
        else:
            if check['report'].get('checks') != 15:
                raise ValueError('integer differential fixture coverage incomplete')
            inputs[str(check_file(check['engine']))] = check['engine']['sha256']
    return {'export': export, 'evidence': evidence, 'inputs_sha256': inputs, 'dataset_path': dataset_path}
