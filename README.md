![etchy — fast, trustworthy PCB visual + geometric diff](assets/brand/png/etchy-banner-1280x320.png)

# etchy

> **Status: Phase 0 — scaffold.** Not yet usable. The working tool today is the
> Python predecessor, frozen at [Cimos/Gerber-Diff-Tool](https://github.com/Cimos/Gerber-Diff-Tool)
> (v0.11). etchy is its ground-up Rust successor.

**etchy** is a fast, trustworthy, open-source **PCB visual + geometric diff** tool.
Point it at two revisions of a board's fab output and it shows — and measures —
exactly what changed.

```
etchy old/ new/        # → crisp SVG overlay + change heatmap + JSON magnitudes
```

## What it does (target)

- **Visual + geometric diff** of Gerber/Excellon revisions: per-layer polygon
  boolean diff (`added = B − A`, `removed = A − B`) → a resolution-independent
  **SVG overlay**, a **change heatmap** ("where to look"), and **magnitudes**
  (changed area, region count) for CI thresholds — all from one computation.
- **Schematic-PDF** page-by-page pixel diff (kept from the predecessor).
- **CLI/CI-first** (exit codes, per-layer thresholds, JSON, PR comments) with a
  **native egui desktop viewer** second.
- **Same-board revisions only** — fails loud on mismatched boards, never a garbage
  diff. **Trustworthy:** golden corpus + property + fuzz tests; no silent misses.
- Ships as small **static binaries** + a **distroless container**.

**Non-goals:** native CAD ingestion (KiCad/Altium/IPC-2581/ODB++), net/connectivity
diff, BOM/component diff, DRC.

## Documentation

- [`docs/ROADMAP.md`](docs/ROADMAP.md) — phased plan to 1.0.
- [`docs/DEVELOPER_GUIDE.md`](docs/DEVELOPER_GUIDE.md) — architecture + the verified Rust crate stack.
- [`docs/PRODUCT_DISCOVERY.md`](docs/PRODUCT_DISCOVERY.md) — the requirements interview behind every decision.
- [`deploy/SETUP.md`](deploy/SETUP.md) — stand up the hosted web demo + feedback widget on a new machine.

> ⚠ **Before making this repo public**, work through [`PRE_PUBLIC.md`](PRE_PUBLIC.md) —
> notably, demo feedback under `deploy/feedback/` contains tester IP/UA/names that
> must be scrubbed first.

## Workspace

| Crate | Role |
|---|---|
| `etchy-core` | the engine: parse → resolve → polygonize → diff → measure → render |
| `etchy-cli` | binary `etchy` — the primary CLI/CI surface |
| `etchy-gui` | native egui viewer (separate binary; never compiled in headless builds) |
| `etchy-pdf` | schematic-PDF pixel diff (feature-gated; keeps the heavy PDFium dep out of the core) |

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE)
at your option.
