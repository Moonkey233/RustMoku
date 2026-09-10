"""Compare float and integer outputs; cross-language equality is separate."""

import argparse
import json
from pathlib import Path

import torch

from checkpoint import load_checkpoint

from common import (EVALUATION_LIMIT, POLICY_OUTPUT_SCALE, decode_position_key,
                    feature_keys, legal_policy_features, load_training_model,
                    read_quantized_model, validate_split_manifest)
from dataset import open_dataset


def calibration(model, quantized, records):
    errors, relative, values, policies = [], [], [], []
    saturated = sign_errors = 0
    with torch.no_grad():
        for record in records:
            board, side = decode_position_key(record.position_key)
            keys = torch.tensor(feature_keys(board, side), dtype=torch.long)
            reference = float(model.value(keys).item())
            actual = quantized.value(board, side) / EVALUATION_LIMIT
            # Compare against the declared clamped float score contract.
            reference = max(-1.0, min(1.0, reference))
            errors.append(abs(actual - reference))
            relative.append(abs(actual - reference) / max(abs(reference), .01))
            saturated += int(abs(actual) >= 1)
            sign_errors += int(abs(reference) > .01 and actual * reference <= 0)
            values.append((reference, actual))
            moves, candidates = legal_policy_features(board, side)
            if moves:
                logits = model.policy(torch.tensor(candidates, dtype=torch.long)).tolist()
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
    torch.set_num_threads(1)
    checkpoint = load_checkpoint(args.checkpoint)
    model = load_training_model(args.checkpoint, 'cpu')
    quantized = read_quantized_model(args.model)
    with open_dataset(args.dataset) as dataset:
        indices = validate_split_manifest(dataset, checkpoint.get('split_manifest'))['validation'][:args.samples]
        report = calibration(model, quantized, [dataset[i] for i in indices])
    print(json.dumps(report, indent=2))
    if (report['value_max_absolute_error'] > args.max_value_error
            or report['policy_max_absolute_error'] > args.max_policy_error
            or report['value_sign_errors_margin_001']):
        raise ValueError('quantization calibration gate failed')


if __name__ == '__main__':
    main()
