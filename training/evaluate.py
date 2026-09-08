"""Evaluate a float checkpoint on a game-isolated canonical split."""

from __future__ import annotations

import argparse
from pathlib import Path

import torch

from common import EVALUATION_LIMIT, DatasetFile, load_training_model, split_indices
from train import make_example


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dataset", type=Path, required=True)
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--split", choices=("train", "validation", "test"), default="validation")
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--device", default="cpu")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    model = load_training_model(args.checkpoint, args.device)
    model.eval()
    absolute_error = 0.0
    value_count = 0
    policy_correct = 0
    policy_count = 0
    with DatasetFile(args.dataset) as dataset, torch.no_grad():
        indices = split_indices(dataset, args.seed)[args.split]
        if not indices:
            raise ValueError(f"split {args.split!r} is empty")
        for index in indices:
            # Validation/test stay in their stored canonical, unaugmented view.
            global_keys, candidates, target, value, _ = make_example(
                dataset[index], 0, args.device
            )
            prediction = float(model.value(global_keys.unsqueeze(0)).item())
            absolute_error += abs(prediction - value)
            value_count += 1
            if target >= 0:
                policy_correct += int(int(model.policy(candidates).argmax()) == target)
                policy_count += 1
    print(
        f"split={args.split} samples={value_count} "
        f"value_mae_score_units={absolute_error / value_count * EVALUATION_LIMIT:.3f} "
        f"policy_top1={policy_correct / policy_count:.4f} policy_samples={policy_count}"
        if policy_count
        else f"split={args.split} samples={value_count} "
        f"value_mae_score_units={absolute_error / value_count * EVALUATION_LIMIT:.3f} "
        "policy_top1=unavailable policy_samples=0"
    )


if __name__ == "__main__":
    main()
