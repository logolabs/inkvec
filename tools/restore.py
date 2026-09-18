#!/usr/bin/env python3
"""Standalone restorer CLI for inkvec --restore-command.

Usage:
    python tools/restore.py input.png output.png [--model path/to/restorer.onnx]
"""
import argparse
import os
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import numpy as np
from PIL import Image

def main():
    parser = argparse.ArgumentParser(description="Neural restorer pre-pass for inkvec")
    parser.add_argument("input", help="Input image path (JPEG/PNG/WebP)")
    parser.add_argument("output", help="Output restored image path (PNG)")
    parser.add_argument(
        "--model",
        default=str(Path(__file__).resolve().parents[1] / "crates" / "inkvec-restore" / "models" / "restorer.onnx"),
        help="Path to restorer.onnx model",
    )
    parser.add_argument("--tile", type=int, default=128, help="Tile dimension (default 128)")
    parser.add_argument("--overlap", type=int, default=32, help="Tile overlap (default 32)")
    parser.add_argument("--snap", type=int, default=6, help="Snap extremes threshold (default 6)")
    parser.add_argument("--workers", type=int, default=8, help="Number of worker threads (default 8)")
    args = parser.parse_args()

    import onnxruntime as ort

    model_path = Path(args.model)
    if not model_path.is_file():
        # Check standard cache directory before pulling
        cache_path = Path.home() / ".cache" / "inkvec" / "models" / "restorer.onnx"
        if cache_path.is_file():
            model_path = cache_path
        else:
            print(f"Model file not found at {model_path}. Auto-pulling from Hugging Face...", file=sys.stderr)
            try:
                from pull_model import pull_model
                model_path = pull_model(dest=model_path)
            except Exception as e:
                # If cannot write to model_path, pull to cache_path
                try:
                    from pull_model import pull_model
                    model_path = pull_model(dest=cache_path)
                except Exception as e2:
                    sys.stderr.write(f"Error: model file not found and auto-pull failed: {e} / {e2}\n")
                    sys.exit(1)

    opts = ort.SessionOptions()
    opts.intra_op_num_threads = 2
    sess = ort.InferenceSession(str(model_path), opts, providers=["CPUExecutionProvider"])
    input_name = sess.get_inputs()[0].name
    output_name = sess.get_outputs()[0].name

    img = Image.open(args.input).convert("RGB")
    arr = np.array(img)
    h, w, _ = arr.shape

    tile = args.tile
    overlap = args.overlap
    step = tile - overlap

    x = arr.astype(np.float32) / 255.0
    x = np.transpose(x, (2, 0, 1))[np.newaxis, ...] # (1, 3, H, W)

    out = np.zeros_like(x)
    acc = np.zeros((1, 1, h, w), dtype=np.float32)

    patches = []
    coords = []

    for y0 in range(0, max(h - overlap, 1), step):
        for x0 in range(0, max(w - overlap, 1), step):
            y1, x1 = min(y0 + tile, h), min(x0 + tile, w)
            y0b, x0b = max(y1 - tile, 0), max(x1 - tile, 0)
            patch = x[..., y0b:y1, x0b:x1]
            patches.append(patch)
            coords.append((y0b, y1, x0b, x1))

    def run_tile(p):
        return sess.run([output_name], {input_name: p})[0]

    with ThreadPoolExecutor(max_workers=args.workers) as ex:
        results = list(ex.map(run_tile, patches))

    for (y0b, y1, x0b, x1), q in zip(coords, results):
        out[..., y0b:y1, x0b:x1] += q
        acc[..., y0b:y1, x0b:x1] += 1

    merged = np.clip(out / np.maximum(acc, 1.0), 0.0, 1.0)[0]
    merged = np.transpose(merged, (1, 2, 0))
    restored_u8 = (merged * 255.0 + 0.5).astype(np.uint8)

    if args.snap > 0:
        snap_white = (restored_u8 >= 255 - args.snap).all(axis=-1)
        snap_black = (restored_u8 <= args.snap).all(axis=-1)
        restored_u8[snap_white] = 255
        restored_u8[snap_black] = 0

    out_img = Image.fromarray(restored_u8)
    Path(args.output).parent.mkdir(parents=True, exist_ok=True)
    out_img.save(args.output)
    print(f"Restored image saved to {args.output}")

if __name__ == "__main__":
    main()
