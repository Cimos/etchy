# Native board ingestion research

Research date: 2026-10-03. This document records only behavior checked in the
listed sources or commands. A failed network check is called out rather than
filled in from memory.

## Recommendation summary

| Format | Recommended route | Reason |
|---|---|---|
| KiCad `.kicad_pcb` | Run a supported `kicad-cli` as a child process, export a temporary Gerber/Excellon/position pack, then use etchy's existing loaders. | KiCad itself performs zone fill, pad/footprint expansion, plotting, text shaping, layer selection, and version migration. This is the shortest path to the same geometry as a KiCad fabrication export. |
| Altium `.PcbDoc` | Add a feature-gated, pure-Rust adapter around the permissive `altium-format` crate, with an etchy-owned manufacturing polygonizer and strict unsupported-record accounting. | It preserves etchy's licence, static binaries, CI use, and a possible wasm path. `altium-monkey` is capable but Python plus AGPL-3.0-or-later prevents bundling it into etchy's permissive binaries. |

These routes do not have equal maturity. KiCad can ship once CLI-version and
export-profile tests pass. Altium should remain experimental until its native
geometry matches independent Altium Gerber exports over an agreed corpus.

## Existing etchy constraints checked

- The current scope says native KiCad is accepted after 1.0, while native
  Altium is still excluded: [`docs/REQUIREMENTS.md`](../../../docs/REQUIREMENTS.md),
  `SCOPE-4` and `SCOPE-5`; [`docs/ROADMAP.md`](../../../docs/ROADMAP.md),
  “Beyond 1.0”. Issue #122 could not be fetched: `gh issue view 122 -R
  Cimos/etchy --json ...` returned `error connecting to api.github.com` on
  2026-10-03. The checked-in requirements and roadmap both record its accepted
  decision, so this report uses those local sources rather than inventing issue
  text.
- The trust rule is “no silent misses”, with typed errors in preference to
  partial output: [`docs/TRUST.md`](../../../docs/TRUST.md) and `TRUST-1`,
  `TRUST-4`, `TRUST-5`, and `TRUST-7` in
  [`docs/REQUIREMENTS.md`](../../../docs/REQUIREMENTS.md).
- `etchy-core::Board` is a vector of resolved `Layer` values. Each layer has a
  `LayerKind`, opaque label, fixed-point `PolygonSet`, and polarity; the engine
  pairs by `LayerKind`: [`crates/etchy-core/src/model.rs`](../../../crates/etchy-core/src/model.rs).
  Filesystem and loading policy currently live in the CLI and GUI:
  [`crates/etchy-cli/src/main.rs`](../../../crates/etchy-cli/src/main.rs) and
  [`crates/etchy-gui/src/loader.rs`](../../../crates/etchy-gui/src/loader.rs).
- etchy allows only permissive dependency licences. GPL, AGPL, LGPL, and MPL
  are deliberately absent from the allow-list:
  [`deny.toml`](../../../deny.toml). KiCad says most program source is
  GPL-3.0-or-later ([KiCad licences](https://www.kicad.org/about/licenses/)).
  Calling an independently installed executable does not put that code into
  etchy's linked dependency tree. Copying, translating, or linking KiCad's
  plotting implementation would conflict with etchy's stated licensing rule
  and is not proposed.

## KiCad

### Route A: `kicad-cli` export bridge

#### Checked installation and commands

Commands run locally on 2026-10-03:

```text
$ kicad-cli version
7.0.11

$ kicad-cli pcb export --help
{drill,dxf,gerber,gerbers,pdf,pos,step,svg}
```

`kicad-cli pcb export gerbers --help` on 7.0.11 exposes layer selection,
reference/value exclusion, border/title inclusion, X2 and netlist switches,
solder-mask subtraction, aperture-macro control, drill-origin use, precision,
Protel extensions, common layers, and `--board-plot-params` (“Use the gerber
plot settings already configured in the board file”). `drill --help` exposes
Excellon/Gerber output, origin, zero format, oval/route choice, units, mirror,
minimal header, PTH separation, and map options. `pos --help` exposes side,
ASCII/CSV/Gerber, units, bottom-X convention, origin, SMD-only, through-hole
footprint exclusion, and board edge. The complete captured output came from
those local commands; the current official CLI reference independently lists
the commands and the saved-plot-settings flag ([KiCad CLI reference](https://docs.kicad.org/master/en/cli/cli.pdf)).

Version differences matter. KiCad says the CLI first shipped in version 7
([KiCad 8 release note](https://www.kicad.org/blog/2024/02/Version-8.0.0-Released/)).
The current CLI adds `--check-zones`, which checks saved fill data and refills
when needed without saving the board, plus newer plotting options not present
in the installed 7.0.11 help ([current CLI reference](https://docs.kicad.org/master/en/cli/cli.pdf)).
Therefore the bridge must probe capabilities, not assume one command line works
on every major version.

#### Geometry coverage

| Concern | What the bridge does | Evidence and consequence |
|---|---|---|
| Zones/fills | KiCad plots its saved filled polygons; on versions with `--check-zones`, etchy can request refill before export. KiCad 7 lacks that flag, so stale or absent fill data cannot safely be repaired headlessly by this command. | The board format says fill polygons are optional and absent when a zone has not been filled ([board format, Zone](https://dev-docs.kicad.org/en/file-formats/sexpr-pcb/)). The current CLI documents `--check-zones`. KiCad's plotter sends filled zone outlines as Gerber regions ([plotter source, 9.0.7](https://gitlab.com/kicad/code/kicad/-/blob/9.0.7/pcbnew/plot_brditems_plotter.cpp?ref_type=tags)). Result: require KiCad 8+ only if the probed help includes `--check-zones`; otherwise fail on zones unless a validated saved-fill policy is deliberately accepted. |
| Footprints and pads | KiCad expands embedded footprint graphics and pads using the same plotter used for fabrication output. | The board format embeds footprint graphics and pads and records footprint placement ([board format](https://dev-docs.kicad.org/en/file-formats/sexpr-pcb/)); using the producer avoids reimplementing transforms and pad rules. |
| Tracks, arcs, graphics | KiCad plots them to Gerber; etchy then parses the Gerber it already supports. | `gerbers` is the documented fabrication command; its one-file-per-layer output is described as the usual fabrication choice ([CLI reference](https://docs.kicad.org/master/en/cli/cli.pdf)). |
| Silk text/fonts | KiCad performs text substitution and font rendering. This is stronger than a direct parser, especially for version 7+ TrueType `face` values. | The common S-expression format added the optional TrueType font family in version 7 ([S-expression format](https://dev-docs.kicad.org/en/file-formats/sexpr-intro/)). A KiCad 8.0.1 CLI issue demonstrates that project-variable substitution has itself varied/failed in a released CLI ([KiCad issue 17732](https://gitlab.com/kicad/code/kicad/-/work_items/17732)); parity tests must include variables and text. |
| Board outline | Plot `Edge.Cuts` as its own Gerber and let existing X2/filename classification map it to `LayerKind::Outline`. | KiCad layer names are internal names with optional user-facing names ([board format, Layers](https://dev-docs.kicad.org/en/file-formats/sexpr-pcb/)); passing internal layer names avoids localized/display-name ambiguity. |
| Layer names | Discover the board's enabled internal layers, pass those names to `--layers`, and classify exported X2 `.FileFunction` plus filenames through existing etchy logic. | etchy already reconciles X2 attributes and filenames in [`crates/etchy-core/src/naming.rs`](../../../crates/etchy-core/src/naming.rs). The installed CLI help calls `F.Cu,B.Cu` “untranslated layer names”. |
| Drill and placement | Run separate `drill` and `pos` exports into the same temporary pack. | Verified in installed 7.0.11 help. etchy already resolves Excellon and placement geometry. |

#### Fidelity and operational trade-offs

- Highest available fidelity to KiCad output, but only relative to an explicit
  export profile. A `.kicad_pcb` contains last-used plot settings, and
  `--board-plot-params` applies them. Different layer lists, origins, text
  exclusions, mask subtraction, zone state, or project variables produce
  different fabrication packs. “What a real fab received” is not recoverable
  from the board alone if different settings were used.
- Requires `kicad-cli` on native desktop and CI. GitHub-hosted runners must
  install an approved KiCad major or use an etchy Action/container variant that
  contains it. The present composite Action only builds etchy from source and
  does not install KiCad: [`action.yml`](../../../action.yml).
- Cannot run in browser wasm because wasm cannot spawn a native executable.
  A server conversion service would change the current local/offline privacy
  model and is not recommended in this feature.
- etchy's static binary remains static, but the feature is not self-contained:
  KiCad stays an external runtime dependency and is much larger than etchy.
- Temporary files must use a private directory, be removed after loading, and
  be bounded before parsing. Both exports must use the same probed KiCad binary
  and profile.

### Route B: direct Rust S-expression parse

The file syntax is public and approachable, but a syntax parser is only the
first step. Producing fabrication geometry also requires version-aware object
semantics, pad stacks, footprint transforms, mask/paste expansions, zone fill,
text shaping, plot settings, and layer/output rules.

The official board documentation covers KiCad 6 onward, uses a date-valued file
version, records fixed 1 nm coordinate resolution, and notes several format
transitions: pre-6 strings were quoted differently, pre-6 footprints were
called `module`, and version 7 introduced the `face` font field
([board format](https://dev-docs.kicad.org/en/file-formats/sexpr-pcb/),
[common S-expression format](https://dev-docs.kicad.org/en/file-formats/sexpr-intro/)).
KiCad 8 added IPC-2581 export and followed the version-7 CLI introduction
([KiCad 8 release note](https://www.kicad.org/blog/2024/02/Version-8.0.0-Released/)).
KiCad 9 added more output behavior and deprecated the singular `gerber` command
in favor of `gerbers` ([current CLI reference](https://docs.kicad.org/master/en/cli/cli.pdf)).
These checked sources do not provide a complete 6→7→8→9 token changelog, so no
more detailed version claim is made here.

#### Permissive Rust candidates checked

The local `cargo search` and `cargo info` attempts failed because crates.io DNS
was unavailable. Versions and scope below come from the linked crate/repository
pages checked on 2026-10-03.

| Crate | Checked version/licence/activity | Maturity for etchy's geometry job |
|---|---|---|
| [`kiutils-rs`](https://docs.rs/kiutils-rs/latest/kiutils_rs/) | 0.2.0; MIT; repository describes 99 commits, alpha status, and KiCad 10 primary / 9 secondary support ([repository](https://github.com/Milind220/kiutils-rs)). Integration and CLI tests are linked from its API docs. | Good lossless syntax/typed-document base with unknown-node capture, but its stated compatibility does not cover KiCad 6–8 and its public PCB types shown in the docs do not amount to a fabrication polygonizer. Not ready as the sole ingestion path. |
| [`kicad_parse_gen`](https://docs.rs/crate/kicad_parse_gen/latest) | 7.0.2; manifest says `MIT/Apache-2.0`; last published 2018-01-29 according to the package-history page ([manifest](https://docs.rs/crate/kicad_parse_gen/latest/source/Cargo.toml.orig), [history](https://socket.dev/cargo/package/kicad-parse-gen)). Docs expose layout, footprint, schematic, and project modules; the package includes tests. | Old API/dependencies and pre-KiCad-6 age make current format completeness and modern rendering behavior unsuitable. Test presence is visible; coverage percentage was not published and was not verified. |
| [`kiparse`](https://github.com/Atlantix-EDA/KiParse) | README shows 0.1.0, MIT, 12 commits; the old repository was archived 2025-12-10 and moved into `atlantix-eda`. It tests one 250k-line FPGA board and claims KiCad 6–9 format support in the parent repository. | Its own support table says PCB layer extraction plus regex-based component extraction, while full support is a 1.0 roadmap item. It does not cover zones, pad rendering, silk fonts, or fabrication plotting, so it is not sufficient. |
| [`kicad-parser`](https://docs.rs/crate/kicad-parser/latest) | Search result showed 0.0.10 and a 400.6 kB source package. Its licence, release date, maintenance history, geometry completeness, and test coverage could not be verified because the crate/source page did not load. | Unverified facts must be treated as a rejection under etchy's trust bar. |

Direct parsing is the only fully local route that could serve wasm and preserve
a self-contained static binary. Its cost is effectively a second PCB plotter,
not just an S-expression decoder. Filled versus unfilled zones are the largest
immediate trap: using zone outlines would overstate copper, while accepting
missing saved fill polygons would miss copper. A correct direct route must
either implement KiCad's refill rules or reject boards whose saved fill data is
absent/stale; no surveyed Rust crate establishes that behavior.

### Route C: Python/SWIG or IPC API

- KiCad's SWIG Python PCB bindings are deprecated as of KiCad 9 and planned for
  removal in KiCad 11. The official developer docs direct new work to the IPC
  API ([APIs and bindings](https://dev-docs.kicad.org/en/apis-and-binding/index.html)).
  They add a Python/runtime and KiCad-version coupling without improving
  headless export fidelity over `kicad-cli`.
- The IPC API is language-neutral. The official status page says KiCad 9 and 10
  require a running GUI, cannot plot/export, and support only the PCB editor;
  headless CLI hosting and export arrive in KiCad 11
  ([IPC API limitations](https://dev-docs.kicad.org/en/apis-and-binding/ipc-api/for-addon-developers/)).
  That makes it worse than the CLI for present CI and native ingestion.
- `kicad-ipc-rs` is MIT and pins generated protocol code to KiCad 10.0.1
  ([repository](https://github.com/Milind220/kicad-ipc-rs)), but the server-side
  limitations above remain. It cannot serve wasm without a running local KiCad
  GUI and cannot export on KiCad 9/10.

### KiCad comparison

| Route | Fab-output fidelity | Runtime | wasm | GitHub Action | Static distribution | Decision |
|---|---|---|---|---|---|---|
| `kicad-cli` export bridge | Highest; producer's plotter, fonts, pads, zones, and version reader | KiCad CLI required | No; fail loud | Install/pin KiCad | etchy stays static, feature not self-contained | **Recommend** |
| Direct Rust parser + new plotter | Initially low/unknown; must reproduce many plot rules | None | Possible | Good | Good | Do not ship until independent parity work exists |
| Python/SWIG | KiCad model access, but deprecated and still needs plot logic/API calls | KiCad + Python | No | Poor | Poor | Reject |
| KiCad 9/10 IPC | Stable transport but no plot/export and GUI must run | Running KiCad GUI | No | Poor | Poor | Reject for this feature; revisit KiCad 11 |

## Altium

### `altium-monkey` inspection

The local checkout was inspected at
`/home/madman/UbuntuProjects/altium_monkey`, commit
`8d8c1a03facb8c18b8e812cb673bfb5b45923c4a`, tag/version `v2026.8.21`, dated
2026-08-21 (`git log`, `git describe`, and
[`_version.py`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/src/py/altium_monkey/_version.py)).

| Question | Checked finding |
|---|---|
| Language/runtime | Python, requiring normal CPython 3.12–3.14. Dependencies include `jsonschema-rs`, `msgspec`, Pillow, FreeType, HarfBuzz, `wn-geometer`, lxml, and lz4: [`pyproject.toml`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/pyproject.toml). |
| Licence | `AGPL-3.0-or-later` in the package manifest and README; the local [`LICENSE`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/LICENSE) contains AGPL v3. |
| Container/file reader | Reads Altium OLE/compound documents directly through its own OLE layer; parsing does not launch Altium. The public `AltiumPcbDoc` parser and typed lists are described in [`docs/pcbdoc.md`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/docs/pcbdoc.md) and implemented in [`altium_pcbdoc.py`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/src/py/altium_monkey/altium_pcbdoc.py). |
| PCB primitives | Exposes components, tracks, arcs, pads, vias, texts, fills, regions, polygon-pour definitions, nets, classes, layer-stack data, custom shapes, component bodies, and board outline. The constructor's typed lists and stream parsing are in [`altium_pcbdoc.py`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/src/py/altium_monkey/altium_pcbdoc.py); the public list is in [`docs/pcbdoc.md`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/docs/pcbdoc.md). |
| Pours | `Polygons6/Data` supplies pour definitions; rendered fill geometry is represented by linked tracks/arcs/regions and shape-based regions. The SVG renderer hides definition overlays by default and renders linked primitives: [`altium_pcb_svg_renderer.py`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/src/py/altium_monkey/altium_pcb_svg_renderer.py). The IPC writer resolves parent polygon nets/shelving and emits regions/primitives: [`altium_pcb_ipc2581_writer.py`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/src/py/altium_monkey/altium_pcb_ipc2581_writer.py). This supports saved pours; it is not evidence that the package can repour an arbitrary stale/unpoured board. |
| Footprints | Components expose resolved designator, footprint, placement, rotation, side, and parameters; component-owned primitives remain available. See [`docs/pcbdoc.md`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/docs/pcbdoc.md). |
| Rendering | Can render a composed PCB or one SVG per layer; text defaults to polygon output and the package includes Altium stroke-font assets. See `to_svg`, `to_layer_svgs` in [`altium_pcbdoc.py`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/src/py/altium_monkey/altium_pcbdoc.py) and options in [`altium_pcb_svg_renderer.py`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/src/py/altium_monkey/altium_pcb_svg_renderer.py). SVG is a viewer rendering, not verified Gerber output. |
| Manufacturing export without Altium | It contains a direct IPC-2581B writer (`write_ipc2581`) in [`altium_pcb_ipc2581_writer.py`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/src/py/altium_monkey/altium_pcb_ipc2581_writer.py). It can create Gerber/ODB++/IPC-2581 OutJob definitions, but running an OutJob explicitly launches Altium Designer: [`altium_outjob.py`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/src/py/altium_monkey/altium_outjob.py), [`docs/prjpcb.md`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/docs/prjpcb.md), and [`altium_outjob_runner.py`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/src/py/altium_monkey/altium_outjob_runner.py). No direct Gerber or ODB++ writer was found. |
| Altium installation | Not needed to parse, inspect, render SVG, or call the package's IPC-2581 writer. It is needed for the OutJob runner and therefore for Altium's own Gerber/ODB++/IPC-2581 export. |
| Validation limits | README says it is tested against a large private corpus spanning “Summer '08” to the present, but that corpus is not public; Windows is the primary validation platform and Linux coverage is limited: [`README.md`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/README.md). Public test files exist under [`tests/`](https://github.com/wavenumber-eng/altium_monkey/blob/8d8c1a03facb8c18b8e812cb673bfb5b45923c4a/tests), but no published coverage percentage was found. |

#### Can etchy depend on it?

Not as a linked, vendored, translated, or bundled dependency while etchy remains
`MIT OR Apache-2.0`. It is Python rather than Rust, and its AGPL licence is
blocked by etchy's policy. FFI would combine runtimes and does not solve either
distribution or licence concerns. Rewriting its implementation into Rust is
also not an acceptable clean-room path.

An optional child-process integration with a separately installed
`altium-monkey` could exchange a documented JSON/IPC-2581 file and keep process
boundaries clear, but etchy should obtain legal review before presenting that
as a supported licence arrangement. Technically it would still require Python
3.12–3.14 and native wheels, could not be inside etchy's static binary, and
could not run in browser wasm. It is therefore not the recommended product
path. The project remains valuable as an independent behavioral oracle and as
evidence about the breadth of the format.

### Other checked options

| Option | Version/licence/scope checked | Assessment |
|---|---|---|
| [`altium-format`](https://docs.rs/altium-format/latest/altium_format/io/pcbdoc/struct.PcbDoc.html) from [`altium-cli`](https://github.com/akiselev/altium-cli) | docs.rs shows 0.1.7 and typed PcbDoc components plus tracks, vias, pads, arcs, fills, regions, polygons, text, nets, and rules. Repository licence is Apache-2.0, has 378 commits, says it tests against thousands of public files, reads only CFB documents, and warns of breaking changes and subtle bugs. Gerber output and PCB rendering are roadmap items; the README says a large 0.2 rewrite was unreleased as of 2026-02-24. | Best permissive Rust starting point, but pinning 0.1.7 does not remove maturity risk. etchy must add its own strict adapter, geometry, fixtures, audits, and wasm build proof. |
| [`AltiumSharp`](https://github.com/issus/AltiumSharp) | The checked third-party survey identifies it as C#/.NET, Apache-2.0, with PCB primitives and layer/rule coverage ([survey](https://github.com/IntelligentElectron/pcb-lens/blob/main/plans/altium-pcbdoc-parser-research.md)). | Licence is suitable, language/runtime is not. Porting is a large maintenance burden; direct .NET shell-out defeats static/wasm goals. No independent code audit was performed here. |
| [`PyAltium`](https://github.com/pluots/PyAltium) | Python, GPLv3; README says broken/incomplete and PcbDoc has no functioning list/display/write support. | Reject. |
| [`altium-tools`](https://github.com/stuart-minion-ai/altium-tools) | Python, Apache-2.0, no Altium install; its roadmap says PCB tracks/vias/pads/polygons still need decoding. | Too incomplete. |
| [`altiumts`](https://github.com/tscircuit/altiumts) | TypeScript; experimental binary reader and browser viewer with typed models, layer stacks, board geometry, SVG, and bounded CFB. Licence was not stated in the checked page and was not independently verified. | Interesting wasm oracle, but not a Rust dependency and licence remains unverified for this report. |

### Altium geometry implications

- Saved polygon pour geometry must be used, not the polygon boundary alone.
  If linked fill primitives/regions are absent, inconsistent, or use an
  unsupported form, ingestion must stop and name the polygon/layer. etchy must
  not attempt a partial repour in its first release.
- Components are ownership/placement records; their pads and graphics must be
  transformed to board coordinates exactly once. Free board primitives and
  component-owned primitives both contribute.
- Copper includes tracks, arcs, pads, vias, fills, regions, and saved pour
  primitives. Mask/paste can be explicit or rule-expanded around pads/vias;
  unsupported rule precedence must fail rather than use a guessed default.
- Silk text fidelity depends on Altium stroke/TrueType metrics and special-text
  substitution. The pure-Rust adapter needs a declared supported subset and
  must reject unresolved fonts/special strings if their geometry can affect a
  compared layer.
- Board outline and cutouts may be assembled from board metadata and primitives;
  malformed or open outlines must be reported rather than silently closed.
- Altium's older numeric layer IDs and newer layer mappings need to resolve to
  etchy's top/inner/bottom, mask, silk, paste, drill, outline, documentation,
  and placement kinds. KiCad's official Altium-import format notes CFB streams
  such as `Board6`, `Components6`, and versioned layer IDs
  ([KiCad Altium format notes](https://dev-docs.kicad.org/en/import-formats/altium/index.html)).

### Altium comparison

| Route | Fab-output fidelity | Runtime | wasm | GitHub Action | Static distribution | Decision |
|---|---|---|---|---|---|---|
| Pure Rust `altium-format` adapter + etchy polygonizer | Unknown until parity corpus; can use saved primitive/pour geometry but must reproduce plotting rules | None | Plausible; must prove dependency build and memory caps | Good | Good | **Recommend, feature-gated and experimental first** |
| `altium-monkey` subprocess | Broad reader, SVG and IPC-2581 output; not direct Altium Gerber | Python 3.12–3.14 + packages | No | Setup-heavy | No | Reject as product dependency; use as independent oracle only |
| Altium OutJob through `altium-monkey` | Highest fidelity to Altium output | Windows + licensed Altium + Python | No | Impractical on hosted runners | No | Validation oracle, not user path |
| Other Rust/Python readers | Either alpha, incomplete, or unverified | Varies | Varies | Varies | Varies | No stronger checked option |

## Fidelity boundary: what “same as Gerber” can mean

A board document is not always the complete manufacturing-output recipe.
KiCad stores plot settings and offers `--board-plot-params`; an Altium project
may keep fabrication choices in a separate `.OutJob`. Therefore this feature
can make a testable promise only against a named export profile:

1. KiCad: the board's saved plot parameters, with etchy's documented layer,
   drill, position, zone-check, origin, and precision overrides.
2. Altium: etchy's documented native manufacturing projection from saved board
   geometry. It cannot claim equality to an unknown OutJob.
3. Every report records source format, adapter/parser version, producer version
   where applicable, profile name/hash, warnings, and unsupported-record count.
4. “Same” means equal after both paths enter etchy's existing fixed-point
   `Board` model and normal polygon-diff tolerance; validation compares each
   native projection against Gerber/Excellon/position output made with that
   exact profile.

This limitation is the largest product question for the owner: whether a simple
two-file invocation should use a fixed etchy profile, saved project settings,
or require an explicit export-profile input whenever settings are ambiguous.
