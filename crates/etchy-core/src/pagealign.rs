//! Content-fingerprint page alignment for multi-page raster diffs (#249).
//!
//! Pairing pages strictly by index desyncs the whole document the moment a sheet
//! is inserted or removed mid-revision: every later pair compares the wrong
//! sheets and reads as heavily changed, drowning the one real edit. This module
//! is the pure kernel that fixes it — a cheap per-page fingerprint plus a
//! sequence alignment over those fingerprints, yielding matched pairs and
//! explicit inserted / removed sheets.
//!
//! Trust rules baked in here (the alignment must never cost a change):
//! - The alignment decides only **which** pages pair. A matching fingerprint
//!   never means "unchanged" and never skips the pixel diff — fingerprints are
//!   lossy by construction (a 16×16 digest of a whole sheet), so acting on one
//!   would be a silent miss. Callers always run the full [`crate::diff_images`]
//!   on every pair.
//! - Every old page and every new page appears in the result exactly once, as a
//!   pair or as an old-only / new-only sheet. Nothing can be dropped.
//! - The digest is **local and absolute**: a cell records how much of itself is
//!   inked against a fixed luminance threshold, so adding or removing ink changes
//!   only the cells that ink touches. An earlier revision of this module compared
//!   each cell against the *page mean*; on sparse line art (i.e. every schematic
//!   sheet) adding one small part lowered the mean and flipped dozens of untouched
//!   cells, which made an edited sheet look less like itself than like a
//!   different sheet — see the regression tests at the bottom of this file.
//! - Index pairing is the baseline this must never do worse than: a re-pairing is
//!   adopted only when it beats index pairing by a clear margin, and otherwise
//!   the alignment falls back to index pairing and says so ([`Pairing`]).
//! - Deterministic: integer arithmetic, fixed tie-breaks, no hashing, no
//!   floating point, no iteration-order dependence. Same inputs, same alignment.

use crate::error::{EngineError, Result};
use crate::imagediff::Image;

/// Side of the fingerprint grid: each page is digested to `16×16` cells.
pub const FINGERPRINT_GRID: u32 = 16;

const CELLS: usize = (FINGERPRINT_GRID * FINGERPRINT_GRID) as usize;

/// A pixel whose luma is at or below this is **ink**. Absolute and fixed — this
/// is what makes the digest local: no page-wide statistic enters a cell's value,
/// so ink added in one corner cannot change the digest of another.
///
/// Deliberately looser than [`crate::ImageDiffOptions::ink_threshold`] (128): the
/// digest only has to notice *that* a cell carries line work, so it counts
/// anti-aliased hairline greys as ink too.
pub const INK_MAX_LUMA: u32 = 200;

/// Levels of ink coverage a cell is quantized to: `0` = no ink at all … `15` =
/// at least half the cell inked.
pub const CELL_LEVELS: u32 = 15;

/// Maximum value of [`dissimilarity`]: every cell maximally unlike (`0` =
/// identical digests). Integer, so the alignment has no floating-point wobble.
pub const SCALE: u32 = CELLS as u32 * CELL_LEVELS;

/// Cost of leaving a page unpaired (an inserted or removed sheet). Pairing two
/// pages costs their dissimilarity, so a pair is preferred over "removed +
/// inserted" whenever `d < 2 × GAP_COST` — i.e. unless the two sheets are no more
/// alike than chance. That bias is deliberate and set generously: a paired sheet
/// gets a real pixel diff, which is far more useful than two whole-sheet "gone /
/// appeared" rows, and losing a precise diff is the failure this module exists to
/// prevent. At `SCALE/4` a pair is kept unless its cells differ by an average of
/// 7.5 coverage levels out of 15 — sheets with essentially nothing in common.
pub const GAP_COST: u32 = SCALE / 4;

/// How much cheaper a content alignment must be than plain index pairing before it
/// is adopted (#249 safety net). Index pairing is the well-understood baseline;
/// re-pairing has to be supported by real evidence, not a rounding difference.
/// Roughly "one sheet's worth of clearly-different content" — a genuinely
/// different sheet of the same pack scores several times this.
pub const ALIGN_MARGIN: u32 = SCALE / 32;

/// Most pages one side may have before page alignment refuses to run.
///
/// The alignment is a dynamic-programming matrix of `(old+1) × (new+1)` `u64`
/// cells, so its footprint is **quadratic** in the page count: two 10 000-page
/// PDFs (a couple of MB each, under every other cap) would ask for 800 MB and
/// abort the process. At this limit the matrix is `1025 × 1025 × 8 B` ≈ 8.4 MB,
/// which is safe on 32-bit wasm as well. Over the limit [`align_pages`] fails
/// loud *before* allocating, matching the per-file and per-page ceilings the
/// callers already enforce. No real fab pack or schematic set comes close.
pub const MAX_ALIGN_PAGES: usize = 1024;

/// A cheap content digest of one rasterized page.
///
/// Built from the raster the caller has **already** produced (no re-render): the
/// page is boxed down to a `16×16` grid and each cell records how much of itself
/// is inked — the fraction of pixels at or below [`INK_MAX_LUMA`], quantized onto
/// a log ladder of [`CELL_LEVELS`] steps so a hairline crossing a cell and a
/// filled pad are both resolvable. Schematic sheets are overwhelmingly white, so
/// a linear coverage scale would crush every cell to zero; the log ladder is what
/// makes the digest discriminative on line art.
///
/// Coverage is a *ratio*, so the digest is size-independent: the same sheet
/// rendered at a different DPI (vector line widths scale with it) or a sheet whose
/// paper size changed still digests onto the same grid with the same levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PageFingerprint {
    /// Ink-coverage level per grid cell, row-major, `0..=CELL_LEVELS`.
    cells: [u8; CELLS],
}

impl PageFingerprint {
    /// Cells carrying any ink at all. Exposed for tests and for callers that want
    /// to log a digest summary.
    pub fn ink_cells(&self) -> u32 {
        self.cells.iter().filter(|c| **c > 0).count() as u32
    }

    /// Total ink weight over the sheet: the sum of every cell's coverage level,
    /// `0` (blank) … [`SCALE`] (solid). A coarse "how much line work is on this
    /// sheet" figure.
    pub fn ink_weight(&self) -> u32 {
        self.cells.iter().map(|c| u32::from(*c)).sum()
    }

    /// The per-cell coverage levels, row-major.
    pub fn cell_levels(&self) -> &[u8] {
        &self.cells
    }
}

#[inline]
fn luma(px: &[u8]) -> u32 {
    // Same Rec.601-ish integer luma as the pixel diff, kept local so the digest
    // does not depend on that module's private helper.
    (px[0] as u32 * 54 + px[1] as u32 * 183 + px[2] as u32 * 19) >> 8
}

/// Quantize one cell's ink coverage onto the log ladder. `0` ink → level `0`; any
/// ink at all → at least level `1`; half the cell or more → [`CELL_LEVELS`].
///
/// The ladder is `log2` of the coverage in 1/32768ths, so each level is a doubling
/// of ink: the bottom of the range (a single hairline, well under 1% of a cell)
/// is as well resolved as the top, which is what sparse line art needs.
fn cell_level(ink: u64, total: u64) -> u8 {
    if ink == 0 || total == 0 {
        return 0;
    }
    let r = (ink * 32_768 / total).max(1) as u32;
    (1 + r.ilog2()).min(CELL_LEVELS) as u8
}

/// Digest an already-rasterized page.
pub fn fingerprint(img: &Image) -> PageFingerprint {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut cells = [0u8; CELLS];
    if w > 0 && h > 0 {
        let g = FINGERPRINT_GRID as usize;
        for cy in 0..g {
            // Integer block bounds; a page narrower/shorter than the grid gives
            // some cells a single shared row/column, which is fine and stable.
            let y0 = cy * h / g;
            let y1 = ((cy + 1) * h).div_ceil(g).max(y0 + 1).min(h);
            for cx in 0..g {
                let x0 = cx * w / g;
                let x1 = ((cx + 1) * w).div_ceil(g).max(x0 + 1).min(w);
                let mut ink = 0u64;
                let mut n = 0u64;
                for y in y0..y1 {
                    let row = y * w;
                    for x in x0..x1 {
                        let l = luma(&img.rgba[(row + x) * 4..(row + x) * 4 + 4]);
                        ink += u64::from(l <= INK_MAX_LUMA);
                        n += 1;
                    }
                }
                // Every cell covers at least one pixel: `y0 < h` and `x0 < w` for
                // any cell index, and the bounds above are clamped to at least
                // one row/column past those — so `n` is never zero here.
                debug_assert!(n > 0, "empty fingerprint cell");
                cells[cy * g + cx] = cell_level(ink, n);
            }
        }
    }
    PageFingerprint { cells }
}

/// How unlike two page digests are, `0` (identical) … [`SCALE`] (maximally
/// unlike): the **L1 distance** over per-cell ink-coverage levels.
///
/// Purely local — each cell contributes only its own difference, so a change
/// confined to one corner of a sheet contributes only that corner's cells. Two
/// sheets sharing a frame and title block therefore score close together no
/// matter how much ink either carries elsewhere.
pub fn dissimilarity(a: &PageFingerprint, b: &PageFingerprint) -> u32 {
    a.cells
        .iter()
        .zip(b.cells.iter())
        .map(|(x, y)| u32::from(x.abs_diff(*y)))
        .sum()
}

/// One step of an alignment: a paired page, or a sheet present on one side only.
/// Indices are 0-based into each side's page list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageMatch {
    /// Both revisions have this sheet — the caller pixel-diffs it in full.
    Paired { old: usize, new: usize },
    /// Only the old revision has it: a removed sheet.
    OldOnly { old: usize },
    /// Only the new revision has it: an inserted sheet.
    NewOnly { new: usize },
}

/// Which rule produced a pairing — always reported, never inferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pairing {
    /// Plain page-index pairing: sheet 1 with sheet 1, and any tail of extra
    /// sheets listed as inserted / removed. Either nothing suggested otherwise,
    /// or the content alignment agreed with it.
    Index,
    /// The content alignment differed from index pairing **and** cleared the
    /// confidence margin, so it was adopted: some sheet moved.
    Content,
    /// The content alignment differed from index pairing but the evidence was not
    /// clear enough ([`ALIGN_MARGIN`], or a pair no more alike than chance). Index
    /// pairing was kept and the ambiguity is stated out loud — a coin-flip
    /// re-pairing is worse than the baseline everybody understands.
    Ambiguous,
}

/// The chosen alignment of a document pair, in merged reading order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageAlignment {
    /// One entry per row of the merged document, front to back.
    pub matches: Vec<PageMatch>,
    /// Which rule produced [`Self::matches`].
    pub pairing: Pairing,
}

impl PageAlignment {
    /// Plain index pairing of `old` against `new` pages: `min(old,new)` pairs,
    /// then whatever tail one side has as inserted / removed sheets. The baseline
    /// behaviour, and the fallback when content alignment is not confident.
    pub fn by_index(old: usize, new: usize) -> Self {
        Self {
            matches: index_matches(old, new),
            pairing: Pairing::Index,
        }
    }

    /// True when the alignment is exactly index pairing with no inserted or
    /// removed sheets — the case where a caller should say nothing extra,
    /// because the report reads exactly as it always did.
    pub fn is_identity(&self) -> bool {
        self.matches
            .iter()
            .enumerate()
            .all(|(i, m)| matches!(m, PageMatch::Paired { old, new } if *old == i && *new == i))
    }

    /// 1-based new-side page numbers of inserted sheets, ascending.
    pub fn inserted_pages(&self) -> Vec<usize> {
        self.matches
            .iter()
            .filter_map(|m| match m {
                PageMatch::NewOnly { new } => Some(new + 1),
                _ => None,
            })
            .collect()
    }

    /// 1-based old-side page numbers of removed sheets, ascending.
    pub fn removed_pages(&self) -> Vec<usize> {
        self.matches
            .iter()
            .filter_map(|m| match m {
                PageMatch::OldOnly { old } => Some(old + 1),
                _ => None,
            })
            .collect()
    }

    /// How many sheets paired (and will therefore be pixel-diffed).
    pub fn paired_count(&self) -> usize {
        self.matches
            .iter()
            .filter(|m| matches!(m, PageMatch::Paired { .. }))
            .count()
    }

    /// True when any paired sheet sits at a different page number in the two
    /// revisions — i.e. the alignment actually shifted something and index
    /// pairing would have compared the wrong sheets.
    pub fn is_shifted(&self) -> bool {
        self.matches
            .iter()
            .any(|m| matches!(m, PageMatch::Paired { old, new } if old != new))
    }

    /// A one-line, human statement of the pairing — `None` only when there is
    /// genuinely nothing to explain (index pairing, nothing inserted or removed).
    /// Callers must surface this whenever it is `Some`: a re-pairing that silently
    /// changed which sheets were compared, or a re-pairing that was *declined*,
    /// would both be exactly the kind of quiet behaviour etchy refuses.
    pub fn note(&self) -> Option<String> {
        let inserted = self.inserted_pages();
        let removed = self.removed_pages();
        let mut parts = Vec::new();
        if !inserted.is_empty() {
            parts.push(phrase("inserted", "new", &inserted));
        }
        if !removed.is_empty() {
            parts.push(phrase("removed", "old", &removed));
        }
        match self.pairing {
            // Nothing was re-paired. The identity says nothing at all; a trailing
            // append or truncation still names the extra sheets.
            Pairing::Index if parts.is_empty() => None,
            Pairing::Index => Some(format!("paired by page index: {}", parts.join(", "))),
            Pairing::Content => {
                if parts.is_empty() {
                    // Reachable only if a future change adds a non-gap, non-index
                    // step; say something rather than nothing.
                    parts.push("pages re-paired by content".into());
                }
                Some(format!("aligned by page content: {}", parts.join(", ")))
            }
            Pairing::Ambiguous => {
                let mut s = String::from("page alignment was ambiguous, paired by index");
                if !parts.is_empty() {
                    s.push_str(": ");
                    s.push_str(&parts.join(", "));
                }
                Some(s)
            }
        }
    }
}

/// "1 sheet inserted at new page 3" / "2 sheets removed at old pages 3, 5".
fn phrase(verb: &str, side: &str, pages: &[usize]) -> String {
    let list = pages
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if pages.len() == 1 {
        format!("1 sheet {verb} at {side} page {list}")
    } else {
        format!("{} sheets {verb} at {side} pages {list}", pages.len())
    }
}

/// Index pairing's steps: `min(n,m)` pairs, then one side's tail as gaps.
fn index_matches(n: usize, m: usize) -> Vec<PageMatch> {
    let k = n.min(m);
    (0..k)
        .map(|i| PageMatch::Paired { old: i, new: i })
        .chain((k..n).map(|old| PageMatch::OldOnly { old }))
        .chain((k..m).map(|new| PageMatch::NewOnly { new }))
        .collect()
}

/// Total cost of an alignment under the same rules the DP minimises.
fn alignment_cost(matches: &[PageMatch], old: &[PageFingerprint], new: &[PageFingerprint]) -> u64 {
    matches
        .iter()
        .map(|m| match *m {
            PageMatch::Paired { old: o, new: n } => u64::from(dissimilarity(&old[o], &new[n])),
            PageMatch::OldOnly { .. } | PageMatch::NewOnly { .. } => u64::from(GAP_COST),
        })
        .sum()
}

/// Align two revisions' page fingerprints into pairs plus inserted / removed
/// sheets.
///
/// A global (Needleman–Wunsch) sequence alignment minimising total cost:
/// [`dissimilarity`] for a pair, [`GAP_COST`] for an unpaired sheet. Ties break
/// towards pairing, then towards the old side, so the result is fully
/// deterministic and reduces to plain index pairing whenever that is optimal —
/// which it is for every document whose sheets simply changed in place.
///
/// **Confidence net.** A re-pairing is adopted only when it is *clearly* better
/// than index pairing: at least [`ALIGN_MARGIN`] cheaper, and with every pair it
/// chooses more alike than chance (`d < 2 × GAP_COST`). Otherwise the result is
/// index pairing tagged [`Pairing::Ambiguous`], which callers report. Index
/// pairing is the baseline this must never do worse than.
///
/// # Errors
/// [`EngineError::TooManyPages`] when either side has more than
/// [`MAX_ALIGN_PAGES`] pages — the DP matrix is quadratic in the page count, so
/// this fails loud *before* allocating rather than exhausting memory.
///
/// Cost is `O(old × new)` fingerprint comparisons of 256 bytes each: negligible
/// next to the rasterization the caller has already paid for.
pub fn align_pages(old: &[PageFingerprint], new: &[PageFingerprint]) -> Result<PageAlignment> {
    let (n, m) = (old.len(), new.len());
    if n > MAX_ALIGN_PAGES || m > MAX_ALIGN_PAGES {
        return Err(EngineError::TooManyPages {
            old: n,
            new: m,
            limit: MAX_ALIGN_PAGES,
        });
    }
    // The cap above bounds this at ~1 M cells, but do the arithmetic in u64 and
    // convert once so a 32-bit `usize` (wasm) can never wrap into an undersized
    // matrix and an out-of-bounds read.
    let stride = m + 1;
    let cell_count = (n as u64 + 1)
        .checked_mul(stride as u64)
        .and_then(|c| usize::try_from(c).ok())
        .ok_or(EngineError::TooManyPages {
            old: n,
            new: m,
            limit: MAX_ALIGN_PAGES,
        })?;
    // dp[i][j] = cheapest alignment of old[..i] against new[..j].
    let mut dp = vec![0u64; cell_count];
    for i in 1..=n {
        dp[i * stride] = i as u64 * u64::from(GAP_COST);
    }
    for (j, cell) in dp[..=m].iter_mut().enumerate() {
        *cell = j as u64 * u64::from(GAP_COST);
    }
    for i in 1..=n {
        for j in 1..=m {
            let pair =
                dp[(i - 1) * stride + (j - 1)] + u64::from(dissimilarity(&old[i - 1], &new[j - 1]));
            let skip_old = dp[(i - 1) * stride + j] + u64::from(GAP_COST);
            let skip_new = dp[i * stride + (j - 1)] + u64::from(GAP_COST);
            dp[i * stride + j] = pair.min(skip_old).min(skip_new);
        }
    }
    // Trace back, preferring a pair on ties (keeps index pairing when it is
    // optimal), then the old side. Built back-to-front, then reversed.
    let mut matches = Vec::with_capacity(n.max(m));
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        let here = dp[i * stride + j];
        if i > 0 && j > 0 {
            let pair =
                dp[(i - 1) * stride + (j - 1)] + u64::from(dissimilarity(&old[i - 1], &new[j - 1]));
            if pair == here {
                matches.push(PageMatch::Paired {
                    old: i - 1,
                    new: j - 1,
                });
                i -= 1;
                j -= 1;
                continue;
            }
        }
        if i > 0 && dp[(i - 1) * stride + j] + u64::from(GAP_COST) == here {
            matches.push(PageMatch::OldOnly { old: i - 1 });
            i -= 1;
            continue;
        }
        // Only remaining possibility (and the j>0 guard is structural: the dp
        // recurrence always reproduces `here` from one of the three moves).
        debug_assert!(j > 0, "traceback must consume the new side");
        matches.push(PageMatch::NewOnly { new: j - 1 });
        j -= 1;
    }
    matches.reverse();

    // The confidence net. If the DP landed on index pairing anyway, nothing was
    // re-paired and there is nothing to justify.
    let index = index_matches(n, m);
    if matches == index {
        return Ok(PageAlignment {
            matches,
            pairing: Pairing::Index,
        });
    }
    let chosen_cost = dp[n * stride + m];
    let index_cost = alignment_cost(&index, old, new);
    let clear_margin = chosen_cost + u64::from(ALIGN_MARGIN) <= index_cost;
    // Every pair it chose must be more alike than chance, or the "alignment" is
    // really just shuffling unrelated sheets around.
    let pairs_are_alike = matches.iter().all(|m| match *m {
        PageMatch::Paired { old: o, new: n } => {
            u64::from(dissimilarity(&old[o], &new[n])) < 2 * u64::from(GAP_COST)
        }
        _ => true,
    });
    if clear_margin && pairs_are_alike {
        Ok(PageAlignment {
            matches,
            pairing: Pairing::Content,
        })
    } else {
        Ok(PageAlignment {
            matches: index,
            pairing: Pairing::Ambiguous,
        })
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Sparse **line-art** page fixtures — thin borders, a title block, nets and
    //! parts. These are what `etchy-pdf` actually rasterizes, and the regime the
    //! digest has to be right in: a dense-blob fixture is stable under a bad
    //! digest and hides the failure this module's tests exist to catch.

    use super::*;

    /// A4 landscape at the default 150 DPI (842 × 595 pt).
    pub const SHEET_W: u32 = 1754;
    pub const SHEET_H: u32 = 1239;

    /// A white page drawn on with black line work.
    pub struct Sheet {
        pub w: u32,
        pub h: u32,
        rgba: Vec<u8>,
    }

    impl Sheet {
        pub fn new(w: u32, h: u32) -> Self {
            Self {
                w,
                h,
                rgba: vec![255u8; w as usize * h as usize * 4],
            }
        }

        /// Fill a rectangle with the grey level `g` (0 = black).
        pub fn fill_grey(&mut self, x0: u32, y0: u32, w: u32, h: u32, g: u8) {
            for y in y0..(y0 + h).min(self.h) {
                for x in x0..(x0 + w).min(self.w) {
                    let i = (y as usize * self.w as usize + x as usize) * 4;
                    self.rgba[i] = g;
                    self.rgba[i + 1] = g;
                    self.rgba[i + 2] = g;
                }
            }
        }

        pub fn fill(&mut self, x0: u32, y0: u32, w: u32, h: u32) {
            self.fill_grey(x0, y0, w, h, 0);
        }

        /// A horizontal run `t` px thick — a net, or one edge of a frame.
        pub fn hline(&mut self, x0: u32, x1: u32, y: u32, t: u32) {
            self.fill(x0, y, x1.saturating_sub(x0), t);
        }

        pub fn vline(&mut self, x: u32, y0: u32, y1: u32, t: u32) {
            self.fill(x, y0, t, y1.saturating_sub(y0));
        }

        pub fn outline(&mut self, x: u32, y: u32, w: u32, h: u32, t: u32) {
            self.hline(x, x + w, y, t);
            self.hline(x, x + w, y + h.saturating_sub(t), t);
            self.vline(x, y, y + h, t);
            self.vline(x + w.saturating_sub(t), y, y + h, t);
        }

        /// A schematic part: an outline box with pin stubs either side.
        pub fn part(&mut self, x: u32, y: u32, w: u32, h: u32) {
            self.outline(x, y, w, h, 2);
            for k in 0..4 {
                let py = y + 8 + k * (h.max(40) / 5);
                self.hline(x.saturating_sub(14), x, py, 2);
                self.hline(x + w, x + w + 14, py, 2);
            }
        }

        pub fn image(&self) -> Image {
            Image::new(self.w, self.h, self.rgba.clone()).expect("fixture buffer is well-formed")
        }

        pub fn fingerprint(&self) -> PageFingerprint {
            fingerprint(&self.image())
        }

        /// Ink pixels on the sheet (luma at or below the digest's threshold).
        pub fn ink_px(&self) -> u64 {
            self.rgba
                .chunks_exact(4)
                .filter(|p| luma(p) <= INK_MAX_LUMA)
                .count() as u64
        }
    }

    /// The frame + title block every sheet of a pack shares.
    fn template(s: &mut Sheet) {
        s.outline(18, 18, s.w - 36, s.h - 36, 2);
        s.outline(30, 30, s.w - 60, s.h - 60, 1);
        let (tx, ty, tw, th) = (s.w - 430, s.h - 220, 380, 170);
        s.outline(tx, ty, tw, th, 2);
        for k in 1..6 {
            s.hline(tx, tx + tw, ty + k * th / 6, 1);
        }
        s.vline(tx + 150, ty, ty + th, 1);
        // Text-ish marks in the title-block rows.
        for k in 0..6 {
            for j in 0..7 {
                s.fill(tx + 12 + j * 18, ty + 10 + k * th / 6, 12, 8);
            }
        }
    }

    /// Sheet 1 of the pack: template, a net grid of 1 px runs, four parts.
    pub fn sheet_one() -> Sheet {
        let mut s = Sheet::new(SHEET_W, SHEET_H);
        template(&mut s);
        for k in 0..13 {
            s.hline(40, SHEET_W - 40, 60 + k * 90, 1);
        }
        for k in 0..11 {
            s.vline(60 + k * 150, 40, SHEET_H - 40, 1);
        }
        s.part(320, 200, 90, 120);
        s.part(600, 380, 90, 120);
        s.part(980, 200, 120, 160);
        s.part(1150, 600, 90, 120);
        s
    }

    /// Sheet 1 with **one** extra 40 × 28 pt part (83 × 58 px at 150 DPI) in an
    /// otherwise empty corner — the smallest realistic revision edit.
    pub fn sheet_one_plus_part() -> Sheet {
        let mut s = sheet_one();
        s.part(150, 940, 83, 58);
        s
    }

    /// Sheet 2 of the same pack: same template, different nets and parts.
    pub fn sheet_two() -> Sheet {
        let mut s = Sheet::new(SHEET_W, SHEET_H);
        template(&mut s);
        for k in 0..6 {
            s.vline(300 + k * 160, 150, 1000, 2);
        }
        for k in 0..4 {
            s.hline(300, 1300, 200 + k * 200, 2);
        }
        s.part(420, 300, 120, 200);
        s.part(760, 620, 90, 120);
        s.part(1100, 300, 90, 120);
        s
    }

    /// Sheet 2 with one part deleted — a lighter revision of itself.
    pub fn sheet_two_light() -> Sheet {
        let mut s = Sheet::new(SHEET_W, SHEET_H);
        template(&mut s);
        for k in 0..6 {
            s.vline(300 + k * 160, 150, 1000, 2);
        }
        for k in 0..4 {
            s.hline(300, 1300, 200 + k * 200, 2);
        }
        s.part(420, 300, 120, 200);
        s.part(760, 620, 90, 120);
        s
    }

    /// Sheet 3: a third distinct sheet of the pack.
    pub fn sheet_three() -> Sheet {
        let mut s = Sheet::new(SHEET_W, SHEET_H);
        template(&mut s);
        for k in 0..9 {
            s.hline(100, 1200, 90 + k * 110, 2);
        }
        s.part(200, 700, 200, 240);
        s.part(900, 120, 90, 120);
        s
    }

    /// The fixture that broke the previous, mean-relative digest: an even
    /// hairline grid over the top 12 cell-rows and a blank band below, which puts
    /// the page mean exactly one luma unit above the inked cells' means. Adding
    /// one small part in the blank band nudged the mean down and flipped every
    /// grid cell off at once.
    pub fn even_grid(extra_part: bool) -> Sheet {
        let (w, h) = (1760u32, 1248u32);
        let (cw, ch) = (w / 16, h / 16);
        let mut s = Sheet::new(w, h);
        for cy in 0..12u32 {
            for cx in 0..16u32 {
                let (x0, y0) = (cx * cw, cy * ch);
                s.hline(x0, x0 + cw, y0 + 20, 1);
            }
        }
        if extra_part {
            s.part(150, 1000, 83, 58);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn align(old: &[PageFingerprint], new: &[PageFingerprint]) -> PageAlignment {
        align_pages(old, new).expect("within the page limit")
    }

    #[test]
    fn the_fixtures_really_are_sparse_line_art() {
        // If a "line art" fixture ever turns into a dense blob it stops guarding
        // anything — the failure this module's tests catch only shows up on sparse
        // ink. Real schematic sheets run a few percent ink.
        for (name, s) in [
            ("sheet 1", sheet_one()),
            ("sheet 2", sheet_two()),
            ("sheet 3", sheet_three()),
        ] {
            let frac = s.ink_px() as f64 / (u64::from(s.w) * u64::from(s.h)) as f64;
            assert!(
                (0.002..0.08).contains(&frac),
                "{name} is {:.2}% ink — not sparse line art any more",
                frac * 100.0
            );
        }
    }

    #[test]
    fn identical_pages_digest_identically() {
        let a = sheet_one().fingerprint();
        let b = sheet_one().fingerprint();
        assert_eq!(a, b, "the digest is a pure function of the pixels");
        assert_eq!(dissimilarity(&a, &b), 0);
    }

    #[test]
    fn a_small_local_edit_barely_moves_the_digest() {
        // THE load-bearing property (#249). One extra part in an empty corner may
        // only perturb the cells that part touches — the digest carries no
        // page-wide statistic that a local edit could shift.
        let base = sheet_one().fingerprint();
        let edited = sheet_one_plus_part().fingerprint();
        let d = dissimilarity(&base, &edited);
        let touched = base
            .cell_levels()
            .iter()
            .zip(edited.cell_levels())
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            touched <= 6,
            "one 83x58 px part changed {touched} of {CELLS} cells — the digest is not local"
        );
        assert!(
            d < ALIGN_MARGIN,
            "a one-part edit scored d={d}, above the alignment margin {ALIGN_MARGIN}"
        );
    }

    #[test]
    fn a_different_sheet_scores_far_worse_than_an_edited_one() {
        // The alignment only needs the ORDER to be right: an edited sheet must
        // score much closer to its original than a different sheet does. That gap
        // is what makes an inserted sheet cheaper to gap than to mis-pair.
        let one = sheet_one().fingerprint();
        let edited = sheet_one_plus_part().fingerprint();
        let two = sheet_two().fingerprint();
        let edit = dissimilarity(&one, &edited);
        let different = dissimilarity(&one, &two);
        assert!(
            edit * 10 < different,
            "edit={edit} different={different} — not a comfortable margin"
        );
        assert!(different > 0, "two different sheets digested identically");
    }

    #[test]
    fn an_edited_sheet_stays_far_below_the_unpair_threshold() {
        // If an edited sheet's digest crosses 2*GAP_COST the DP unpairs it, the
        // pixel diff is never run and the real change is never located — the
        // regression that made the mean-relative digest strictly worse than index
        // pairing. Keep a wide margin.
        let threshold = 2 * GAP_COST;
        for (name, a, b) in [
            (
                "sheet 1 + one part",
                sheet_one().fingerprint(),
                sheet_one_plus_part().fingerprint(),
            ),
            (
                "sheet 2 - one part",
                sheet_two().fingerprint(),
                sheet_two_light().fingerprint(),
            ),
        ] {
            let d = dissimilarity(&a, &b);
            assert!(
                d * 20 < threshold,
                "{name}: d={d} is not comfortably below the {threshold} unpair threshold"
            );
        }
    }

    #[test]
    fn a_sheet_is_more_like_its_own_revision_than_any_other_sheet() {
        // Reviewer scenario B: the alignment must never find an unrelated sheet a
        // cheaper partner than a sheet's own revision, or it recreates the very
        // desync #249 exists to fix and mislabels which sheet was inserted.
        let two = sheet_two().fingerprint();
        let own = dissimilarity(&two, &sheet_two_light().fingerprint());
        for (name, other) in [
            ("sheet 1", sheet_one().fingerprint()),
            ("sheet 1 + part", sheet_one_plus_part().fingerprint()),
            ("sheet 3", sheet_three().fingerprint()),
        ] {
            let d = dissimilarity(&two, &other);
            assert!(
                own < d,
                "sheet 2 looked more like {name} (d={d}) than its own revision (d={own})"
            );
        }
    }

    #[test]
    fn adding_ink_cannot_flip_untouched_cells() {
        // The exact fixture that inverted the previous digest: an even hairline
        // grid whose page mean sat one luma unit above every inked cell, so ONE
        // small part added in the blank band flipped 188 untouched cells off and
        // scored the sheet as a different document. Now only the cells the part
        // covers may move, and every other cell must be bit-identical.
        let base = even_grid(false).fingerprint();
        let edited = even_grid(true).fingerprint();
        let moved: Vec<usize> = base
            .cell_levels()
            .iter()
            .zip(edited.cell_levels())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect();
        assert!(
            moved.len() <= 6,
            "{} of {CELLS} cells moved for one small part: {moved:?}",
            moved.len()
        );
        assert!(
            base.ink_cells().abs_diff(edited.ink_cells()) <= 6,
            "ink_cells {} -> {} — a local edit must not flip the whole grid",
            base.ink_cells(),
            edited.ink_cells()
        );
        let d = dissimilarity(&base, &edited);
        assert!(
            d < ALIGN_MARGIN,
            "d={d} for one added part, at/above the alignment margin {ALIGN_MARGIN}"
        );
    }

    #[test]
    fn the_digest_survives_a_resolution_change() {
        // The same sheet at 2x the DPI must still read as the same sheet: coverage
        // is a ratio and vector line widths scale with the DPI, so the levels hold.
        let lo = {
            let mut s = Sheet::new(400, 300);
            s.outline(10, 10, 380, 280, 1);
            s.hline(40, 360, 100, 1);
            s.vline(200, 40, 260, 1);
            s.part(80, 150, 40, 60);
            s.fingerprint()
        };
        let hi = {
            let mut s = Sheet::new(800, 600);
            s.outline(20, 20, 760, 560, 2);
            s.hline(80, 720, 200, 2);
            s.vline(400, 80, 520, 2);
            s.part(160, 300, 80, 120);
            s.fingerprint()
        };
        let d = dissimilarity(&lo, &hi);
        assert!(
            d < GAP_COST,
            "same sheet at 2x scale read as different: d={d}"
        );
    }

    #[test]
    fn a_blank_and_a_solid_page_are_not_confused() {
        let blank = Sheet::new(320, 240).fingerprint();
        let solid = {
            let mut s = Sheet::new(320, 240);
            s.fill(0, 0, 320, 240);
            s.fingerprint()
        };
        assert_eq!(blank.ink_weight(), 0, "a blank sheet carries no ink");
        assert_eq!(solid.ink_weight(), SCALE, "a solid sheet saturates");
        assert_eq!(dissimilarity(&blank, &solid), SCALE);
    }

    #[test]
    fn a_hairline_and_a_filled_pad_are_told_apart() {
        // The log ladder's whole point: line art lives in the bottom percent of
        // coverage, so a hairline must not quantize to the same level as a pad.
        let hair = {
            let mut s = Sheet::new(160, 160);
            s.hline(0, 160, 80, 1);
            s.fingerprint()
        };
        let pad = {
            let mut s = Sheet::new(160, 160);
            s.fill(0, 70, 160, 20);
            s.fingerprint()
        };
        assert!(
            hair.ink_weight() * 2 < pad.ink_weight(),
            "hairline weight {} vs pad weight {}",
            hair.ink_weight(),
            pad.ink_weight()
        );
        assert!(hair.ink_cells() > 0, "a hairline must register at all");
    }

    #[test]
    fn equal_documents_align_by_index() {
        let (a, b, c) = (
            sheet_one().fingerprint(),
            sheet_two().fingerprint(),
            sheet_three().fingerprint(),
        );
        let al = align(&[a, b, c], &[a, b, c]);
        assert!(al.is_identity(), "{:?}", al.matches);
        assert_eq!(al.pairing, Pairing::Index);
        assert_eq!(al.note(), None, "identity says nothing extra");
        assert_eq!(al.paired_count(), 3);
    }

    #[test]
    fn a_small_edit_keeps_a_page_paired() {
        // A sheet with a tiny change must still pair — never be reported as a
        // removed + inserted sheet, which would lose its pixel diff.
        let a = sheet_one().fingerprint();
        let b = sheet_two().fingerprint();
        let b_edited = sheet_two_light().fingerprint();
        let al = align(&[a, b], &[a, b_edited]);
        assert!(al.is_identity(), "{:?}", al.matches);
        assert_eq!(al.note(), None);
    }

    #[test]
    fn an_edited_sheet_is_never_reported_as_inserted_plus_removed() {
        // The end-to-end shape of reviewer scenario A: a one-part edit on the last
        // sheet of a pack must stay a paired, pixel-diffable row.
        let one = sheet_one().fingerprint();
        let two = sheet_two().fingerprint();
        let two_plus = {
            let mut s = sheet_two();
            s.part(150, 940, 83, 58);
            s.fingerprint()
        };
        let al = align(&[one, two], &[one, two_plus]);
        assert!(al.is_identity(), "{:?}", al.matches);
        assert!(al.inserted_pages().is_empty(), "nothing was inserted");
        assert!(al.removed_pages().is_empty(), "nothing was removed");
        assert_eq!(al.paired_count(), 2, "both sheets get a pixel diff");
    }

    #[test]
    fn a_mid_document_insertion_is_found_and_the_rest_still_pairs() {
        // #249: old [1, 2, 3], new [1, X, 2, 3]. Index pairing would compare
        // 2↔X and 3↔2 and call all three heavily changed.
        let a = sheet_one().fingerprint();
        let b = sheet_two().fingerprint();
        let c = sheet_three().fingerprint();
        let x = {
            let mut s = Sheet::new(SHEET_W, SHEET_H);
            s.outline(18, 18, SHEET_W - 36, SHEET_H - 36, 2);
            s.fill(400, 300, 700, 500);
            s.fingerprint()
        };
        let al = align(&[a, b, c], &[a, x, b, c]);
        assert_eq!(
            al.matches,
            vec![
                PageMatch::Paired { old: 0, new: 0 },
                PageMatch::NewOnly { new: 1 },
                PageMatch::Paired { old: 1, new: 2 },
                PageMatch::Paired { old: 2, new: 3 },
            ]
        );
        assert_eq!(al.pairing, Pairing::Content, "clear evidence, adopted");
        assert_eq!(al.inserted_pages(), vec![2], "1-based new page 2");
        assert!(al.removed_pages().is_empty());
        assert!(al.is_shifted(), "pages moved — the report must say so");
        let note = al
            .note()
            .expect("a non-identity alignment is always stated");
        assert!(note.contains("aligned by page content"), "{note}");
        assert!(note.contains("inserted"), "{note}");
        assert!(note.contains('2'), "the note names the page: {note}");
    }

    #[test]
    fn a_mid_document_deletion_is_found() {
        let a = sheet_one().fingerprint();
        let b = sheet_two().fingerprint();
        let c = sheet_three().fingerprint();
        let al = align(&[a, b, c], &[a, c]);
        assert_eq!(
            al.matches,
            vec![
                PageMatch::Paired { old: 0, new: 0 },
                PageMatch::OldOnly { old: 1 },
                PageMatch::Paired { old: 2, new: 1 },
            ]
        );
        assert_eq!(al.pairing, Pairing::Content);
        assert_eq!(al.removed_pages(), vec![2]);
        assert!(al.note().unwrap().contains("removed"));
    }

    #[test]
    fn an_ambiguous_realignment_falls_back_to_index_pairing_and_says_so() {
        // The safety net (#249 follow-up): three sheets so alike that re-pairing
        // them saves almost nothing. Index pairing is the baseline we understand,
        // so a coin-flip re-pairing must be declined — loudly.
        let a = sheet_one().fingerprint();
        let a2 = sheet_one_plus_part().fingerprint();
        let a3 = {
            let mut s = sheet_one();
            s.part(150, 1040, 83, 58);
            s.fingerprint()
        };
        let al = align(&[a, a2], &[a3, a, a2]);
        assert_eq!(
            al.pairing,
            Pairing::Ambiguous,
            "near-identical sheets give no clear evidence: {:?}",
            al.matches
        );
        assert_eq!(al.matches, index_matches(2, 3), "index pairing kept");
        let note = al.note().expect("a declined re-pairing is stated");
        assert!(
            note.contains("ambiguous") && note.contains("index"),
            "{note}"
        );
        // And it still accounts for every page.
        assert_total_accounting(&al, 2, 3);
    }

    #[test]
    fn a_trailing_append_pairs_the_leading_pages() {
        let a = sheet_one().fingerprint();
        let b = sheet_two().fingerprint();
        let z = sheet_three().fingerprint();
        let al = align(&[a, b], &[a, b, z]);
        assert_eq!(
            al.matches,
            vec![
                PageMatch::Paired { old: 0, new: 0 },
                PageMatch::Paired { old: 1, new: 1 },
                PageMatch::NewOnly { new: 2 },
            ]
        );
        assert!(!al.is_shifted(), "an append shifts nothing");
        assert_eq!(
            al.pairing,
            Pairing::Index,
            "an append IS index pairing — no ambiguity to report"
        );
        let note = al.note().expect("the appended sheet is named");
        assert!(note.contains("paired by page index"), "{note}");
        assert!(note.contains("1 sheet inserted at new page 3"), "{note}");
    }

    #[test]
    fn one_empty_side_is_all_gaps() {
        let a = sheet_one().fingerprint();
        assert_eq!(
            align(&[], &[a, a]).matches,
            vec![PageMatch::NewOnly { new: 0 }, PageMatch::NewOnly { new: 1 }]
        );
        assert_eq!(
            align(&[a], &[]).matches,
            vec![PageMatch::OldOnly { old: 0 }]
        );
        assert_eq!(align(&[], &[]).matches, vec![]);
        assert!(align(&[], &[]).is_identity(), "nothing to explain");
        assert_eq!(align(&[], &[]).note(), None);
    }

    #[test]
    fn too_many_pages_fails_loud_before_allocating() {
        // #249 follow-up: the DP matrix is quadratic, so two 10 000-page PDFs
        // asked for 800 MB and aborted the process with no message. Over the cap
        // it must be a typed error naming both counts and the limit.
        let a = sheet_one().fingerprint();
        let big = vec![a; MAX_ALIGN_PAGES + 1];
        let ok = vec![a; MAX_ALIGN_PAGES];
        for (old, new) in [(&big, &ok), (&ok, &big), (&big, &big)] {
            let err = align_pages(old, new).expect_err("over the page limit");
            assert!(
                matches!(err, EngineError::TooManyPages { limit, .. } if limit == MAX_ALIGN_PAGES),
                "{err:?}"
            );
            let msg = err.to_string();
            assert!(
                msg.contains("too many pages") && msg.contains(&MAX_ALIGN_PAGES.to_string()),
                "the error names the limit: {msg}"
            );
        }
        // And exactly at the limit it still works (the cap is not off by one).
        assert!(align_pages(&ok, &ok).is_ok());
    }

    /// The accounting invariant: an alignment may never drop a page. Every old
    /// index and every new index appears exactly once, in ascending order.
    fn assert_total_accounting(al: &PageAlignment, n: usize, m: usize) {
        let mut olds = Vec::new();
        let mut news = Vec::new();
        for step in &al.matches {
            match step {
                PageMatch::Paired { old, new } => {
                    olds.push(*old);
                    news.push(*new);
                }
                PageMatch::OldOnly { old } => olds.push(*old),
                PageMatch::NewOnly { new } => news.push(*new),
            }
        }
        assert_eq!(olds, (0..n).collect::<Vec<_>>(), "old pages accounted once");
        assert_eq!(news, (0..m).collect::<Vec<_>>(), "new pages accounted once");
    }

    /// Six distinct line-art sheets to build documents out of.
    fn pack() -> Vec<PageFingerprint> {
        vec![
            sheet_one().fingerprint(),
            sheet_two().fingerprint(),
            sheet_three().fingerprint(),
            sheet_one_plus_part().fingerprint(),
            sheet_two_light().fingerprint(),
            even_grid(false).fingerprint(),
        ]
    }

    #[test]
    fn every_page_is_accounted_for_exactly_once() {
        let pages = pack();
        // A spread of shapes: identical, insert, delete, both, disjoint, empty.
        let cases: Vec<(Vec<usize>, Vec<usize>)> = vec![
            (vec![0, 1, 2], vec![0, 1, 2]),
            (vec![0, 1, 2], vec![0, 3, 1, 2]),
            (vec![0, 1, 2, 3], vec![0, 2, 3]),
            (vec![0, 1, 2, 3], vec![4, 0, 2, 5]),
            (vec![0, 1], vec![2, 3, 4, 5]),
            (vec![], vec![0, 1]),
            (vec![0, 1, 2], vec![]),
            (vec![0, 0, 0], vec![0, 0]),
        ];
        for (o, n) in cases {
            let olds: Vec<PageFingerprint> = o.iter().map(|i| pages[*i]).collect();
            let news: Vec<PageFingerprint> = n.iter().map(|i| pages[*i]).collect();
            let al = align(&olds, &news);
            assert_total_accounting(&al, olds.len(), news.len());
        }
    }

    #[test]
    fn the_alignment_is_deterministic() {
        let pages = pack();
        let old = [pages[0], pages[1], pages[2], pages[3]];
        let new = [pages[0], pages[4], pages[2], pages[3], pages[1]];
        let first = align(&old, &new);
        for _ in 0..5 {
            assert_eq!(align(&old, &new), first, "alignment must be stable");
        }
    }

    #[test]
    fn duplicate_pages_still_account_and_stay_in_order() {
        // Repeated identical sheets are the degenerate case for any alignment:
        // many equal-cost paths exist, so the tie-breaks must still yield a
        // monotone, complete accounting.
        let a = sheet_one().fingerprint();
        let al = align(&[a, a, a, a], &[a, a]);
        assert_total_accounting(&al, 4, 2);
        assert_eq!(al.paired_count(), 2);
        assert_eq!(al.removed_pages().len(), 2);
    }

    #[test]
    fn a_content_alignment_is_never_worse_than_index_pairing() {
        // The confidence net's contract, stated as a property over the pack: an
        // adopted re-pairing always costs strictly less than index pairing, and a
        // declined one IS index pairing.
        let pages = pack();
        for (o, n) in [
            (vec![0, 1, 2], vec![0, 5, 1, 2]),
            (vec![0, 1, 2, 3], vec![0, 2, 3]),
            (vec![0, 1], vec![1, 0]),
            (vec![0, 3, 4], vec![0, 4]),
            (vec![2, 0, 1], vec![2, 5, 0, 1]),
        ] {
            let olds: Vec<PageFingerprint> = o.iter().map(|i| pages[*i]).collect();
            let news: Vec<PageFingerprint> = n.iter().map(|i| pages[*i]).collect();
            let al = align(&olds, &news);
            let idx = index_matches(olds.len(), news.len());
            let idx_cost = alignment_cost(&idx, &olds, &news);
            let cost = alignment_cost(&al.matches, &olds, &news);
            match al.pairing {
                Pairing::Content => assert!(
                    cost + u64::from(ALIGN_MARGIN) <= idx_cost,
                    "{o:?} -> {n:?}: adopted a re-pairing costing {cost} vs index {idx_cost}"
                ),
                Pairing::Index | Pairing::Ambiguous => {
                    assert_eq!(al.matches, idx, "{o:?} -> {n:?} must keep index pairing")
                }
            }
        }
    }
}

#[cfg(test)]
mod prop_tests {
    use super::fixtures::*;
    use super::*;
    use proptest::prelude::*;

    /// Digests of synthetic **line-art** sheets — thin frame, nets, a title block
    /// and a few parts, varied by `k`. The alignment only ever sees fingerprints,
    /// so this exercises it over realistic page content cheaply. (Dense random
    /// dither, which an earlier version used, is the one regime where a broken
    /// digest looks fine.)
    fn arb_fp() -> impl Strategy<Value = PageFingerprint> {
        (0u32..24).prop_map(|k| line_art(k).fingerprint())
    }

    /// One of 24 distinct sparse line-art sheets, deterministic in `k`.
    fn line_art(k: u32) -> Sheet {
        let (w, h) = (480, 360);
        let mut s = Sheet::new(w, h);
        s.outline(6, 6, w - 12, h - 12, 1);
        s.outline(w - 150, h - 70, 140, 60, 1);
        for j in 0..(3 + k % 5) {
            s.hline(20, w - 20, 30 + j * (20 + k % 7), 1);
        }
        for j in 0..(2 + k % 4) {
            s.vline(40 + j * (30 + k % 11), 20, h - 20, 1);
        }
        for j in 0..(1 + k % 3) {
            s.part(60 + j * 90 + (k % 3) * 20, 120 + (k % 4) * 30, 40, 50);
        }
        s
    }

    proptest! {
        /// Trust invariant #2: no alignment, on any input, may drop or duplicate
        /// a page. Both sides come out complete and in order.
        #[test]
        fn alignment_accounts_for_every_page(
            old in proptest::collection::vec(arb_fp(), 0..7),
            new in proptest::collection::vec(arb_fp(), 0..7),
        ) {
            let al = align_pages(&old, &new).expect("within the page limit");
            let mut olds = Vec::new();
            let mut news = Vec::new();
            for step in &al.matches {
                match step {
                    PageMatch::Paired { old, new } => { olds.push(*old); news.push(*new); }
                    PageMatch::OldOnly { old } => olds.push(*old),
                    PageMatch::NewOnly { new } => news.push(*new),
                }
            }
            prop_assert_eq!(olds, (0..old.len()).collect::<Vec<_>>());
            prop_assert_eq!(news, (0..new.len()).collect::<Vec<_>>());
            // And the merged row count is bounded by the two page counts.
            prop_assert!(al.matches.len() >= old.len().max(new.len()));
            prop_assert!(al.matches.len() <= old.len() + new.len());
        }

        /// A document aligned against itself is always the identity — the tool's
        /// behaviour on an unchanged page count is exactly what it always was.
        #[test]
        fn self_alignment_is_the_identity(pages in proptest::collection::vec(arb_fp(), 0..7)) {
            let al = align_pages(&pages, &pages).expect("within the page limit");
            prop_assert!(al.is_identity(), "{:?}", al.matches);
            prop_assert_eq!(al.pairing, Pairing::Index);
            prop_assert_eq!(al.note(), None);
        }

        /// The confidence net, as a property: whatever the input, the alignment
        /// either costs less than index pairing by the full margin, or it IS
        /// index pairing. It can never be a worse pairing than the baseline.
        #[test]
        fn alignment_never_loses_to_index_pairing(
            old in proptest::collection::vec(arb_fp(), 0..6),
            new in proptest::collection::vec(arb_fp(), 0..6),
        ) {
            let al = align_pages(&old, &new).expect("within the page limit");
            let idx = index_matches(old.len(), new.len());
            if al.matches == idx {
                prop_assert!(matches!(al.pairing, Pairing::Index | Pairing::Ambiguous));
            } else {
                prop_assert_eq!(al.pairing, Pairing::Content);
                let cost = alignment_cost(&al.matches, &old, &new);
                let idx_cost = alignment_cost(&idx, &old, &new);
                prop_assert!(cost + u64::from(ALIGN_MARGIN) <= idx_cost);
            }
        }

        /// A local edit anywhere on a sheet keeps it far below the unpair
        /// threshold, so its pixel diff is never thrown away.
        #[test]
        fn a_local_edit_never_unpairs_a_sheet(k in 0u32..24, x in 0u32..380, y in 0u32..280) {
            let base = line_art(k);
            let mut edited = line_art(k);
            edited.part(x + 20, y + 20, 40, 50);
            let d = dissimilarity(&base.fingerprint(), &edited.fingerprint());
            prop_assert!(
                u64::from(d) < 2 * u64::from(GAP_COST),
                "one added part scored d={} against the {} unpair threshold",
                d,
                2 * GAP_COST
            );
        }

        /// Deterministic: repeated runs over the same input agree.
        #[test]
        fn alignment_is_reproducible(
            old in proptest::collection::vec(arb_fp(), 0..6),
            new in proptest::collection::vec(arb_fp(), 0..6),
        ) {
            prop_assert_eq!(
                align_pages(&old, &new).unwrap(),
                align_pages(&old, &new).unwrap()
            );
        }
    }
}
