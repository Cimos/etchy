#!/usr/bin/env python3
"""Generate a synthetic multi-layer Gerber board pair with an EXACT known delta.

Ground-truth test input for etchy's engine (Spike 1 correctness + perf) and the
seed of corpus/synthetic/. No KiCad needed. Output:

    <out>/revA/synth-<layer>.gbr      # base board
    <out>/revB/synth-<layer>.gbr      # identical except a precise injected delta
    <out>/ground_truth.json           # which layers changed + expected add/remove

The delta is deliberately simple and measurable:
  * F_Cu  : revB ADDS a block of flashes at a known location  -> known added area
  * In1_Cu: revB REMOVES a block of flashes                   -> known removed area
  * every other layer is byte-identical between revA and revB -> must diff to empty

RS-274X, 4.6 coordinate format, millimetres. Deterministic (no RNG).

Usage:
    python gen_synth_board.py --out ../synthetic --copper-layers 8 --grid 60
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

SCALE = 1_000_000  # 4.6 format: integer = mm * 1e6
PAD_DIA_MM = 0.5
BLOCK = 10  # delta block is BLOCK x BLOCK pads


def mm(v: float) -> int:
    return round(v * SCALE)


def header() -> list[str]:
    return [
        "G04 etchy synthetic board*",
        "%FSLAX46Y46*%",
        "%MOMM*%",
        f"%ADD10C,{PAD_DIA_MM:.6f}*%",
        "D10*",
    ]


def flash_grid(nx: int, ny: int, pitch: float, ox: float, oy: float) -> list[str]:
    """A grid of pad flashes — the bulk geometry (density)."""
    out = []
    for j in range(ny):
        for i in range(nx):
            out.append(f"X{mm(ox + i * pitch)}Y{mm(oy + j * pitch)}D03*")
    return out


def write_layer(path: Path, body: list[str]) -> None:
    lines = header() + body + ["M02*"]
    path.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")


def pad_area_mm2() -> float:
    import math

    return math.pi * (PAD_DIA_MM / 2.0) ** 2


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--copper-layers", type=int, default=8, help="inner+outer copper count")
    ap.add_argument("--grid", type=int, default=60, help="NxN pad grid per layer")
    ap.add_argument("--pitch", type=float, default=2.0, help="mm between pads")
    args = ap.parse_args()

    n = args.grid
    pitch = args.pitch
    base = flash_grid(n, n, pitch, 5.0, 5.0)  # the common geometry on every layer

    # Copper layer names: F_Cu, In1_Cu .. In(k)_Cu, B_Cu.
    cu = ["F_Cu"]
    cu += [f"In{i}_Cu" for i in range(1, max(0, args.copper_layers - 2) + 1)]
    cu += ["B_Cu"]
    others = ["F_Mask", "B_Mask", "F_Silk", "B_Silk", "F_Paste", "B_Paste"]
    layers = cu + others

    revA, revB = args.out / "revA", args.out / "revB"
    for d in (revA, revB):
        d.mkdir(parents=True, exist_ok=True)

    # Delta blocks placed clear of the base grid so areas are independent.
    add_block = flash_grid(BLOCK, BLOCK, 1.0, 5.0 + n * pitch + 5.0, 5.0)
    # "Removed" = a block present in revA's In1_Cu but absent in revB's.
    rm_block = flash_grid(BLOCK, BLOCK, 1.0, 5.0 + n * pitch + 5.0, 30.0)

    changed = {}
    for layer in layers:
        a_body = list(base)
        b_body = list(base)
        if layer == "F_Cu":
            b_body += add_block  # revB adds -> "added" geometry
            changed[layer] = {"kind": "added", "pads": BLOCK * BLOCK}
        elif layer == "In1_Cu":
            a_body += rm_block  # present in A, absent in B -> "removed"
            changed[layer] = {"kind": "removed", "pads": BLOCK * BLOCK}
        write_layer(revA / f"synth-{layer}.gbr", a_body)
        write_layer(revB / f"synth-{layer}.gbr", b_body)

    gt = {
        "format": "rs274x",
        "copper_layers": len(cu),
        "total_layers": len(layers),
        "pads_per_layer": n * n,
        "pad_area_mm2": round(pad_area_mm2(), 6),
        "block_pads": BLOCK * BLOCK,
        "expected_changed": changed,
        "expected_unchanged": [layer for layer in layers if layer not in changed],
        "note": "F_Cu gains a block (added); In1_Cu loses a block (removed); all "
        "other layers are byte-identical and must diff to empty.",
    }
    (args.out / "ground_truth.json").write_text(json.dumps(gt, indent=2) + "\n", encoding="utf-8")
    print(
        f"wrote {len(layers)} layers x2 ({n * n} pads/layer) to {args.out}; "
        f"changed: {list(changed)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
