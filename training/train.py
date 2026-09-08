"""Train the RustMoku V0.12 local-pattern Value/Policy model."""

from __future__ import annotations

import argparse
import random
from pathlib import Path

import torch
import torch.nn.functional as functional

from common import (
    DatasetFile,
    LocalPatternModel,
    calibrated_value_target,
    decode_position_key,
    feature_keys,
    legal_policy_features,
    split_indices,
    transform_position,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dataset", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--epochs", type=int, default=10)
    parser.add_argument("--batch-size", type=int, default=32)
    parser.add_argument("--learning-rate", type=float, default=1e-3)
    parser.add_argument("--policy-weight", type=float, default=1.0)
    parser.add_argument("--exact-weight", type=float, default=4.0)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--device", default="cpu")
    return parser.parse_args()


def make_example(record, symmetry: int, device: str):
    board, side = decode_position_key(record.position_key)
    board, policy_move = transform_position(board, record.policy_move, symmetry)
    global_keys = torch.tensor(feature_keys(board, side), dtype=torch.long, device=device)
    moves, candidates = legal_policy_features(board, side)
    candidate_keys = torch.tensor(candidates, dtype=torch.long, device=device)
    target = -1
    if policy_move is not None:
        try:
            target = moves.index(policy_move)
        except ValueError as error:
            raise ValueError("policy target is not a legal empty cell") from error
    return (
        global_keys,
        candidate_keys,
        target,
        calibrated_value_target(record),
        record.exact,
    )


def main() -> None:
    args = parse_args()
    if args.epochs < 1 or args.batch_size < 1:
        raise ValueError("epochs and batch size must be positive")
    random.seed(args.seed)
    torch.manual_seed(args.seed)
    torch.use_deterministic_algorithms(True)
    model = LocalPatternModel().to(args.device)
    optimizer = torch.optim.AdamW(model.parameters(), lr=args.learning_rate)

    with DatasetFile(args.dataset) as dataset:
        splits = split_indices(dataset, args.seed)
        if not splits["train"]:
            raise ValueError("dataset has no training games")
        generator = torch.Generator().manual_seed(args.seed)
        for epoch in range(args.epochs):
            order = torch.randperm(len(splits["train"]), generator=generator).tolist()
            totals = {"value": 0.0, "policy": 0.0, "batches": 0}
            for start in range(0, len(order), args.batch_size):
                selected = order[start : start + args.batch_size]
                examples = []
                for local_index in selected:
                    record_index = splits["train"][local_index]
                    # D4 augmentation is deterministic and training-only.
                    symmetry = random.Random(
                        (args.seed << 32) ^ (epoch << 20) ^ record_index
                    ).randrange(8)
                    examples.append(make_example(dataset[record_index], symmetry, args.device))
                global_keys = torch.stack([example[0] for example in examples])
                targets = torch.tensor(
                    [example[3] for example in examples],
                    dtype=torch.float32,
                    device=args.device,
                )
                weights = torch.tensor(
                    [args.exact_weight if example[4] else 1.0 for example in examples],
                    dtype=torch.float32,
                    device=args.device,
                )
                predicted = model.value(global_keys)
                value_loss = (
                    functional.smooth_l1_loss(predicted, targets, reduction="none") * weights
                ).sum() / weights.sum()
                policy_losses = []
                for _, candidates, target, _, _ in examples:
                    if target >= 0:
                        logits = model.policy(candidates).unsqueeze(0)
                        policy_losses.append(
                            functional.cross_entropy(
                                logits,
                                torch.tensor([target], device=args.device),
                            )
                        )
                policy_loss = (
                    torch.stack(policy_losses).mean()
                    if policy_losses
                    else torch.zeros((), device=args.device)
                )
                loss = value_loss + args.policy_weight * policy_loss
                optimizer.zero_grad(set_to_none=True)
                loss.backward()
                optimizer.step()
                totals["value"] += value_loss.item()
                totals["policy"] += policy_loss.item()
                totals["batches"] += 1
            print(
                f"epoch={epoch + 1} "
                f"value_loss={totals['value'] / totals['batches']:.4f} "
                f"policy_loss={totals['policy'] / totals['batches']:.4f}"
            )
        args.output.parent.mkdir(parents=True, exist_ok=True)
        torch.save(
            {
                "format": "rustmoku-local-pattern-v1",
                "state_dict": model.state_dict(),
                "seed": args.seed,
                "split_game_counts": {
                    name: len({dataset[index].game_id for index in indices})
                    for name, indices in splits.items()
                },
            },
            args.output,
        )
        print(f"saved={args.output}")


if __name__ == "__main__":
    main()
