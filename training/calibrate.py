"""Compare float and integer outputs; cross-language equality is separate."""

import argparse
import json
import math
from pathlib import Path

import torch

from checkpoint import load_checkpoint

from common import (EVALUATION_LIMIT, POLICY_OUTPUT_SCALE, decode_position_key,
                    feature_keys, legal_policy_features, load_training_model,
                    read_quantized_model, validate_split_manifest)
from dataset import open_dataset
from provenance import read_export, write_check, file_identity, object_hash


def calibration(model, quantized, records):
    from nonlinear_model import NonlinearModel, QuantizedNonlinear, UNIT, CLIP
    nonlinear = isinstance(model, NonlinearModel)
    if nonlinear != isinstance(quantized, QuantizedNonlinear):
        raise ValueError('float/integer architecture mismatch')
    errors, relative, values, policies = [], [], [], []
    saturated = sign_errors = 0
    policy_pairs = policy_correct = top1_samples = top1_correct = 0
    with torch.no_grad():
        for record in records:
            board, side = decode_position_key(record.position_key)
            keys = torch.tensor(feature_keys(board, side), dtype=torch.long)
            reference = float(model.board_value(board, side).item()) if nonlinear else float(model.value(keys).item())
            actual = quantized.normalized(board, side) / UNIT if nonlinear else quantized.value(board, side) / EVALUATION_LIMIT
            # Compare against the declared clamped float score contract.
            limit = CLIP / UNIT if nonlinear else 1.0
            reference = max(-limit, min(limit, reference))
            errors.append(abs(actual - reference))
            relative.append(abs(actual - reference) / max(abs(reference), .01))
            saturated += int(abs(actual) >= 1)
            sign_errors += int(abs(reference) > .01 and actual * reference <= 0)
            values.append((reference, actual))
            moves, candidates = legal_policy_features(board, side)
            if moves:
                logits = model.policy(torch.tensor(candidates, dtype=torch.long)).tolist()
                integers = [quantized.policy(board, side, move) / POLICY_OUTPUT_SCALE for move in moves]
                ranking = sorted(range(len(moves)), key=lambda i: (-logits[i], moves[i]))
                if len(ranking) > 1 and logits[ranking[0]] - logits[ranking[1]] > .01:
                    top1_samples += 1
                    top1_correct += ranking[0] == max(range(len(moves)), key=lambda i: (integers[i], -moves[i]))
                for i in range(len(moves)):
                    for j in range(i + 1, len(moves)):
                        if abs(logits[i] - logits[j]) > .01:
                            policy_pairs += 1
                            policy_correct += (logits[i] - logits[j]) * (integers[i] - integers[j]) > 0
                for move, logit in zip(moves, logits, strict=True):
                    policies.append((max(-8, min(32767 / POLICY_OUTPUT_SCALE, logit)),
                                     quantized.policy(board, side, move) / POLICY_OUTPUT_SCALE))
    if not errors:
        raise ValueError('no eligible calibration records')
    def ordering(pairs):
        comparable = correct = 0
        for i, (left, quantized_left) in enumerate(pairs):
            for right, quantized_right in pairs[i + 1:]:
                if abs(left - right) > .01:
                    comparable += 1
                    correct += int((left - right) * (quantized_left - quantized_right) > 0)
        return {'comparable_pairs': comparable,
                'agreement': correct / comparable if comparable else None}
    return {'samples': len(errors), 'value_mae': sum(errors) / len(errors),
            'value_max_absolute_error': max(errors), 'value_max_relative_error_floor_001': max(relative),
            'value_saturation_rate': saturated / len(errors), 'value_sign_errors_margin_001': sign_errors,
            'value_ordering': ordering(values),
            'policy_ordering': {'comparable_pairs': policy_pairs, 'agreement': policy_correct / policy_pairs if policy_pairs else None},
            'policy_top1': {'comparable_positions': top1_samples, 'agreement': top1_correct / top1_samples if top1_samples else None},
            'policy_max_absolute_error': max((abs(a - b) for a, b in policies), default=0),
            'policy_saturation_rate': sum(b <= -8 or b >= 32767 / POLICY_OUTPUT_SCALE for _, b in policies) / max(1, len(policies))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dataset', type=Path, required=True)
    parser.add_argument('--checkpoint', type=Path, required=True)
    parser.add_argument('--model', type=Path, required=True)
    parser.add_argument('--samples', type=int, default=16)
    parser.add_argument('--max-value-error', type=float, default=.05)
    parser.add_argument('--max-policy-error', type=float, default=.05)
    args = parser.parse_args()
    if not 1 <= args.samples <= 256:
        raise ValueError('calibration samples must be 1..256')
    if any(not math.isfinite(value) or not 0 < value <= .05 for value in (args.max_value_error, args.max_policy_error)):
        raise ValueError('calibration error gates must be finite and no weaker than .05')
    torch.set_num_threads(1)
    checkpoint = load_checkpoint(args.checkpoint)
    exported = read_export(args.model)
    if file_identity(args.checkpoint)['sha256'] != exported['checkpoint']['sha256']:
        raise ValueError('calibration checkpoint differs from model export')
    if file_identity(args.dataset)['sha256'] != exported['dataset']['sha256']:
        raise ValueError('calibration dataset differs from model export')
    model = load_training_model(args.checkpoint, 'cpu')
    quantized = read_quantized_model(args.model)
    if hasattr(quantized, 'score_scale') and quantized.score_scale != checkpoint['configuration']['score_scale']:
        raise ValueError('V2 calibration score contract mismatch')
    with open_dataset(args.dataset) as dataset:
        indices = validate_split_manifest(dataset, checkpoint.get('split_manifest'))['validation'][:args.samples]
        report = calibration(model, quantized, [dataset[i] for i in indices])
    print(json.dumps(report, indent=2))
    if (report['value_max_absolute_error'] > args.max_value_error
            or report['policy_max_absolute_error'] > args.max_policy_error
            or report['value_sign_errors_margin_001']):
        raise ValueError('quantization calibration gate failed')
    if hasattr(quantized, 'score_scale') and any(report[key]['agreement'] is not None and report[key]['agreement'] < .99
                                              for key in ('value_ordering', 'policy_ordering', 'policy_top1')):
        raise ValueError('V2 quantization rank/top1 agreement below 99%; use QAT and re-export')
    write_check(args.model, 'calibration', report,
        checkpoint_sha256=exported['checkpoint']['sha256'],
        dataset_fingerprint=checkpoint['split_manifest']['dataset_sha256'],
        split_sha256=object_hash(checkpoint['split_manifest']),
        gates={'max_value_error': args.max_value_error, 'max_policy_error': args.max_policy_error},
        selection={'split': 'validation', 'indices': indices}, producer=file_identity(__file__))


if __name__ == '__main__':
    main()
