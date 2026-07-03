//! Pure raster page-diff: two same-size RGBA images → a classified pixel diff
//! (added / removed / changed), a connected-region count, and a brand-coloured
//! overlay. Reused by `etchy-pdf` (schematic-PDF page diff) and any future raster
//! input.
//!
//! Purity (like the rest of `etchy-core`): no I/O and no PNG encode — a caller
//! rasterizes the pages and encodes the returned overlay. Classification mirrors
//! the geometry diff's language: on a light-background schematic, ink that appears
//! only in the new page is **added** (green), ink only in the old is **removed**
//! (red), and ink present in both but recoloured is **changed** (amber).
//!
//! Trust bar: two pages that don't line up pixel-for-pixel are a loud
//! [`EngineError::ImageSizeMismatch`] — etchy never rescales or realigns a raster.

use crate::error::{EngineError, Result};

/// An 8-bit RGBA raster, row-major, `rgba.len() == width*height*4`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// Construct, validating the buffer length against `width*height*4`.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        let expected = width as usize * height as usize * 4;
        if rgba.len() != expected {
            return Err(EngineError::MalformedImage {
                w: width,
                h: height,
                len: rgba.len(),
                expected,
            });
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}

/// Tunables for [`diff_images`].
#[derive(Clone, Copy, Debug)]
pub struct ImageDiffOptions {
    /// Luma at or below this is "ink" (foreground) on a light page. 0..=255.
    pub ink_threshold: u8,
    /// Per-channel absolute delta above this counts a same-ink pixel as *changed*.
    pub change_threshold: u8,
    /// Connected changed components smaller than this many pixels are dropped as
    /// noise (anti-aliasing shimmer, rescan speckle) — they count toward neither
    /// the region total nor the pixel tallies.
    pub min_region_px: u32,
}

impl Default for ImageDiffOptions {
    fn default() -> Self {
        Self {
            ink_threshold: 128,
            change_threshold: 24,
            min_region_px: 2,
        }
    }
}

/// Per-class pixel tallies + the connected-region count for one page pair.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageDiffStats {
    pub width: u32,
    pub height: u32,
    pub added_px: u64,
    pub removed_px: u64,
    pub changed_px: u64,
    pub total_px: u64,
    /// `(added+removed+changed) / total`.
    pub changed_fraction: f64,
    /// Connected components (4-connectivity) over the union changed mask, after
    /// the `min_region_px` filter.
    pub regions: u32,
}

/// The diff of one page pair: tallies plus a renderable overlay.
#[derive(Clone, Debug)]
pub struct ImageDiffResult {
    pub stats: ImageDiffStats,
    pub overlay: Image,
}

/// Classification of a single pixel between the two pages.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Unchanged,
    Added,
    Removed,
    Changed,
}

// Brand overlay colours (opaque). Green = added, red = removed, amber = changed;
// unchanged ink is a dim ghost on a board-dark background so real changes pop.
const OV_ADDED: [u8; 3] = [70, 209, 138];
const OV_REMOVED: [u8; 3] = [255, 93, 115];
const OV_CHANGED: [u8; 3] = [232, 163, 61];
const OV_BG: [u8; 3] = [11, 15, 14];
const OV_GHOST: [u8; 3] = [90, 96, 92];

#[inline]
fn luma(px: &[u8]) -> u8 {
    // Rec.601-ish integer luma; ignores alpha.
    ((px[0] as u32 * 54 + px[1] as u32 * 183 + px[2] as u32 * 19) >> 8) as u8
}

#[inline]
fn max_channel_delta(a: &[u8], b: &[u8]) -> u8 {
    let d = |i: usize| (a[i] as i32 - b[i] as i32).unsigned_abs();
    d(0).max(d(1)).max(d(2)) as u8
}

/// Diff two same-size RGBA pages. Fails loud on a size or buffer mismatch.
pub fn diff_images(old: &Image, new: &Image, opts: &ImageDiffOptions) -> Result<ImageDiffResult> {
    if old.width != new.width || old.height != new.height {
        return Err(EngineError::ImageSizeMismatch {
            ow: old.width,
            oh: old.height,
            nw: new.width,
            nh: new.height,
        });
    }
    let (w, h) = (old.width, new.height);
    let n = w as usize * h as usize;
    let expected = n * 4;
    for img in [old, new] {
        if img.rgba.len() != expected {
            return Err(EngineError::MalformedImage {
                w: img.width,
                h: img.height,
                len: img.rgba.len(),
                expected,
            });
        }
    }

    // Pass 1: classify every pixel.
    let mut class = vec![Class::Unchanged; n];
    for (i, c) in class.iter_mut().enumerate() {
        let a = &old.rgba[i * 4..i * 4 + 4];
        let b = &new.rgba[i * 4..i * 4 + 4];
        let ink_a = luma(a) <= opts.ink_threshold;
        let ink_b = luma(b) <= opts.ink_threshold;
        *c = if ink_b && !ink_a {
            Class::Added
        } else if ink_a && !ink_b {
            Class::Removed
        } else if max_channel_delta(a, b) > opts.change_threshold {
            Class::Changed
        } else {
            Class::Unchanged
        };
    }

    // Pass 2: label connected components over the changed mask (4-conn) and drop
    // any smaller than `min_region_px` — clearing them back to Unchanged so the
    // pixel tallies and the region count agree.
    let mut regions: u32 = 0;
    let mut visited = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..n {
        if visited[start] || class[start] == Class::Unchanged {
            continue;
        }
        // Flood this component, collecting its members.
        let mut members = Vec::new();
        visited[start] = true;
        stack.push(start);
        while let Some(p) = stack.pop() {
            members.push(p);
            let (x, y) = ((p % w as usize) as i64, (p / w as usize) as i64);
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                let q = ny as usize * w as usize + nx as usize;
                if !visited[q] && class[q] != Class::Unchanged {
                    visited[q] = true;
                    stack.push(q);
                }
            }
        }
        if (members.len() as u32) < opts.min_region_px {
            for m in members {
                class[m] = Class::Unchanged; // noise — drop it
            }
        } else {
            regions += 1;
        }
    }

    // Tally + render the overlay from the (noise-filtered) classes.
    let mut added_px = 0u64;
    let mut removed_px = 0u64;
    let mut changed_px = 0u64;
    let mut overlay = vec![0u8; expected];
    for i in 0..n {
        let (rgb, tally): ([u8; 3], Option<&mut u64>) = match class[i] {
            Class::Added => (OV_ADDED, Some(&mut added_px)),
            Class::Removed => (OV_REMOVED, Some(&mut removed_px)),
            Class::Changed => (OV_CHANGED, Some(&mut changed_px)),
            Class::Unchanged => {
                // Ghost the surviving ink; everything else is background.
                let ink = luma(&new.rgba[i * 4..i * 4 + 4]) <= opts.ink_threshold
                    || luma(&old.rgba[i * 4..i * 4 + 4]) <= opts.ink_threshold;
                (if ink { OV_GHOST } else { OV_BG }, None)
            }
        };
        if let Some(c) = tally {
            *c += 1;
        }
        overlay[i * 4] = rgb[0];
        overlay[i * 4 + 1] = rgb[1];
        overlay[i * 4 + 2] = rgb[2];
        overlay[i * 4 + 3] = 255;
    }

    let total_px = n as u64;
    let changed_fraction = if total_px == 0 {
        0.0
    } else {
        (added_px + removed_px + changed_px) as f64 / total_px as f64
    };
    Ok(ImageDiffResult {
        stats: ImageDiffStats {
            width: w,
            height: h,
            added_px,
            removed_px,
            changed_px,
            total_px,
            changed_fraction,
            regions,
        },
        overlay: Image {
            width: w,
            height: h,
            rgba: overlay,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // A w×h image filled white, with `ink` pixels (index → [r,g,b]) painted.
    fn img(w: u32, h: u32, ink: &[(usize, [u8; 3])]) -> Image {
        let mut rgba = vec![255u8; w as usize * h as usize * 4];
        for (i, c) in ink {
            rgba[i * 4] = c[0];
            rgba[i * 4 + 1] = c[1];
            rgba[i * 4 + 2] = c[2];
            rgba[i * 4 + 3] = 255;
        }
        Image::new(w, h, rgba).unwrap()
    }

    #[test]
    fn identical_pages_have_no_change() {
        let a = img(4, 4, &[(5, [0, 0, 0]), (6, [0, 0, 0])]);
        let r = diff_images(&a, &a, &ImageDiffOptions::default()).unwrap();
        assert_eq!(r.stats.added_px, 0);
        assert_eq!(r.stats.removed_px, 0);
        assert_eq!(r.stats.changed_px, 0);
        assert_eq!(r.stats.regions, 0);
        assert_eq!(r.stats.changed_fraction, 0.0);
        assert_eq!(r.overlay.width, 4);
        assert_eq!(r.overlay.rgba.len(), 4 * 4 * 4);
    }

    #[test]
    fn size_mismatch_fails_loud() {
        let a = img(4, 4, &[]);
        let b = img(4, 5, &[]);
        assert!(matches!(
            diff_images(&a, &b, &ImageDiffOptions::default()),
            Err(EngineError::ImageSizeMismatch {
                ow: 4,
                oh: 4,
                nw: 4,
                nh: 5
            })
        ));
    }

    #[test]
    fn added_and_removed_are_classified_and_counted() {
        // old ink at index 0; new ink at index 5 — one removed, one added.
        // min_region_px=1 so single-pixel marks survive.
        let old = img(4, 4, &[(0, [0, 0, 0])]);
        let new = img(4, 4, &[(5, [0, 0, 0])]);
        let opts = ImageDiffOptions {
            min_region_px: 1,
            ..Default::default()
        };
        let r = diff_images(&old, &new, &opts).unwrap();
        assert_eq!(r.stats.added_px, 1, "index 5 is new ink");
        assert_eq!(r.stats.removed_px, 1, "index 0 lost its ink");
        assert_eq!(r.stats.changed_px, 0);
        assert_eq!(r.stats.regions, 2, "two disjoint single-pixel marks");
        // Overlay: added → green, removed → red.
        assert_eq!(&r.overlay.rgba[5 * 4..5 * 4 + 3], &OV_ADDED);
        assert_eq!(&r.overlay.rgba[0..3], &OV_REMOVED);
    }

    #[test]
    fn recoloured_ink_is_changed_not_added_or_removed() {
        // Both pages have ink at index 3, but different (dark) colours.
        let old = img(4, 4, &[(3, [10, 10, 10])]);
        let new = img(4, 4, &[(3, [10, 120, 10])]); // still ink (luma<=128), greener
        let opts = ImageDiffOptions {
            min_region_px: 1,
            ..Default::default()
        };
        let r = diff_images(&old, &new, &opts).unwrap();
        assert_eq!(r.stats.added_px, 0);
        assert_eq!(r.stats.removed_px, 0);
        assert_eq!(r.stats.changed_px, 1);
        assert_eq!(&r.overlay.rgba[3 * 4..3 * 4 + 3], &OV_CHANGED);
    }

    #[test]
    fn min_region_px_drops_speckle() {
        // A lone changed pixel is dropped at the default min_region_px (2)…
        let old = img(5, 5, &[]);
        let new = img(5, 5, &[(12, [0, 0, 0])]);
        let dropped = diff_images(&old, &new, &ImageDiffOptions::default()).unwrap();
        assert_eq!(dropped.stats.regions, 0, "1px < min_region_px(2) → noise");
        assert_eq!(dropped.stats.added_px, 0, "dropped from tallies too");
        // …but a 2-pixel adjacent blob survives.
        let new2 = img(5, 5, &[(12, [0, 0, 0]), (13, [0, 0, 0])]);
        let kept = diff_images(&old, &new2, &ImageDiffOptions::default()).unwrap();
        assert_eq!(kept.stats.regions, 1);
        assert_eq!(kept.stats.added_px, 2);
    }

    #[test]
    fn malformed_buffer_is_rejected() {
        assert!(matches!(
            Image::new(2, 2, vec![0; 3]),
            Err(EngineError::MalformedImage { .. })
        ));
    }

    #[test]
    fn four_connectivity_merges_an_l_shape_into_one_region() {
        // Three added pixels forming an L (0,0)-(1,0)-(1,1) on a 3×3 → one region.
        let old = img(3, 3, &[]);
        let new = img(3, 3, &[(0, [0, 0, 0]), (1, [0, 0, 0]), (4, [0, 0, 0])]);
        let opts = ImageDiffOptions {
            min_region_px: 1,
            ..Default::default()
        };
        let r = diff_images(&old, &new, &opts).unwrap();
        assert_eq!(r.stats.regions, 1);
        assert_eq!(r.stats.added_px, 3);
    }
}
