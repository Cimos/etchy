//! Polygonize the primitive IR into filled integer contours ([`PolygonSet`]).
//!
//! This increment fills `Flash` of circle (→ 64-gon) and rect (→ 4 corners)
//! apertures. `Line`/`Arc`/`Region` are the documented growth seam: they return a
//! loud [`EngineError::Unsupported`] today and become stroke/tessellate/fill in
//! the next increment, behind the same fail-loud guarantee.

use std::f64::consts::PI;

use crate::error::{EngineError, Result};
use crate::geo::{Aperture, Contour, PolygonSet, Primitive, Pt, Shape};

/// Segments approximating a circular aperture. 64 keeps the n-gon area within
/// ~0.13% of the true circle and matches the golden-corpus ground-truth maths.
pub const CIRCLE_SEGMENTS: usize = 64;

/// Turn a layer's primitives into its filled geometry.
pub fn polygonize(prims: &[Primitive]) -> Result<PolygonSet> {
    let mut shapes: Vec<Shape> = Vec::with_capacity(prims.len());
    for p in prims {
        match p {
            Primitive::Flash { at, aperture } => shapes.push(vec![flash_contour(*at, aperture)]),
            Primitive::Line { .. } => return Err(unsupported("stroked line (D01)")),
            Primitive::Arc { .. } => return Err(unsupported("arc (G02/G03)")),
            Primitive::Region { .. } => return Err(unsupported("region fill (G36/G37)")),
        }
    }
    Ok(PolygonSet::new(shapes))
}

fn unsupported(feature: &str) -> EngineError {
    EngineError::Unsupported {
        feature: feature.to_string(),
    }
}

/// A flash → its filled outer contour (CCW).
fn flash_contour(at: Pt, ap: &Aperture) -> Contour {
    match ap {
        Aperture::Circle { diameter_nm } => circle_ngon(at, diameter_nm / 2),
        Aperture::Rect { w_nm, h_nm } => {
            let (hw, hh) = (w_nm / 2, h_nm / 2);
            vec![
                Pt::new(at.x - hw, at.y - hh),
                Pt::new(at.x + hw, at.y - hh),
                Pt::new(at.x + hw, at.y + hh),
                Pt::new(at.x - hw, at.y + hh),
            ]
        }
    }
}

/// CCW regular n-gon inscribed in radius `r_nm` at `center`. Derived geometry, so
/// rounding the trig to the nm grid is deterministic and exact-per-run.
fn circle_ngon(center: Pt, r_nm: i64) -> Contour {
    let r = r_nm as f64;
    (0..CIRCLE_SEGMENTS)
        .map(|k| {
            let a = 2.0 * PI * (k as f64) / (CIRCLE_SEGMENTS as f64);
            Pt::new(
                center.x + (r * a.cos()).round() as i64,
                center.y + (r * a.sin()).round() as i64,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flash_circle_makes_one_ngon_shape() {
        let ps = polygonize(&[Primitive::Flash {
            at: Pt::new(0, 0),
            aperture: Aperture::Circle {
                diameter_nm: 500_000,
            },
        }])
        .unwrap();
        assert_eq!(ps.region_count(), 1);
        assert_eq!(ps.shapes[0][0].len(), CIRCLE_SEGMENTS);
        assert!(ps.area_nm2() > 0);
    }

    #[test]
    fn line_fails_loud() {
        let err = polygonize(&[Primitive::Line {
            from: Pt::new(0, 0),
            to: Pt::new(1, 1),
            aperture: Aperture::Circle { diameter_nm: 1 },
        }])
        .unwrap_err();
        assert!(matches!(err, EngineError::Unsupported { .. }));
    }
}
