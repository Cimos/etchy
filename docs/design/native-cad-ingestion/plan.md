# Native board ingestion implementation and release plan

This plan intentionally separates plumbing, producer bridges, geometry, and
release claims. Each pull request is small enough to review and has its own
trust tests. No phase changes existing Gerber behavior without regression
coverage.

## Proposed versions

| Release | Scope |
|---|---|
| 0.2.0 | Input dispatch, ingestion manifest, and stable native KiCad `.kicad_pcb` support through a tested external `kicad-cli`; native desktop/CLI/Action, fail-loud in wasm. |
| 0.2.1 | KiCad corpus/version expansion and profile-file support if needed; no new format promise. |
| 0.3.0 | Experimental Altium `.PcbDoc` support behind `native-altium-experimental`; CLI and native GUI first. |
| 0.4.0 | Stable Altium support only if parity, adversarial, licence, dependency, and distribution gates pass; otherwise 0.4.0 does not remove the experimental label. wasm Altium may ship here only after separate browser gates pass. |

Suggested feature flags:

- `native-kicad`: CLI/native-GUI process bridge and profile support. It adds no
  linked KiCad code. Enable it in normal native release builds after 0.2.0.
- `native-altium-experimental`: exact-pinned parser plus adapter/polygonizer.
  Off by default in release archives until the stable gate.
- `native-altium`: stable name introduced only when the experimental gate is
  met; it may alias the same implementation for one deprecation cycle.
- Neither flag changes `etchy-core`'s default Gerber/Excellon behavior. wasm
  must not include process-only KiCad code.

## Phase 0: decisions and fixtures

### PR 1 — policy and profile decision

Crates/files: documentation only.

1. Owner resolves the open questions in `spec.md`, especially the default
   profile, KiCad minimum version, wasm expectations, and Altium OutJob policy.
2. Update `CLAUDE.md`, `docs/REQUIREMENTS.md`, `docs/TRUST.md`, and
   `docs/ROADMAP.md` with the exact narrow scope in `NCAD-SCOPE-4`.
3. Add a versioned native-ingestion design document and supported-version table.

Tests: documentation link/check tests only. Review gate: no implementation
starts with “same as Gerber” still undefined.

### PR 2 — corpus provenance and oracle recipe

Crates/files: `corpus/`, test tooling outside runtime crates.

1. Add manifests recording source URL/licence, producer version, export command
   or OutJob, profile hash, and expected layers.
2. Add two small synthetic KiCad revisions and, after provenance review, the
   native `Cimos/Mad_RP2040` revisions that correspond to public demo output.
3. Define scripts/instructions for generating oracle Gerber, drill, and
   placement packs. Generated outputs are checked in where licensing permits so
   normal CI does not need every old producer.
4. Establish a private-to-public sanitization process for Altium fixtures; do
   not copy `altium-monkey`'s private corpus.

Tests: manifest schema, hashes, expected file inventory, and existing fab-pack
golden tests. No runtime code.

## Phase 1: shared ingestion boundary

### PR 3 — input family and manifest types

Crates: `etchy-core`, `etchy-cli`, `etchy-gui`.

1. Add pure `InputFamily`, `IngestionManifest`, profile ID/hash, producer
   metadata, and typed ingestion-error data in `etchy-core`.
2. Add suffix-plus-content detection in shared pure code.
3. Wire dispatch only far enough to return explicit “recognized but this build
   lacks support” errors. Preserve PDF and fab-pack routing exactly.

Tests:

- table tests for casing, whitespace, KiCad root, CFB magic, false extensions,
  mixed pairs, native-only flags, PDFs, zips, directories, and hostile short
  inputs;
- CLI integration tests for exit 2 and exact corrective messages;
- native/wasm GUI loader tests for availability messages.

### PR 4 — resolved-board adapter API

Crate: `etchy-core`; optional thin crates may be scaffolded but contain no
format parser yet.

1. Add a pure builder that validates adapter layer uniqueness, record counts,
   fixed-point bounds, and completeness before producing `Board`.
2. Keep I/O/process/profile discovery in surfaces or adapter crates.
3. Attach ingestion metadata to detailed reports without changing existing
   geometry calculations.

Tests: known polygons map to every existing `LayerKind`; duplicate/ambiguous
layers and incomplete manifests fail; report serialization remains backward
compatible according to the owner's schema decision; `diff(A,A)` and symmetry
continue to pass.

## Phase 2: KiCad 0.2.0

### PR 5 — `kicad-cli` discovery and capability probe

Crates: new `etchy-kicad` plus `etchy-cli`; no GUI yet.

1. Resolve explicit flag, environment variable, then `PATH`.
2. Run `version` and capture help for Gerber, drill, and position commands.
3. Parse capabilities into a typed matrix; reject untested versions and missing
   zone checks.
4. Add bounded child-process output and timeout handling.

Tests:

- fake executables/scripts for 7.0.11, supported 8/9 variants, malformed
  versions, missing flags, non-zero exits, signals, timeout, excessive output,
  and paths with spaces;
- assert 7.0.11 zone behavior fails as specified rather than silently exporting;
- no real KiCad required in ordinary unit tests.

### PR 6 — secure temporary export bridge

Crate: `etchy-kicad`, with minimal CLI wiring.

1. Create separate private old/new temporary directories.
2. Build command arguments without a shell.
3. Export Gerbers with a locked profile, then drill and position files.
4. Inventory actual output, reject missing/unexpected path escapes, and feed the
   complete set to a shared fab-pack bytes loader.
5. Delete by default and support explicit diagnostic retention.

Tests:

- fake producer writes representative files; verify argument tokens, directory
  isolation, cleanup, retention, non-UTF-8 diagnostics, output path validation,
  and stage-labelled errors;
- test profile mismatch before diff;
- reuse current Gerber/Excellon/placement golden tests.

### PR 7 — real KiCad parity matrix

Crates: `etchy-kicad`, `etchy-cli`; corpus tests.

1. Run supported KiCad release containers/installations over synthetic boards
   and Mad_RP2040.
2. Compare bridge-generated `Board` values against checked-in reference packs
   per layer.
3. Cover zones, text/font/variables, footprint flips, custom pads, arcs,
   outlines, drill split, position output, and layer stacks.
4. Publish the tested version/profile table.

Tests: native-vs-export zero-diff assertions, known error goldens, property
tests, fuzzed board headers/profile documents, and export timeout/resource caps.
This PR is the KiCad correctness gate.

### PR 8 — GUI and Action

Crates/files: `etchy-gui`, `etchy-cli`, `action.yml`, CI workflows, packaging.

1. Add native file filters/drop handling, background conversion, cancellation,
   and producer/profile details.
2. Add wasm's immediate explanatory rejection for `.kicad_pcb`.
3. Add opt-in Action setup pinned to supported KiCad artifacts/container digest.
4. Exercise release archives without KiCad and document the external dependency.

Tests: native GUI loader state tests, cancellation and cleanup tests, wasm build
and message test, Action fixture workflow on a pinned runner image, Windows,
macOS, and Linux path/discovery tests.

### PR 9 — KiCad documentation and 0.2.0 release

Files: README, CLI reference, website, Action docs, trust/limitations,
third-party notices, changelog, release workflow.

Document install commands for each OS, supported KiCad versions, exact profile,
zone policy, wasm limitation, CI setup, privacy/temporary files, and how to
reproduce a comparison from exported packs. Release only after all KiCad gates
below pass.

## Phase 3: Altium experimental 0.3.0

### PR 10 — dependency and format spike

Crate: new `etchy-altium`; feature `native-altium-experimental`.

1. Pin the selected `altium-format` release by exact version after checking its
   full dependency licence tree with `cargo deny`.
2. Audit its CFB bounds and unknown-record behavior; wrap it so parser warnings
   cannot become dropped manufacturing records.
3. Compile native targets and `wasm32-unknown-unknown`; measure representative
   board memory/time.
4. Parse inventory only: emit `IngestionManifest`, not geometry or a diff.

Tests: CFB bombs/truncation/cycles, stream/record counts, unknown records,
dependency licence CI, native target matrix, wasm compile and browser parse cap.
Exit criterion: owner explicitly accepts the pinned dependency/API risk.

### PR 11 — layer stack, outline, and simple primitives

Crate: `etchy-altium`, feeding `etchy-core`.

1. Resolve legacy/new layer IDs and physical copper order.
2. Polygonize board outline/cutouts, tracks, simple arcs, fills, and regions.
3. Convert units to checked 1 nm integers and reuse etchy boolean utilities.
4. Reject every primitive variant not yet implemented.

Tests: synthetic exact-area fixtures, old/new layer maps, arc tessellation error
bounds, outline topology, overflow, open outline, transform invariants, and
Altium-Gerber parity for this limited subset.

### PR 12 — pads, vias, component transforms, drills, placement

Crate: `etchy-altium`.

1. Add supported pad/custom-pad/via stack shapes and component transforms.
2. Separate PTH/NPTH and emit placement markers through existing semantics.
3. Apply ownership once and validate counts.

Tests: rotations, bottom mirroring, through/SMD/multilayer pads, slots, blind
and buried vias if supported, custom pad holes, duplicate-ownership traps,
known areas, and reference-export parity. Unsupported modes retain golden hard
errors.

### PR 13 — saved pours and planes

Crate: `etchy-altium`.

1. Link polygon definitions to saved tracks/arcs/regions and plane geometry.
2. Respect shelving, cutouts, net inheritance only where needed for correct
   material, and negative-plane semantics.
3. Detect absent/inconsistent/stale fill conservatively; do not repour.

Tests: filled/unfilled/stale/shelved pours, thermal/solid connections where
represented in saved geometry, split/negative planes, islands, cutouts,
double-count prevention, and per-layer Altium-Gerber parity.

### PR 14 — mask, paste, and silk text

Crate: `etchy-altium`.

1. Implement only audited mask/paste expansion precedence.
2. Add supported stroke/text rendering and special-string substitution.
3. Reject unavailable fonts and every unproved plotting mode.

Tests: rule precedence, tenting, paste shrink/expansion, flipped silk, stroke
glyph goldens, TrueType supported/unsupported cases, variables/special strings,
and pixel/geometry-assisted comparison against Altium Gerber output.

### PR 15 — experimental CLI and native GUI surface

Crates: `etchy-cli`, `etchy-gui`, `etchy-altium`.

1. Enable `.PcbDoc` dispatch only with the experimental feature.
2. Display the experimental label and full manifest in every output.
3. Keep wasm disabled unless the Phase-10 browser gates and current corpus pass.

Tests: CLI exit contract, GUI load/cancel, report metadata, feature-off errors,
static release builds, and, if enabled, browser caps.

### PR 16 — adversarial audit and 0.3.0 release

Crates: all affected; docs and CI.

Run a review focused on CFB/resource attacks, unknown record handling, integer
overflow, transforms, pours, planes, text, and layer identity. Fix findings in
separate small PRs. Publish 0.3.0 only after the audit has no unresolved silent
miss; keep the feature experimental even if usable.

## Phase 4: Altium stable gate and 0.4.0

Altium may lose the experimental label only when all of these are true:

1. A legally redistributable corpus covers the agreed Altium producer/version
   range and every requirement in `NCAD-TEST-4`.
2. Every accepted fixture has independent Gerber/NC/position oracle output and
   zero unexplained per-layer geometry difference.
3. Unknown manufacturing records and unsupported plotting rules always fail.
4. `cargo deny`, supply-chain review, native static builds, Action tests, fuzzing,
   resource caps, and adversarial review are green.
5. Parser version/API stability is acceptable to the owner; an exact pin and
   update policy are documented.
6. wasm is advertised only if real-browser performance, memory caps, and the
   same parity corpus pass. Otherwise the web viewer continues to fail loud.

The stabilization PR renames/enables the stable feature, removes experimental
labels, publishes the support matrix, and updates README, website, CLI/GUI help,
Action examples, `TRUST.md`, third-party notices, changelog, and release notes.

## CI structure

| Job | Runs on ordinary PRs | Scheduled/manual |
|---|---|---|
| Existing workspace fmt/clippy/test/deny | Yes | Yes |
| Native adapter unit/property/golden tests | Yes | Yes |
| wasm compile and browser smoke | Yes once relevant feature is enabled | Yes |
| Checked-in native-vs-reference-pack comparison | Yes | Yes |
| Real supported `kicad-cli` matrix | One pinned baseline on relevant PRs | Full supported matrix nightly |
| Altium Designer oracle regeneration | No; proprietary/manual controlled runner | Before Altium release and parser updates |
| Fuzz and dense-board/resource tests | Fuzz-lite/capped set | Extended nightly |

## Risks and unknowns

| Risk | Response |
|---|---|
| Board file does not identify the fab's actual output options | Name/hash the profile; never claim unknown OutJob/history parity; decide defaults before code. |
| KiCad external dependency breaks self-contained distribution | Keep fab-pack path first-class, document/install/pin CLI, and make absence actionable. Do not bundle GPL code. |
| KiCad CLI behavior changes by major/minor | Capability probe, tested allow-list, producer matrix, profile parity, fail on unknown majors. |
| KiCad wasm cannot spawn CLI | Explicit unsupported message; direct plotter is separate future work if owner makes browser support mandatory. |
| Altium format is proprietary and reverse-engineered | Experimental feature, exact dependency pin, record accounting, independent producer oracle, narrow supported subset. |
| `altium-format` is pre-1.0 and undergoing a rewrite | Spike before adoption, wrapper boundary, pinned version, owner go/no-go, no stable claim based on parsing alone. |
| Saved Altium pours may be absent or stale | Do not use polygon outlines as copper; detect/reject; defer repour. |
| Text/font output differs from producer | Supported-font subset, no silent fallback, parity corpus, reject unresolved fonts/variables. |
| Geometry amplification or hostile CFB/S-expression input | Bound bytes, streams, records, points, glyphs, process output/time, and final polygons before allocation where possible. |
| Public Altium corpus is too small | Stable release waits; seek donated redistributable fixtures and retain proprietary oracle regeneration on controlled runners. |
| AGPL contamination from `altium-monkey` | No linking, vendoring, translation, or distribution; use only independent black-box validation after legal review. |

## Documentation checklist for each release

1. README input matrix and install/runtime prerequisites.
2. CLI reference for detection, profiles, flags, exit codes, and examples.
3. Native GUI and browser capability matrix.
4. GitHub Action setup with pinned producer/parser versions.
5. Trust/limitations page covering zone/pour state, fonts, profiles, and
   unsupported producer versions.
6. Supported-version and export-profile tables with hashes.
7. Corpus provenance and oracle regeneration guide.
8. Third-party licence/notice update and `cargo deny` evidence.
9. Website examples based on a public board, with both native and exported-pack
   reproduction commands.
10. Changelog migration notes for profile/schema/feature-flag changes.
