"""Inspect a production RustMoku learned-model artifact."""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path

from common import MODEL_FEATURE_COUNT, MODEL_HIDDEN, read_quantized_model


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", type=Path, required=True)
    args = parser.parse_args()
    model = read_quantized_model(args.model)
    digest = hashlib.sha256(args.model.read_bytes()).hexdigest()
    print(
        f"architecture=local-pattern-v1 features={MODEL_FEATURE_COUNT} hidden={MODEL_HIDDEN} "
        f"value_scale={model.value_scale} policy_scale={model.policy_scale} "
        f"embedding_min={min(model.embeddings)} embedding_max={max(model.embeddings)} "
        f"sha256={digest}"
    )


if __name__ == "__main__":
    main()
