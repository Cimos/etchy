# Filename classification corpus

Each `<scheme>.txt` lists fab-pack filenames from one EDA tool's default output
with the layer kind `etchy_core::classify` must give them (IN-6, #426):

```text
<filename> => <kind>
```

`<kind>` is `LayerKind::kind_str()`, except inner copper, which carries its
ordinal (`inner-copper-1` is the first layer under the top). `#` lines are
comments; each file cites its sources. Names whose layer can't be told from the
name alone (plane layers, Eagle `.ly<n>`, `.drd`) are listed as `other` on
purpose: they are never guessed. Excellon drills and placement files are also
routed by content when a board is loaded, so a name listed as `other` here may
still load as a drill.

`altium.txt` and `kicad.txt` are baselines: they must not change when a new
scheme lands. `crates/etchy-core/tests/naming_corpus.rs` checks every file.
