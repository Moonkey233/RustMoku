"""Quantize and export a training checkpoint to the Rust V0.12 format."""

from __future__ import annotations

import argparse
import math
import hashlib
import os
import struct
import tempfile
from pathlib import Path

import torch
from provenance import export_identity, write_export, check_file
from common import read_quantized_model

from common import (
    EVALUATION_LIMIT,
    MAX_MODEL_BYTES,
    MODEL_ARCHITECTURE_ID,
    MODEL_FEATURE_COUNT,
    MODEL_FORMAT_VERSION,
    MODEL_HEADER,
    MODEL_HIDDEN,
    MODEL_MAGIC,
    POLICY_OUTPUT_SCALE,
    load_training_model,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--dataset", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--embedding-scale", type=int, default=16384)
    parser.add_argument("--value-head-scale", type=int, default=16384)
    parser.add_argument("--policy-head-scale", type=int, default=16384)
    return parser.parse_args()


def quantize(tensor: torch.Tensor, preferred_scale: int, name: str) -> tuple[torch.Tensor, int]:
    if preferred_scale < 1:
        raise ValueError(f"{name} scale must be positive")
    tensor = tensor.detach().to(device="cpu", dtype=torch.float64).contiguous()
    if not torch.isfinite(tensor).all():
        raise ValueError(f"{name} contains a non-finite value")
    maximum = float(tensor.abs().max()) if tensor.numel() else 0.0
    scale = preferred_scale
    if maximum > 0.0:
        scale = min(scale, math.floor(32767.0 / maximum))
    if scale < 1:
        raise ValueError(f"{name} cannot be represented as i16")
    values = torch.round(tensor * scale).to(torch.int64)
    if int(values.min()) < -32768 or int(values.max()) > 32767:
        raise ValueError(f"{name} quantization exceeds i16")
    return values, scale


def calibrated_divisor(combined_scale: int, output_scale: int) -> int:
    """V1 truncates the final dot/divisor. Bound gain error to one percent.

    This checks scale calibration, separately from weight rounding error. V1
    bytes retain their existing meaning; unrepresentable exports fail closed.
    """
    divisor = max(1, round(combined_scale / output_scale))
    if abs(combined_scale - divisor * output_scale) * 100 > divisor * output_scale:
        raise ValueError("V1 divisor gain error exceeds 1%; increase quantization scales")
    return divisor


def main() -> None:
    args = parse_args()
    identity = export_identity(args.checkpoint, args.dataset)
    model = load_training_model(args.checkpoint, "cpu")
    from mixlite import MixLite, IntegerMixLite
    if isinstance(model, MixLite):
        payload = model.bytes()
        IntegerMixLite(payload)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        check_file(identity['checkpoint'])
        check_file(identity['dataset'])
        publish_model(args.output, payload)
        write_export(args.output, identity, dict(contract='mixlite-d4-int8-int16-v1',
            value_divisor=model.value_divisor, policy_divisor=model.policy_divisor, score_scale=model.score_scale))
        print(f'saved={args.output} architecture=4 bytes={len(payload)}')
        return
    from nonlinear_model import NonlinearModel, export_integer
    if isinstance(model, NonlinearModel):
        from checkpoint import load_checkpoint
        payload, quantization = export_integer(model, args, load_checkpoint(args.checkpoint)['configuration']['score_scale'])
        args.output.parent.mkdir(parents=True, exist_ok=True)
        check_file(identity['checkpoint'])
        publish_model(args.output, payload)
        write_export(args.output, identity, quantization)
        print(f'saved={args.output} architecture=2 bytes={len(payload)}')
        return
    embeddings, embedding_scale = quantize(
        model.embedding.weight, args.embedding_scale, "embeddings"
    )
    value_head, value_head_scale = quantize(
        model.value_head.weight.squeeze(0), args.value_head_scale, "value head"
    )
    policy_head, policy_head_scale = quantize(
        model.policy_head, args.policy_head_scale, "policy head"
    )
    value_quantization_scale = embedding_scale * value_head_scale
    policy_quantization_scale = embedding_scale * policy_head_scale
    if value_quantization_scale > 2_147_483_647 or policy_quantization_scale > 2_147_483_647:
        raise ValueError("combined quantization scale exceeds i32")
    # The float Value head learns [-1, 1], then the production divisor maps it
    # into RustMoku's ordinary evaluator units. Policy logits get a modest
    # fixed integer resolution used only for ordering.
    value_scale = calibrated_divisor(value_quantization_scale, EVALUATION_LIMIT)
    policy_scale = calibrated_divisor(policy_quantization_scale, POLICY_OUTPUT_SCALE)
    value_bias = round(float(model.value_head.bias.item()) * value_quantization_scale)
    maximum_accumulator = 225 * 4 * 32768
    maximum_value_dot = maximum_accumulator * 32768 * MODEL_HIDDEN
    if abs(value_bias) > (2**63 - 1) - maximum_value_dot:
        raise ValueError("quantized value bias violates Rust arithmetic bounds")
    flat = torch.cat((embeddings.flatten(), value_head, policy_head)).tolist()
    header = MODEL_HEADER.pack(
        MODEL_MAGIC,
        MODEL_FORMAT_VERSION,
        MODEL_ARCHITECTURE_ID,
        MODEL_FEATURE_COUNT,
        MODEL_HIDDEN,
        0,
        value_scale,
        value_bias,
        policy_scale,
        MODEL_FEATURE_COUNT * MODEL_HIDDEN,
        MODEL_HIDDEN,
        MODEL_HIDDEN,
    )
    payload = struct.pack(f"<{len(flat)}h", *flat)
    if len(header) + len(payload) > MAX_MODEL_BYTES:
        raise ValueError("exported model exceeds the Rust size limit")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    check_file(identity['checkpoint'])
    publish_model(args.output, header + payload)
    write_export(args.output, identity, {'embedding_scale': embedding_scale,
        'value_head_scale': value_head_scale, 'policy_head_scale': policy_head_scale,
        'value_divisor': value_scale, 'policy_divisor': policy_scale})
    print(
        f"saved={args.output} bytes={len(header) + len(payload)} "
        f"embedding_scale={embedding_scale} value_divisor={value_scale} "
        f"policy_divisor={policy_scale}"
    )


def publish_model(path, payload):
    """Publish fully parsed model bytes without replacing a frozen model."""
    digest = hashlib.sha256(payload).hexdigest()
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, suffix='.partial', delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        read_quantized_model(temporary)
        try:
            os.link(temporary, path)
        except FileExistsError:
            from dataset import file_hash
            if path.is_symlink() or file_hash(path) != digest:
                raise ValueError('refusing to replace an immutable exported model')
    finally:
        if temporary is not None and temporary.exists():
            temporary.unlink()


if __name__ == "__main__":
    main()
