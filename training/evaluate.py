"""Evaluate a float checkpoint on a game-isolated canonical split."""

from __future__ import annotations

import argparse
import json
from collections import defaultdict
from pathlib import Path

import torch

from checkpoint import load_checkpoint

from dataset import open_dataset

from common import EVALUATION_LIMIT, load_training_model, validate_split_manifest
from train import make_example


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dataset", type=Path, required=True)
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--split", choices=("train", "validation", "test", "opening_heldout"), default="validation")
    parser.add_argument("--seed", type=int, default=None)
    parser.add_argument("--device", default="cpu")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    checkpoint = load_checkpoint(args.checkpoint)
    model = load_training_model(args.checkpoint, args.device)
    model.eval()
    nonlinear = checkpoint.get('format') == 'rustmoku-nonlinear-v2'
    architecture = 'v2' if nonlinear else 'v1'
    score_scale = checkpoint['configuration'].get('score_scale')
    absolute_error = 0.0
    value_count = 0
    policy_correct = 0
    policy_count = 0
    strata = defaultdict(lambda: {"samples": 0, "value_absolute_error": 0.0, "saturated": 0,
                                  "policy_samples": 0, "top1": 0, "top3": 0, "top5": 0,
                                  "target_pairwise_agreement": 0.0, "outcome_samples": 0,
                                  "outcome_brier_linear_clamp": 0.0})
    with open_dataset(args.dataset) as dataset, torch.no_grad():
        indices = validate_split_manifest(
            dataset, checkpoint.get("split_manifest"), args.seed
        )[args.split]
        if not indices:
            raise ValueError(f"split {args.split!r} is empty")
        for index in indices:
            # Validation/test stay in their stored canonical, unaugmented view.
            global_keys, candidates, target, value, _, _ = make_example(
                dataset[index], 0, args.device, architecture, score_scale
            )
            prediction = float(model.value(global_keys.unsqueeze(0)).item())
            record = dataset[index]
            phase = "opening" if record.ply < 20 else ("middle" if record.ply < 80 else "late")
            groups = [strata["all"], strata[f"source-{record.source}"], strata[phase]]
            for group in groups:
                group["samples"] += 1
                group["value_absolute_error"] += abs(prediction - value)
                group["saturated"] += int(abs(prediction) >= 1)
                if record.outcome is not None:
                    expected_score = (max(-1, min(1, prediction)) + 1) / 2
                    group["outcome_samples"] += 1
                    group["outcome_brier_linear_clamp"] += (expected_score - (record.outcome + 1) / 2) ** 2
            absolute_error += abs(prediction - value)
            value_count += 1
            if target >= 0:
                logits = model.policy(candidates)
                ranking = torch.argsort(logits, descending=True, stable=True).tolist()
                rank = ranking.index(target)
                policy_correct += int(rank == 0)
                for group in groups:
                    group["policy_samples"] += 1
                    group["top1"] += int(rank < 1)
                    group["top3"] += int(rank < 3)
                    group["top5"] += int(rank < 5)
                    group["target_pairwise_agreement"] += (len(ranking) - 1 - rank) / max(1, len(ranking) - 1)
                policy_count += 1
    print(
        f"split={args.split} samples={value_count} "
        f"value_mae_normalized={absolute_error / value_count:.6f} "
        f"policy_top1={policy_correct / policy_count:.4f} policy_samples={policy_count}"
        if policy_count
        else f"split={args.split} samples={value_count} "
        f"value_mae_normalized={absolute_error / value_count:.6f} "
        "policy_top1=unavailable policy_samples=0"
    )

    for group in strata.values():
        group["value_mae"] = group.pop("value_absolute_error") / group["samples"]
        group["saturation_rate"] = group.pop("saturated") / group["samples"]
        for metric in ("top1", "top3", "top5", "target_pairwise_agreement"):
            group[metric] = group[metric] / group["policy_samples"] if group["policy_samples"] else None
        group["outcome_brier_linear_clamp"] = (group["outcome_brier_linear_clamp"] / group["outcome_samples"]
                                                   if group["outcome_samples"] else None)
    print(json.dumps({"split": args.split, "strata": dict(strata)}, indent=2))


if __name__ == "__main__":
    main()
