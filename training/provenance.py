"""Model export and validation receipts. No receipt is a strength claim."""
import hashlib
import json
import math
from pathlib import Path

from manifest import read_manifest, save_manifest
from dataset import file_hash, open_dataset, publish_shard

SCORE_CONTRACT = 'ordinary-limit-normalized-exact-sign-v1'
ARCHITECTURE = 'local-pattern-linear-v1'

def supported(architecture, contract):
    return (architecture, contract) in ((ARCHITECTURE, SCORE_CONTRACT),
        ('local-pattern-relu-width8-v2', 'stm-rational-q15-v2'))


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


def export_identity(checkpoint_path, dataset_path, resolver=None):
    from checkpoint import load_checkpoint
    from common import validate_split_manifest
    checkpoint_id = file_identity(checkpoint_path)
    checkpoint = load_checkpoint(checkpoint_path)
    config = checkpoint.get('configuration', {})
    if not supported(config.get('architecture'), config.get('value_contract')):
        raise ValueError('checkpoint architecture/score contract is not supported for export evidence')
    split = checkpoint['split_manifest']
    dataset_id = file_identity(dataset_path)
    with open_dataset(Path(dataset_path), resolver=resolver) as dataset:
        validate_split_manifest(dataset, split)
        shards = []
        if hasattr(dataset, 'descriptor'):
            for shard in dataset.descriptor['shards']:
                source = str((Path(dataset_path).resolve().parent / shard['path']).resolve())
                shards.append(file_identity((resolver or {}).get(shard['path'], (resolver or {}).get(source, source))))
    check_file(checkpoint_id)
    check_file(dataset_id)
    return {'checkpoint': checkpoint_id, 'dataset': {**dataset_id, 'fingerprint': split['dataset_sha256'],
            'shards': shards, **({'resolver': resolver} if resolver else {})}, 'split': {'sha256': object_hash(split), 'manifest': split},
            'architecture': {'format_version': 1 if config['architecture'] == ARCHITECTURE else 2,
                             'architecture_id': 1 if config['architecture'] == ARCHITECTURE else 2,
                             'name': config['architecture']},
            'score_contract': config['value_contract']}


def snapshot_file(identity, root):
    import shutil
    import tempfile
    source = check_file(identity)
    root.mkdir(parents=True, exist_ok=True)
    destination = root / (identity['sha256'] + source.suffix)
    with tempfile.NamedTemporaryFile(dir=root, delete=False) as stream:
        temporary = Path(stream.name)
    try:
        shutil.copyfile(source, temporary)
        # Frozen cross-version players must remain executable on POSIX hosts.
        shutil.copymode(source, temporary)
        if file_hash(temporary) != identity['sha256']:
            raise ValueError('input changed while freezing snapshot')
        publish_shard(temporary, destination)
    finally:
        temporary.unlink(missing_ok=True)
    return file_identity(destination)


def freeze_identity(model, identity):
    root = Path(str(model) + '.inputs')
    identity = json.loads(json.dumps(identity))
    resolver = {}
    for item in identity['dataset']['shards']:
        frozen = snapshot_file(item, root)
        resolver[item['path']] = frozen['path']
        item.update(frozen)
    descriptor = Path(identity['dataset']['path'])
    if descriptor.suffix == '.json':
        description = read_manifest(descriptor)
        for shard, frozen in zip(description['shards'], identity['dataset']['shards'], strict=True):
            resolver[shard['path']] = frozen['path']
        companion = description.get('comparisons')
        if companion is not None:
            frozen = snapshot_file(companion, root)
            resolver[companion['path']] = frozen['path']
    identity['checkpoint'] = snapshot_file(identity['checkpoint'], root)
    identity['dataset'].update(snapshot_file(identity['dataset'], root))
    if resolver:
        identity['dataset']['resolver'] = resolver
    return identity


def write_export(model, identity, quantization):
    identity = freeze_identity(model, identity)
    root = Path(str(model) + '.inputs')
    receipt = {'schema': 1, 'kind': 'export', 'model': file_identity(model), **identity,
               'quantization': quantization, 'producer': snapshot_file(file_identity(Path(__file__).with_name('export.py')), root),
               'producers': [snapshot_file(file_identity(path), root) for path in sorted(Path(__file__).parent.glob('*.py'))],
               'required_checks': ['calibration', 'integer']}
    save_manifest(sidecar(model, 'export'), receipt)
    return receipt


def read_export(model):
    receipt = read_manifest(sidecar(model, 'export'))
    if receipt.get('schema') != 1 or receipt.get('kind') != 'export':
        raise ValueError('missing supported export evidence')
    if receipt['model']['sha256'] != file_hash(model):
        raise ValueError('model/export hash mismatch')
    if not supported(receipt['architecture']['name'], receipt['score_contract']):
        raise ValueError('unsupported export architecture/score contract')
    if object_hash(receipt['split']['manifest']) != receipt['split']['sha256']:
        raise ValueError('split evidence hash mismatch')
    return receipt


def write_check(model, kind, report, **details):
    receipt = read_export(model)
    for key in ('producer', 'engine'):
        if key in details:
            details[key] = snapshot_file(details[key], Path(str(model) + '.inputs'))
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
    for identity in export.get('producers', []):
        inputs[str(check_file(identity))] = identity['sha256']
    resolver = export['dataset'].get('resolver')
    current = export_identity(Path(export['checkpoint']['path']), dataset_path, resolver=resolver)
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
            if (any(not math.isfinite(gates[key]) or not 0 < gates[key] <= .05 for key in ('max_value_error', 'max_policy_error'))
                    or report['samples'] < 1 or report['value_max_absolute_error'] > gates['max_value_error']
                    or report['policy_max_absolute_error'] > gates['max_policy_error']
                    or report['value_sign_errors_margin_001'] != 0):
                raise ValueError('calibration did not pass its declared gates')
            if export['architecture']['architecture_id'] == 2:
                if any(report[key]['agreement'] is not None and (not math.isfinite(report[key]['agreement']) or report[key]['agreement'] < .99)
                       for key in ('value_ordering', 'policy_ordering', 'policy_top1')):
                    raise ValueError('V2 rank or top1 calibration failed')
        else:
            if check['report'].get('checks') != 15:
                raise ValueError('integer differential fixture coverage incomplete')
            inputs[str(check_file(check['engine']))] = check['engine']['sha256']
    return {'export': export, 'evidence': evidence, 'inputs_sha256': inputs, 'dataset_path': dataset_path, 'resolver': resolver}
