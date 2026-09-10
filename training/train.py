"""Train the RustMoku V0.12 local-pattern Value/Policy model."""

from __future__ import annotations

import argparse
import random
import hashlib
import json
import time
import math
from pathlib import Path

import torch
import torch.nn.functional as functional

from dataset import open_dataset
from checkpoint import atomic_save, load_checkpoint

from common import (
    LocalPatternModel,
    calibrated_value_target,
    decode_position_key,
    feature_keys,
    legal_policy_features,
    make_split_manifest,
    validate_split_manifest,
    save_split_manifest,
    eligible_label,
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
    parser.add_argument("--resume", type=Path)
    parser.add_argument("--max-steps", type=int, help="bounded interruption after this absolute step")
    parser.add_argument("--checkpoint-every", type=int, default=50)
    parser.add_argument("--torch-threads", type=int, default=1)
    parser.add_argument("--max-proof-fraction", type=float, default=.25)
    return parser.parse_args()


def make_example(record, symmetry: int, device: str):
    if not eligible_label(record):
        raise ValueError("fallback/analysis labels cannot be used as supervision")
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


def train(args):
    if args.epochs < 1 or args.batch_size < 1 or args.checkpoint_every < 1 or args.torch_threads < 1:
        raise ValueError("epochs and batch size must be positive")
    if (not all(math.isfinite(x) for x in (args.learning_rate, args.policy_weight, args.exact_weight))
            or not 0 < args.learning_rate < 1 or args.policy_weight < 0 or args.exact_weight <= 0):
        raise ValueError("invalid training loss/optimizer configuration")
    if args.max_steps is not None and args.max_steps < 1:
        raise ValueError("max-steps must be positive")
    if not 0 <= args.max_proof_fraction < 1:
        raise ValueError("max-proof-fraction must be in [0,1)")
    torch.set_num_threads(args.torch_threads)
    configuration = {key: getattr(args, key) for key in (
        "epochs", "batch_size", "learning_rate", "policy_weight", "exact_weight", "seed", "device", "torch_threads", "max_proof_fraction")}
    configuration["torch_version"] = str(torch.__version__)
    configuration["architecture"] = "local-pattern-linear-v1"
    configuration["value_contract"] = "ordinary-limit-normalized-exact-sign-v1"
    configuration_hash = hashlib.sha256(json.dumps(configuration, sort_keys=True).encode()).hexdigest()
    resumed = load_checkpoint(args.resume, args.device) if args.resume else None
    if resumed is not None and (resumed.get("checkpoint_version") != 2 or resumed.get("configuration") != configuration
                                or resumed.get("configuration_sha256") != configuration_hash):
        raise ValueError("resume configuration/version mismatch")
    if args.output.exists() and args.resume is None:
        raise ValueError("output checkpoint exists; use an explicit resume or new output")
    started = time.perf_counter()
    random.seed(args.seed)
    torch.manual_seed(args.seed)
    torch.use_deterministic_algorithms(True)
    model = LocalPatternModel().to(args.device)
    optimizer = torch.optim.AdamW(model.parameters(), lr=args.learning_rate)

    with open_dataset(args.dataset) as dataset:
        manifest = resumed["split_manifest"] if resumed else make_split_manifest(dataset, args.seed)
        splits = validate_split_manifest(dataset, manifest)
        ordinary = [i for i in splits["train"] if dataset[i].source != 7]
        proof = [i for i in splits["train"] if dataset[i].source == 7]
        random.Random(args.seed).shuffle(proof)
        cap = int(len(ordinary) * args.max_proof_fraction / (1 - args.max_proof_fraction))
        splits["train"] = sorted(ordinary + proof[:cap])
        args.output.parent.mkdir(parents=True, exist_ok=True)
        save_split_manifest(args.output.with_suffix(".split.json"), manifest)
        if not splits["train"]:
            raise ValueError("dataset has no training games")
        generator = torch.Generator().manual_seed(args.seed)
        epoch = cursor = step = 0
        order = None
        if resumed:
            model.load_state_dict(resumed["state_dict"])
            optimizer.load_state_dict(resumed["optimizer"])
            epoch, cursor, step = resumed["epoch"], resumed["cursor"], resumed["step"]
            order = resumed["order"]
            if (type(epoch) is not int or not 0 <= epoch <= args.epochs
                    or type(cursor) is not int or not 0 <= cursor < len(splits["train"])
                    or type(step) is not int or step < 0
                    or (order is None and cursor != 0)
                    or (order is not None and sorted(order) != list(range(len(splits["train"]))))):
                raise ValueError("invalid checkpoint data cursor")
            generator.set_state(resumed["shuffle_rng"].cpu())
            random.setstate(resumed["python_rng"])
            torch.set_rng_state(resumed["torch_rng"].cpu())
            if args.device.startswith("cuda"):
                torch.cuda.set_rng_state_all(resumed["cuda_rng"])

        def save():
            checkpoint = {
                "format": "rustmoku-local-pattern-v1", "checkpoint_version": 2,
                "state_dict": model.state_dict(), "optimizer": optimizer.state_dict(),
                "scheduler": None, "epoch": epoch, "cursor": cursor, "step": step, "order": order,
                "seed": args.seed, "split_manifest": manifest,
                "configuration": configuration, "configuration_sha256": configuration_hash,
                "python_rng": random.getstate(), "torch_rng": torch.get_rng_state(),
                "shuffle_rng": generator.get_state(),
                "cuda_rng": torch.cuda.get_rng_state_all() if args.device.startswith("cuda") else [],
            }
            atomic_save(args.output, checkpoint)
            return checkpoint

        while epoch < args.epochs:
            if args.max_steps is not None and step >= args.max_steps:
                return save()
            if order is None:
                order = torch.randperm(len(splits["train"]), generator=generator).tolist()
            totals = {"value": 0.0, "policy": 0.0, "batches": 0}
            for start in range(cursor, len(order), args.batch_size):
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
                step += 1
                cursor = start + len(selected)
                finished_epoch = cursor == len(order)
                if finished_epoch:
                    epoch += 1
                    cursor, order = 0, None
                if step % args.checkpoint_every == 0:
                    save()
                if args.max_steps is not None and step >= args.max_steps:
                    print(f"stopped_step={step} epoch={epoch} cursor={cursor}")
                    return save()

            print(
                f"epoch={epoch} "
                f"value_loss={totals['value'] / totals['batches']:.4f} "
                f"policy_loss={totals['policy'] / totals['batches']:.4f}"
            )
        checkpoint = save()
        elapsed = time.perf_counter() - started
        print(f"saved={args.output} steps={step} elapsed_seconds={elapsed:.3f}")
        return checkpoint


def main() -> None:
    train(parse_args())


if __name__ == "__main__":
    main()
