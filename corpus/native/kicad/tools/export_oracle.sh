#!/bin/sh
set -eu

usage() {
    echo "usage: $0 FIXTURE.kicad_pcb OUTPUT_DIR" >&2
    exit 2
}

[ "$#" -eq 2 ] || usage
fixture=$1
output_dir=$2
[ -f "$fixture" ] || { echo "fixture not found: $fixture" >&2; exit 2; }

expected_version=7.0.11
actual_version=$(kicad-cli version)
[ "$actual_version" = "$expected_version" ] || {
    echo "expected kicad-cli $expected_version, found $actual_version" >&2
    exit 2
}

mkdir -p "$output_dir/gerbers" "$output_dir/drill" "$output_dir/position"
printf 'kicad-cli %s\n' "$actual_version" > "$output_dir/KICAD_VERSION.txt"

# Keep the layer order fixed. A layer is requested only when its exact KiCad
# name occurs in the fixture's layer table; this lets intentionally tiny boards
# omit irrelevant layers without relying on saved plot settings.
layers=
for layer in F.Cu In1.Cu In2.Cu B.Cu F.Paste B.Paste F.Mask B.Mask F.SilkS B.SilkS Edge.Cuts; do
    if grep -Eq "\([0-9]+ \"$layer\" (signal|power|user)" "$fixture"; then
        if [ -n "$layers" ]; then layers="$layers,$layer"; else layers=$layer; fi
    fi
done
[ -n "$layers" ] || { echo "no oracle layers found in $fixture" >&2; exit 2; }

kicad-cli pcb export gerbers \
    --output "$output_dir/gerbers/" \
    --layers "$layers" \
    --precision 6 \
    "$fixture"

kicad-cli pcb export drill \
    --output "$output_dir/drill/" \
    --format excellon \
    --drill-origin absolute \
    --excellon-zeros-format decimal \
    --excellon-oval-format alternate \
    --excellon-units mm \
    --excellon-separate-th \
    "$fixture"

kicad-cli pcb export pos \
    --output "$output_dir/position/placements.csv" \
    --side both \
    --format csv \
    --units mm \
    "$fixture"
