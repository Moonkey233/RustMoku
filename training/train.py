"""Train the RustMoku V0.12 local-pattern Value/Policy model."""

from __future__ import annotations

import argparse
import random
import hashlib
import json
import time
import math
import array
import contextlib
import heapq
import copy
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
    parser.add_argument("--policy-target", choices=("soft", "ranking"), default="soft")
    parser.add_argument("--policy-weight", type=float, default=1.0)
    parser.add_argument("--exact-weight", type=float, default=4.0)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--device", default="cpu")
    parser.add_argument("--resume", type=Path)
    parser.add_argument("--max-steps", type=int, help="bounded interruption after this absolute step")
    parser.add_argument("--checkpoint-every", type=int, default=50)
    parser.add_argument("--torch-threads", type=int, default=1)
    parser.add_argument("--max-proof-fraction", type=float, default=.25)
    parser.add_argument('--architecture', choices=('v1', 'v2'), default='v1')
    parser.add_argument('--qat', action='store_true')
    parser.add_argument('--outcome-weight', type=float, default=0.0)
    parser.add_argument('--early-stop-patience', type=int, default=5)
    return parser.parse_args()


def make_example(record, symmetry: int, device: str, architecture='v1', score_scale=None):
    if not eligible_label(record):
        raise ValueError("fallback/analysis labels cannot be used as supervision")
    board, side = decode_position_key(record.position_key)
    board, policy_move = transform_position(board, record.policy_move, symmetry)
    global_keys = torch.tensor(feature_keys(board, side), dtype=torch.long, device=device)
    value = calibrated_value_target(record)
    if architecture == 'v2':
        from nonlinear_model import model_features, normalized_target
        global_keys = model_features(board, side, device)
        value = normalized_target(record, score_scale)
    moves, candidates = legal_policy_features(board, side)
    candidate_keys = torch.tensor(candidates, dtype=torch.long, device=device)
    target = -1
    if policy_move is not None:
        try:
            target = moves.index(policy_move)
        except ValueError as error:
            raise ValueError("policy target is not a legal empty cell") from error
    comparison_target = None
    if record.comparison is not None and not record.exact:
        from common import transform_index
        target_moves = [transform_index(at, symmetry) for at in record.comparison['moves']]
        comparison_target = ( [moves.index(at) for at in target_moves], record.comparison['probabilities'], record.comparison['scores'])
    return (
        global_keys,
        candidate_keys,
        target,
        value,
        record.exact,
        comparison_target,
    )


def train(args):
    with open_dataset(args.dataset) as dataset:
        return train_dataset(args, dataset)


def train_dataset(args, dataset):
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
    architecture = getattr(args, 'architecture', 'v1')
    qat = getattr(args, 'qat', False)
    outcome_weight = getattr(args, 'outcome_weight', 0.0)
    patience = getattr(args, 'early_stop_patience', 5)
    if architecture not in ('v1', 'v2') or (qat and architecture != 'v2'):
        raise ValueError('invalid architecture/QAT combination')
    if not math.isfinite(outcome_weight) or not 0 <= outcome_weight <= 1 or patience < 1:
        raise ValueError('invalid outcome weight or early stop patience')
    resumed = load_checkpoint(args.resume, args.device) if args.resume else None
    manifest = resumed['split_manifest'] if resumed else make_split_manifest(dataset, args.seed)
    splits = validate_split_manifest(dataset, manifest)
    score_scale = None
    scale_evidence = None
    if architecture == 'v2':
        # Deterministic reservoir over TRAIN only; test/heldout cannot tune S.
        rng = random.Random(0)
        scores = []
        seen = 0
        for i in splits['train']:
            record = dataset[i]
            if not record.exact:
                seen += 1
                value = abs(record.value)
                if len(scores) < 4096:
                    scores.append(value)
                else:
                    index = rng.randrange(seen)
                    if index < len(scores): scores[index] = value
        if not scores:
            raise ValueError('V2 requires ordinary training scores to calibrate its scale')
        scores.sort()
        score_scale = max(100, min(1_000_000, scores[len(scores) // 2]))
        scale_evidence = {'split': 'train', 'method': 'seed0-reservoir4096-median-abs-clamp100-1000000',
                          'ordinary_records': seen, 'samples': len(scores),
                          'p10': scores[len(scores)//10], 'p50': scores[len(scores)//2],
                          'p90': scores[len(scores)*9//10], 'maximum': scores[-1],
                          'sample_sha256': hashlib.sha256(json.dumps(scores).encode()).hexdigest()}
        if not splits['validation']:
            raise ValueError('V2 model selection requires a fixed validation split')
    configuration = {key: getattr(args, key) for key in (
        "epochs", "batch_size", "learning_rate", "policy_weight", "exact_weight", "seed", "device", "torch_threads", "max_proof_fraction")}
    configuration["torch_version"] = str(torch.__version__)
    configuration["architecture"] = "local-pattern-linear-v1" if architecture == 'v1' else 'local-pattern-relu-width8-v2'
    configuration["value_contract"] = "ordinary-limit-normalized-exact-sign-v1" if architecture == 'v1' else 'stm-rational-q15-v2'
    configuration.update(policy_target=getattr(args, 'policy_target', 'soft'), qat=qat, outcome_weight=outcome_weight, score_scale=score_scale,
                         scale_evidence=scale_evidence, early_stop_patience=patience)
    configuration_hash = hashlib.sha256(json.dumps(configuration, sort_keys=True).encode()).hexdigest()
    if resumed is not None and (resumed.get("checkpoint_version") != 2 or resumed.get("configuration") != configuration
                                or resumed.get("configuration_sha256") != configuration_hash):
        raise ValueError("resume configuration/version mismatch")
    if args.output.exists() and args.resume is None:
        raise ValueError("output checkpoint exists; use an explicit resume or new output")
    started = time.perf_counter()
    random.seed(args.seed)
    torch.manual_seed(args.seed)
    torch.use_deterministic_algorithms(True)
    from nonlinear_model import NonlinearModel
    model = (LocalPatternModel() if architecture == 'v1' else NonlinearModel(qat=qat)).to(args.device)
    optimizer = torch.optim.AdamW(model.parameters(), lr=args.learning_rate)

    with contextlib.nullcontext(dataset):
        ordinary = array.array('I', (i for i in splits['train'] if dataset[i].source != 7))
        proof = array.array('I', (i for i in splits['train'] if dataset[i].source == 7))
        random.Random(args.seed).shuffle(proof)
        cap = int(len(ordinary) * args.max_proof_fraction / (1 - args.max_proof_fraction))
        selected_proof = array.array('I', sorted(proof[:cap]))
        splits['train'] = array.array('I', heapq.merge(ordinary, selected_proof))
        del ordinary, proof, selected_proof
        args.output.parent.mkdir(parents=True, exist_ok=True)
        save_split_manifest(args.output.with_suffix(".split.json"), manifest)
        if not splits["train"]:
            raise ValueError("dataset has no training games")
        generator = torch.Generator().manual_seed(args.seed)
        epoch = cursor = step = 0
        order = None
        best_loss = None
        best_state = None
        stale_epochs = 0
        history = []
        stopped_early = False
        totals = {'value': 0.0, 'policy': 0.0, 'batches': 0}
        if resumed:
            model.load_state_dict(resumed["state_dict"])
            optimizer.load_state_dict(resumed["optimizer"])
            epoch, cursor, step = resumed["epoch"], resumed["cursor"], resumed["step"]
            order = resumed["order"]
            if isinstance(order, list):
                order = torch.tensor(order, dtype=torch.int32)
            if (type(epoch) is not int or not 0 <= epoch <= args.epochs
                    or type(cursor) is not int or not 0 <= cursor < len(splits["train"])
                    or type(step) is not int or step < 0
                    or (order is None and cursor != 0)
                    or (order is not None and (order.ndim != 1 or len(order) != len(splits['train'])
                        or not torch.equal(torch.sort(order.cpu()).values, torch.arange(len(splits['train']), dtype=order.dtype))))):
                raise ValueError("invalid checkpoint data cursor")
            generator.set_state(resumed["shuffle_rng"].cpu())
            random.setstate(resumed["python_rng"])
            torch.set_rng_state(resumed["torch_rng"].cpu())
            if args.device.startswith("cuda"):
                torch.cuda.set_rng_state_all(resumed["cuda_rng"])
            best_loss, best_state = resumed.get('best_validation_loss'), resumed.get('selected_state_dict')
            stale_epochs, history = resumed.get('stale_epochs', 0), resumed.get('history', [])
            stopped_early = resumed.get('stopped_early', False)
            totals = resumed.get('epoch_totals', totals)

        def save():
            checkpoint = {
                "format": "rustmoku-local-pattern-v1" if architecture == 'v1' else 'rustmoku-nonlinear-v2', "checkpoint_version": 2,
                "state_dict": model.state_dict(), "optimizer": optimizer.state_dict(),
                "scheduler": None, "epoch": epoch, "cursor": cursor, "step": step, "order": order,
                "seed": args.seed, "split_manifest": manifest,
                "configuration": configuration, "configuration_sha256": configuration_hash,
                "python_rng": random.getstate(), "torch_rng": torch.get_rng_state(),
                "shuffle_rng": generator.get_state(),
                "cuda_rng": torch.cuda.get_rng_state_all() if args.device.startswith("cuda") else [],
                'best_validation_loss': best_loss, 'selected_state_dict': best_state,
                'stale_epochs': stale_epochs, 'history': history, 'stopped_early': stopped_early,
                'epoch_totals': totals,
            }
            atomic_save(args.output, checkpoint)
            return checkpoint

        while epoch < args.epochs and not stopped_early:
            if args.max_steps is not None and step >= args.max_steps:
                return save()
            if order is None:
                order = torch.randperm(len(splits["train"]), generator=generator, dtype=torch.int32)
                totals = {"value": 0.0, "policy": 0.0, "batches": 0}
            for start in range(cursor, len(order), args.batch_size):
                selected = order[start : start + args.batch_size].tolist()
                examples = []
                for local_index in selected:
                    record_index = splits["train"][local_index]
                    # D4 augmentation is deterministic and training-only.
                    symmetry = random.Random(
                        (args.seed << 32) ^ (epoch << 20) ^ record_index
                    ).randrange(8)
                    examples.append(make_example(dataset[record_index], symmetry, args.device, architecture, score_scale))
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
                outcome_items = [(j, dataset[splits['train'][local]].outcome)
                                 for j, local in enumerate(selected)
                                 if dataset[splits['train'][local]].outcome is not None
                                 and not dataset[splits['train'][local]].exact]
                if outcome_weight and outcome_items:
                    outcome_prediction = predicted[[j for j, _ in outcome_items]]
                    outcome_targets = torch.tensor([value for _, value in outcome_items], dtype=torch.float32, device=args.device)
                    value_loss = value_loss + outcome_weight * functional.smooth_l1_loss(outcome_prediction, outcome_targets)
                policy_losses = []
                for _, candidates, target, _, _, comparison_target in examples:
                    if comparison_target is not None:
                        from teacher import masked_loss
                        indices, probabilities, scores = comparison_target
                        policy_losses.append(masked_loss(model.policy(candidates), indices, probabilities,
                            ranking_scores=scores if getattr(args, 'policy_target', 'soft') == 'ranking' else None))
                    elif target >= 0:
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
                    if architecture == 'v2':
                        with torch.no_grad():
                            losses = []
                            # Frozen bounded validation prefix, never test.
                            for index in splits['validation'][:256]:
                                keys, _, _, target, _, _ = make_example(dataset[index], 0, args.device, architecture, score_scale)
                                prediction = model.value(keys)
                                losses.append(float((prediction - target).abs().item()))
                            validation_loss = sum(losses) / len(losses)
                        if not math.isfinite(validation_loss):
                            raise ValueError('non-finite validation loss')
                        history.append({'epoch': epoch, 'validation_mae': validation_loss,
                                        'training_value_loss': totals['value'] / totals['batches'],
                                        'training_policy_loss': totals['policy'] / totals['batches']})
                        if best_loss is None or validation_loss < best_loss:
                            best_loss, best_state, stale_epochs = validation_loss, copy.deepcopy(model.state_dict()), 0
                        else:
                            stale_epochs += 1
                        stopped_early = stale_epochs >= patience
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
