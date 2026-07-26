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
    /// Connected changed components smaller than this many pixels are hidden from
    /// the overlay, the region total, and the pixel tallies as likely noise
    /// (anti-aliasing shimmer, rescan speckle). Any hidden region is still counted
    /// in [`ImageDiffStats::suppressed_px`] / `suppressed_regions` and still trips
    /// [`ImageDiffStats::has_any_change`], so the floor can never turn a genuine
    /// change into a silent "no differences" (the trust bar).
    ///
    /// The default is `1`, which hides nothing — every changed region survives.
    /// A thin feature can rasterize to exactly one pixel at low DPI, so a floor
    /// above 1 is opt-in, not the shipping default.
    pub min_region_px: u32,
}

impl Default for ImageDiffOptions {
    fn default() -> Self {
        Self {
            ink_threshold: 128,
            change_threshold: 24,
            min_region_px: 1,
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
    /// `(added+removed+changed) / total` — the fraction of pixels in *surviving*
    /// (above-floor) changed regions. A page whose only change is sub-floor reads
    /// 0.0 here yet still has `suppressed_px > 0`; use [`Self::has_any_change`],
    /// not this fraction, to decide whether anything changed.
    pub changed_fraction: f64,
    /// Connected components (4-connectivity) over the union changed mask, after
    /// the `min_region_px` filter (i.e. only regions at or above the floor).
    pub regions: u32,
    /// Pixels in changed regions the `min_region_px` floor hid from the overlay
    /// and the tallies. `0` at the default floor. Surfaced so a hidden change is
    /// never invisible — the noise filter declutters, it does not silence.
    pub suppressed_px: u64,
    /// Number of sub-floor changed regions hidden by the `min_region_px` filter.
    pub suppressed_regions: u32,
}

impl ImageDiffStats {
    /// True when this page carries ANY change — including sub-floor regions the
    /// noise filter hid from the tallies. Change-detection must use this, never
    /// `changed_fraction`, or a hidden region becomes a silent miss (trust bar).
    pub fn has_any_change(&self) -> bool {
        self.added_px > 0 || self.removed_px > 0 || self.changed_px > 0 || self.suppressed_px > 0
    }
}

/// The diff of one page pair: tallies plus a renderable overlay.
#[derive(Clone, Debug)]
pub struct ImageDiffResult {
    pub stats: ImageDiffStats,
    pub overlay: Image,
}

/// Per-axis pixel slack that counts as the **same** sheet size (#262).
///
/// Rasterizing a page floors `size_in_points × dpi / 72` to whole pixels, so two
/// exports of the same paper can land a pixel apart on either axis: an A4
/// landscape `MediaBox [0 0 842 595]` gives 1754×1239 px at 150 DPI while the
/// exact `[0 0 841.89 595.276]` gives 1753×1240 px. Calling that a paper-size
/// change would throw away the sheet's entire pixel diff over a rounding
/// artefact — the second cardinal sin, destroying a precise diff. Two pixels per
/// axis covers the rounding of both sides at any DPI (each side can only lose
/// under one pixel to the floor) without letting a real resize through: the
/// smallest genuine paper step, A4→A3, is hundreds of pixels.
pub const SIZE_TOLERANCE_PX: u32 = 2;

/// How the two rasters of a paired sheet relate in size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairSizing {
    /// Identical dimensions — diff as they are.
    Same,
    /// Within [`SIZE_TOLERANCE_PX`] on both axes: rasterization rounding, not a
    /// resize. Crop both sides to `to` with [`crop_top_left`] and diff that, so a
    /// real change is still found and located.
    Rounded { to: (u32, u32) },
    /// A genuine paper-size change (#262). The two rasters have no pixel
    /// correspondence, so the sheet is reported as wholly changed with both sizes
    /// named — etchy never rescales a raster onto the other's frame.
    Changed,
}

/// Classify a paired sheet's two raster sizes. Pure, so the CLI, the engine and
/// the viewer all decide the same way.
pub fn classify_pair_size(old: (u32, u32), new: (u32, u32)) -> PairSizing {
    let (dw, dh) = (old.0.abs_diff(new.0), old.1.abs_diff(new.1));
    if dw == 0 && dh == 0 {
        PairSizing::Same
    } else if dw <= SIZE_TOLERANCE_PX && dh <= SIZE_TOLERANCE_PX {
        PairSizing::Rounded {
            to: (old.0.min(new.0), old.1.min(new.1)),
        }
    } else {
        PairSizing::Changed
    }
}

/// Crop a raster to `w × h` from its top-left corner — the corner a rasterizer
/// anchors a page to, so the shared region of two nearly-identical renders is the
/// top-left `min × min` block.
///
/// # Errors
/// [`EngineError::ImageSizeMismatch`] if the requested crop does not fit inside
/// `img`, or is empty: cropping to nothing would silently diff zero pixels.
pub fn crop_top_left(img: &Image, w: u32, h: u32) -> Result<Image> {
    if w == 0 || h == 0 || w > img.width || h > img.height {
        return Err(EngineError::ImageSizeMismatch {
            ow: img.width,
            oh: img.height,
            nw: w,
            nh: h,
        });
    }
    if w == img.width && h == img.height {
        return Ok(img.clone());
    }
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as usize {
        let row = y * img.width as usize * 4;
        rgba.extend_from_slice(&img.rgba[row..row + w as usize * 4]);
    }
    Image::new(w, h, rgba)
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
    let mut suppressed_px: u64 = 0;
    let mut suppressed_regions: u32 = 0;
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
            // Below the floor: hide from the overlay/tallies as likely noise, but
            // record it so the change is surfaced, never silently dropped.
            suppressed_regions += 1;
            suppressed_px += members.len() as u64;
            for m in members {
                class[m] = Class::Unchanged;
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
            suppressed_px,
            suppressed_regions,
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
    fn a_lone_pixel_change_is_never_a_silent_miss_at_the_default_floor() {
        // #260 trust bar: a genuine one-pixel change (a thin feature at low DPI can
        // rasterize to exactly 1px) must NOT read as "no differences" under the
        // shipping default options. The default floor hides nothing.
        let old = img(5, 5, &[]);
        let new = img(5, 5, &[(12, [0, 0, 0])]);
        let r = diff_images(&old, &new, &ImageDiffOptions::default()).unwrap();
        assert!(
            r.stats.has_any_change(),
            "a 1px change was silently dropped — trust-bar failure"
        );
        assert!(
            r.stats.changed_fraction > 0.0,
            "changed_fraction tipped to 0.0 on a genuine change"
        );
        assert_eq!(r.stats.added_px, 1, "the pixel is tallied by default");
        assert_eq!(r.stats.regions, 1);
        assert_eq!(
            r.stats.suppressed_px, 0,
            "nothing hidden at the default floor"
        );
    }

    #[test]
    fn min_region_px_hides_speckle_but_surfaces_it() {
        // With an explicit floor of 2 a lone changed pixel is hidden from the
        // overlay/tallies as likely noise…
        let opts = ImageDiffOptions {
            min_region_px: 2,
            ..Default::default()
        };
        let old = img(5, 5, &[]);
        let new = img(5, 5, &[(12, [0, 0, 0])]);
        let dropped = diff_images(&old, &new, &opts).unwrap();
        assert_eq!(dropped.stats.regions, 0, "1px < min_region_px(2) → hidden");
        assert_eq!(dropped.stats.added_px, 0, "hidden from tallies too");
        // …but the suppression is surfaced, never silent, and still counts as a change.
        assert_eq!(dropped.stats.suppressed_px, 1, "hidden pixel is surfaced");
        assert_eq!(dropped.stats.suppressed_regions, 1);
        assert!(
            dropped.stats.has_any_change(),
            "a hidden sub-floor region is still a change (no silent miss)"
        );
        // A 2-pixel adjacent blob survives above the floor.
        let new2 = img(5, 5, &[(12, [0, 0, 0]), (13, [0, 0, 0])]);
        let kept = diff_images(&old, &new2, &opts).unwrap();
        assert_eq!(kept.stats.regions, 1);
        assert_eq!(kept.stats.added_px, 2);
        assert_eq!(kept.stats.suppressed_px, 0);
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

    #[test]
    fn identical_dimensions_are_the_same_size() {
        assert_eq!(
            classify_pair_size((1754, 1239), (1754, 1239)),
            PairSizing::Same
        );
    }

    #[test]
    fn a_sub_pixel_rounding_difference_is_not_a_resize() {
        // #262: the same A4 landscape sheet, MediaBox [0 0 842 595] against the
        // exact [0 0 841.89 595.276], rasterizes to 1754x1239 vs 1753x1240 px at
        // 150 DPI. Treating that as a paper-size change discards the sheet's whole
        // pixel diff over a rounding artefact.
        assert_eq!(
            classify_pair_size((1754, 1239), (1753, 1240)),
            PairSizing::Rounded { to: (1753, 1239) }
        );
        // Either direction, and up to the tolerance on both axes at once.
        assert_eq!(
            classify_pair_size((1753, 1240), (1754, 1239)),
            PairSizing::Rounded { to: (1753, 1239) }
        );
        assert_eq!(
            classify_pair_size((100, 100), (102, 98)),
            PairSizing::Rounded { to: (100, 98) }
        );
    }

    #[test]
    fn a_real_resize_is_still_a_size_change() {
        // A4 -> A3 at 150 DPI, and the synthetic 100x100 -> 100x200 pt case: both
        // are hundreds of pixels out, far past any rounding.
        assert_eq!(
            classify_pair_size((1240, 1754), (1754, 2480)),
            PairSizing::Changed
        );
        assert_eq!(
            classify_pair_size((100, 100), (100, 200)),
            PairSizing::Changed
        );
        // One axis inside the tolerance is not enough.
        assert_eq!(
            classify_pair_size((100, 100), (101, 140)),
            PairSizing::Changed
        );
        assert_eq!(
            classify_pair_size((100, 100), (97, 100)),
            PairSizing::Changed
        );
    }

    #[test]
    fn cropping_keeps_the_top_left_block() {
        let a = img(4, 3, &[(0, [0, 0, 0]), (3, [0, 0, 0]), (8, [0, 0, 0])]);
        let c = crop_top_left(&a, 3, 2).unwrap();
        assert_eq!((c.width, c.height), (3, 2));
        // Pixel 0 survives; pixel 3 (last column) and 8 (row 3) are cropped away.
        assert_eq!(&c.rgba[0..3], &[0, 0, 0]);
        assert!(c.rgba[4..].chunks_exact(4).all(|p| p[0] == 255));
        // A no-op crop is the same image.
        assert_eq!(crop_top_left(&a, 4, 3).unwrap(), a);
    }

    #[test]
    fn an_impossible_crop_fails_loud() {
        // Cropping to nothing, or to more than the raster holds, would silently
        // diff the wrong pixels (or zero of them).
        let a = img(4, 3, &[]);
        for (w, h) in [(0, 3), (4, 0), (5, 3), (4, 4)] {
            assert!(crop_top_left(&a, w, h).is_err(), "{w}x{h} must fail loud");
        }
    }

    #[test]
    fn a_rounded_pair_still_locates_its_change() {
        // The end of the #262 tolerance path: crop both to the shared size and the
        // pixel diff finds the added ink, instead of the pair being written off.
        let old = img(4, 3, &[]);
        let new = img(3, 4, &[(1, [0, 0, 0])]);
        let PairSizing::Rounded { to } = classify_pair_size((4, 3), (3, 4)) else {
            panic!("a 1 px difference per axis is rounding, not a resize");
        };
        assert_eq!(to, (3, 3));
        let (oc, nc) = (
            crop_top_left(&old, to.0, to.1).unwrap(),
            crop_top_left(&new, to.0, to.1).unwrap(),
        );
        let r = diff_images(&oc, &nc, &ImageDiffOptions::default()).unwrap();
        assert_eq!(r.stats.added_px, 1, "the added pixel is found");
        assert_eq!(r.stats.regions, 1);
        assert_eq!(r.stats.total_px, 9, "over the shared region only");
    }
}
