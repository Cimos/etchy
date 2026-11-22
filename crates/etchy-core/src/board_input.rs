//! Build a board from named in-memory fab files: classify by filename, sniff the
//! content, and route each file to the Gerber, Excellon or placement front-end.
//! Shared by the CLI and the viewer so both load a board the same way (#436).
//! Reading files, ordering them and size limits belong to the callers.

use std::sync::Arc;

use crate::{
    classify, drill_kind, file_function, file_polarity, gerber_format, looks_like_excellon,
    looks_like_gerber, looks_like_placement, reconcile_kind, resolve_excellon_detailed,
    resolve_layer_detailed, resolve_placement, Board, EngineError, FilePolarity, GerberFormat,
    Layer, LayerKind, Result,
};

/// Incremental form of [`board_from_bytes`], for callers that enforce a
/// per-file limit before each file is parsed.
#[derive(Default)]
pub struct BoardBuilder {
    layers: Vec<Layer>,
    fmt: Option<GerberFormat>,
    warnings: Vec<String>,
}

impl BoardBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse one named file, or skip it if its content is none of Gerber,
    /// Excellon or placement (a job file, a README). On error the builder is left
    /// as it was, so a caller may skip the file and carry on.
    pub fn push(&mut self, name: String, bytes: Vec<u8>) -> Result<()> {
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name.as_str(), ""));
        let mut kind = classify(stem, ext);
        let mut negative = false;
        let mut x2_objects = None;
        let mut drill_tools = None;
        let mut fmt = None;
        let mut warnings = Vec::new();
        let geometry = if looks_like_gerber(&bytes) {
            if self.fmt.is_none() {
                fmt = gerber_format(&bytes).ok();
            }
            // A negative image (#317): the objects are clearances. The engine swaps
            // added/removed for such a layer so the report still speaks in copper;
            // say so, since the overlay draws the objects as exported.
            if file_polarity(&bytes) == Some(FilePolarity::Negative) {
                negative = true;
                warnings.push(format!(
                    "{name}: negative-polarity image (%TF.FilePolarity,Negative) — \
                     added/removed are reported as material, not as drawn objects"
                ));
            }
            // Cross-check the filename classification against the file's own X2
            // `.FileFunction` attribute (#239): adopt it where the filename was
            // unrecognized, warn on a genuine conflict, never a silent override.
            let (reconciled, warn) = reconcile_kind(kind, file_function(&bytes), &name);
            kind = reconciled;
            warnings.extend(warn);
            let resolved =
                resolve_layer_detailed(&bytes).map_err(|source| EngineError::Processing {
                    kind: "layer",
                    name: name.clone(),
                    inner: Box::new(source),
                })?;
            x2_objects = resolved.labels.map(Arc::new);
            // A drill layer exported as Gerber gets a drill table from its circle
            // flashes, so it compares with an Excellon revision (#457). Anything
            // else on the layer (slots drawn as D01, regions, other apertures)
            // can't be counted, so the layer gets no table, and says why.
            if matches!(kind, LayerKind::Drill(_)) {
                if negative {
                    warnings.push(format!(
                        "{name}: negative-polarity Gerber drill layer — no drill table for it"
                    ));
                } else {
                    match resolved.flashes.into_drill_tools() {
                        Ok(tools) => drill_tools = Some(tools),
                        Err(why) => warnings.push(format!(
                            "{name}: Gerber drill layer has {why}, which a drill table can't \
                             count (only circle flashes are holes) — no drill table for it"
                        )),
                    }
                }
            }
            Arc::new(resolved.geometry)
        } else if looks_like_excellon(&bytes) {
            // Content wins over the filename: an Excellon file named e.g. Board.TXT
            // must land on the Drill layer, not "other", or `--gate-layers drill`
            // would miss a real drill change. Plating (PTH/NPTH) still comes from
            // the filename, so `board-NPTH.txt` keeps its plating (#237).
            kind = LayerKind::Drill(drill_kind(stem));
            let drill =
                resolve_excellon_detailed(&bytes).map_err(|source| EngineError::Processing {
                    kind: "drill layer",
                    name: name.clone(),
                    inner: Box::new(source),
                })?;
            drill_tools = Some(drill.tools);
            // An assumed digit format (TZ, no format line, #429) is a warning.
            warnings.extend(drill.warnings.into_iter().map(|w| format!("{name}: {w}")));
            Arc::new(drill.geometry)
        } else if looks_like_placement(&bytes) {
            kind = LayerKind::Placement;
            Arc::new(
                resolve_placement(&bytes).map_err(|source| EngineError::Processing {
                    kind: "placement file",
                    name: name.clone(),
                    inner: Box::new(source),
                })?,
            )
        } else {
            return Ok(());
        };
        if fmt.is_some() {
            self.fmt = fmt;
        }
        self.warnings.append(&mut warnings);
        self.layers.push(Layer {
            kind,
            label: name,
            geometry,
            negative,
            x2_objects,
            drill_tools,
        });
        Ok(())
    }

    pub fn finish(self) -> (Board, Option<GerberFormat>, Vec<String>) {
        (
            Board {
                layers: self.layers,
            },
            self.fmt,
            self.warnings,
        )
    }
}

/// Process files in the supplied order, skipping unsupported content. Callers
/// choose their own ordering and enforce their own input limits before this call.
pub fn board_from_bytes(
    files: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> Result<(Board, Option<GerberFormat>, Vec<String>)> {
    let mut builder = BoardBuilder::new();
    for (name, bytes) in files {
        builder.push(name, bytes)?;
    }
    Ok(builder.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_drill_retains_tool_counts() {
        let bytes = b"M48\nMETRIC,TZ\nT1C0.500\n%\nT1\nX10.0Y10.0\nM30\n".to_vec();
        let (board, _, warnings) = board_from_bytes([("board-PTH.drl".into(), bytes)]).unwrap();
        assert!(board.layers[0].drill_tools.is_some());
        assert!(warnings.is_empty());
    }

    #[test]
    fn gerber_drill_layer_gets_a_table_from_its_flashes() {
        // #457: X2 says plated drill, so the circle flashes become holes.
        let bytes = b"%FSLAX46Y46*%\n%MOMM*%\n%TF.FileFunction,Plated,1,2,PTH,Drill*%\n\
%ADD10C,0.3*%\nD10*\nX0Y0D03*\nX1000000Y0D03*\nM02*\n"
            .to_vec();
        let (board, _, warnings) = board_from_bytes([("board-PTH-drl.gbr".into(), bytes)]).unwrap();
        let tools = board.layers[0].drill_tools.as_ref().unwrap();
        assert_eq!(tools.by_diameter_um[&300].holes, 2);
        assert!(
            warnings.iter().all(|w| !w.contains("drill table")),
            "{warnings:?}"
        );
    }

    #[test]
    fn gerber_drill_layer_with_a_slot_warns_and_has_no_table() {
        let bytes = b"%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.8*%\nD10*\nX0Y0D03*\n\
X0Y0D02*\nX2000000Y0D01*\nM02*\n"
            .to_vec();
        let (board, _, warnings) = board_from_bytes([("board-drill.gbr".into(), bytes)]).unwrap();
        assert!(matches!(board.layers[0].kind, LayerKind::Drill(_)));
        assert!(board.layers[0].drill_tools.is_none());
        assert!(
            warnings.iter().any(|w| w.starts_with("board-drill.gbr: ")
                && w.contains("1 draw (slot or route)")
                && w.contains("no drill table")),
            "{warnings:?}"
        );
    }

    #[test]
    fn gerber_copper_layer_has_no_drill_table() {
        let bytes = b"%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.3*%\nD10*\nX0Y0D03*\nM02*\n".to_vec();
        let (board, _, _) = board_from_bytes([("board-F_Cu.gbr".into(), bytes)]).unwrap();
        assert!(board.layers[0].drill_tools.is_none());
    }

    #[test]
    fn failed_push_leaves_the_builder_unchanged() {
        // A caller that skips a bad file (the web demo) must not keep warnings
        // or a format from it.
        let mut builder = BoardBuilder::new();
        let bad = b"%FSLAX46Y46*%
%TF.FilePolarity,Negative*%
%ADD10C,0.5*%
D10*
X0Y0D03*
M02*
";
        assert!(builder.push("bad.gbr".into(), bad.to_vec()).is_err());
        let (board, fmt, warnings) = builder.finish();
        assert!(board.layers.is_empty());
        assert!(fmt.is_none());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn parse_error_names_file_and_kind() {
        let bytes = b"%FSLAX46Y46*%\n%ADD10C,0.5*%\nD10*\nX0Y0D03*\nM02*\n".to_vec();
        let error = board_from_bytes([("bad.gbr".into(), bytes)]).unwrap_err();
        assert!(error.to_string().starts_with("processing layer bad.gbr: "));
    }
}
