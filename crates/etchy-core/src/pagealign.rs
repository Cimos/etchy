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
//! - Deterministic: integer arithmetic, fixed tie-breaks, no hashing, no
//!   floating point, no iteration-order dependence. Same inputs, same alignment.

use crate::imagediff::Image;

/// Side of the fingerprint grid: each page is digested to `16×16` cells.
pub const FINGERPRINT_GRID: u32 = 16;

const CELLS: usize = (FINGERPRINT_GRID * FINGERPRINT_GRID) as usize;

/// Fixed-point scale for [`dissimilarity`]: `0` = identical digests, [`SCALE`] =
/// maximally unlike. Integer so the alignment has no floating-point wobble.
pub const SCALE: u32 = 2048;

/// Cost of leaving a page unpaired (an inserted or removed sheet). Pairing two
/// pages costs their dissimilarity, so a pair is preferred over "removed +
/// inserted" whenever `d < 2 × GAP_COST` — i.e. unless the two sheets are no more
/// alike than chance. That bias is deliberate: a paired sheet gets a real pixel
/// diff, which is far more useful than two whole-sheet "gone / appeared" rows.
pub const GAP_COST: u32 = SCALE / 8;

/// A cheap content digest of one rasterized page.
///
/// Built from the raster the caller has **already** produced (no re-render): the
/// page is boxed down to a `16×16` grid of mean luminances, then each cell is
/// recorded as one bit — set when the cell is darker (more ink) than the page's
/// own average. That relative encoding is what makes the digest discriminative
/// on schematic sheets, which are overwhelmingly white: an absolute-brightness
/// digest would make every sheet look alike. A coarse absolute brightness level
/// is kept alongside so an all-blank and an all-black page are not confused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PageFingerprint {
    /// One bit per grid cell (row-major), set when the cell is darker than the
    /// page mean.
    bits: [u64; CELLS / 64],
    /// Page mean luminance quantized to 16 levels (0 = black … 15 = white).
    level: u8,
}

impl PageFingerprint {
    /// Bits set — i.e. cells darker than the page average. Exposed for tests and
    /// for callers that want to log a digest summary.
    pub fn ink_cells(&self) -> u32 {
        self.bits.iter().map(|w| w.count_ones()).sum()
    }

    /// The page's coarse brightness level (0 = black … 15 = white).
    pub fn level(&self) -> u8 {
        self.level
    }
}

#[inline]
fn luma(px: &[u8]) -> u32 {
    // Same Rec.601-ish integer luma as the pixel diff, kept local so the digest
    // does not depend on that module's private helper.
    (px[0] as u32 * 54 + px[1] as u32 * 183 + px[2] as u32 * 19) >> 8
}

/// Digest an already-rasterized page. Size-independent: a page rendered at a
/// different DPI, or a sheet whose paper size changed, still digests onto the
/// same 16×16 grid, so the alignment keeps working across a resize.
pub fn fingerprint(img: &Image) -> PageFingerprint {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut cell_mean = [255u32; CELLS];
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
                let mut sum = 0u64;
                let mut n = 0u64;
                for y in y0..y1 {
                    let row = y * w;
                    for x in x0..x1 {
                        sum += u64::from(luma(&img.rgba[(row + x) * 4..(row + x) * 4 + 4]));
                        n += 1;
                    }
                }
                // Every cell covers at least one pixel: `y0 < h` and `x0 < w` for
                // any cell index, and the bounds above are clamped to at least
                // one row/column past those — so `n` is never zero here.
                debug_assert!(n > 0, "empty fingerprint cell");
                cell_mean[cy * g + cx] = (sum / n.max(1)) as u32;
            }
        }
    }
    let page_mean: u32 = cell_mean.iter().sum::<u32>() / CELLS as u32;
    let mut bits = [0u64; CELLS / 64];
    for (i, m) in cell_mean.iter().enumerate() {
        if *m < page_mean {
            bits[i / 64] |= 1u64 << (i % 64);
        }
    }
    PageFingerprint {
        bits,
        level: (page_mean / 16).min(15) as u8,
    }
}

/// How unlike two page digests are, `0` (identical) … [`SCALE`] (maximally
/// unlike). Dominated by the cell-pattern Hamming distance, with a small
/// absolute-brightness term to separate pages whose *patterns* are degenerate
/// (a blank sheet and a solid-black sheet both have a flat pattern).
pub fn dissimilarity(a: &PageFingerprint, b: &PageFingerprint) -> u32 {
    let hamming: u32 = a
        .bits
        .iter()
        .zip(b.bits.iter())
        .map(|(x, y)| (x ^ y).count_ones())
        .sum();
    let level = u32::from(a.level.abs_diff(b.level));
    // 7 per differing cell (256 cells → 1792) + up to 256 for brightness = 2048.
    7 * hamming + (256 * level) / 15
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

/// The chosen alignment of a document pair, in merged reading order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageAlignment {
    /// One entry per row of the merged document, front to back.
    pub matches: Vec<PageMatch>,
}

impl PageAlignment {
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

    /// A one-line, human statement of the alignment chosen — `None` only when the
    /// alignment is the identity (nothing to explain). Callers must surface this
    /// whenever it is `Some`: a re-pairing that silently changed which sheets were
    /// compared would be exactly the kind of quiet behaviour etchy refuses.
    pub fn note(&self) -> Option<String> {
        if self.is_identity() {
            return None;
        }
        let inserted = self.inserted_pages();
        let removed = self.removed_pages();
        let mut parts = Vec::new();
        if !inserted.is_empty() {
            parts.push(phrase("inserted", "new", &inserted));
        }
        if !removed.is_empty() {
            parts.push(phrase("removed", "old", &removed));
        }
        if parts.is_empty() {
            // Reachable only if a future change adds a non-gap, non-identity
            // step; say something rather than nothing.
            parts.push("pages re-paired by content".into());
        }
        Some(format!("aligned by page content: {}", parts.join(", ")))
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

/// Align two revisions' page fingerprints into pairs plus inserted / removed
/// sheets.
///
/// A global (Needleman–Wunsch) sequence alignment minimising total cost:
/// [`dissimilarity`] for a pair, [`GAP_COST`] for an unpaired sheet. Ties break
/// towards pairing, then towards the old side, so the result is fully
/// deterministic and reduces to plain index pairing whenever that is optimal —
/// which it is for every document whose sheets simply changed in place.
///
/// Cost is `O(old × new)` fingerprint comparisons of 256 bits each: negligible
/// next to the rasterization the caller has already paid for.
pub fn align_pages(old: &[PageFingerprint], new: &[PageFingerprint]) -> PageAlignment {
    let (n, m) = (old.len(), new.len());
    // dp[i][j] = cheapest alignment of old[..i] against new[..j].
    let stride = m + 1;
    let mut dp = vec![0u64; (n + 1) * stride];
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
    PageAlignment { matches }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w×h` page, white, with the given filled black rectangles (x, y, w, h).
    fn page(w: u32, h: u32, rects: &[(u32, u32, u32, u32)]) -> Image {
        let mut rgba = vec![255u8; w as usize * h as usize * 4];
        for (rx, ry, rw, rh) in rects {
            for y in *ry..(ry + rh).min(h) {
                for x in *rx..(rx + rw).min(w) {
                    let i = (y as usize * w as usize + x as usize) * 4;
                    rgba[i] = 0;
                    rgba[i + 1] = 0;
                    rgba[i + 2] = 0;
                }
            }
        }
        Image::new(w, h, rgba).unwrap()
    }

    fn fp(w: u32, h: u32, rects: &[(u32, u32, u32, u32)]) -> PageFingerprint {
        fingerprint(&page(w, h, rects))
    }

    #[test]
    fn identical_pages_digest_identically() {
        let a = fp(64, 64, &[(8, 8, 16, 16)]);
        let b = fp(64, 64, &[(8, 8, 16, 16)]);
        assert_eq!(a, b, "the digest is a pure function of the pixels");
        assert_eq!(dissimilarity(&a, &b), 0);
    }

    #[test]
    fn the_digest_survives_a_resolution_change() {
        // The same sheet at 2x the DPI must still read as the same sheet: the
        // grid is relative, so alignment works across a re-render or a resize.
        let lo = fp(64, 64, &[(8, 8, 16, 16), (40, 40, 8, 8)]);
        let hi = fp(128, 128, &[(16, 16, 32, 32), (80, 80, 16, 16)]);
        assert!(
            dissimilarity(&lo, &hi) < GAP_COST,
            "same sheet at 2x scale read as different: d={}",
            dissimilarity(&lo, &hi)
        );
    }

    #[test]
    fn a_different_sheet_scores_far_worse_than_an_edited_one() {
        // The alignment only needs the ORDER to be right: an edited sheet must
        // score much closer to its original than a different sheet does. That
        // gap is what makes an inserted sheet cheaper to gap than to mis-pair.
        let a = fp(64, 64, &[(0, 0, 20, 20)]);
        let a_edited = fp(64, 64, &[(0, 0, 20, 20), (48, 48, 3, 3)]);
        let other = fp(64, 64, &[(44, 44, 20, 20)]);
        let edit = dissimilarity(&a, &a_edited);
        let different = dissimilarity(&a, &other);
        assert!(edit < different / 4, "edit={edit} different={different}");
        assert!(different > 0, "disjoint ink digested identically");
    }

    #[test]
    fn a_blank_and_a_solid_page_are_not_confused() {
        // Both have a flat cell pattern; only the brightness term separates them.
        let blank = fp(32, 32, &[]);
        let solid = fp(32, 32, &[(0, 0, 32, 32)]);
        assert!(
            dissimilarity(&blank, &solid) > 0,
            "a blank and a black sheet must not digest identically"
        );
    }

    #[test]
    fn equal_documents_align_by_index() {
        let a = fp(64, 64, &[(0, 0, 20, 20)]);
        let b = fp(64, 64, &[(44, 0, 20, 20)]);
        let c = fp(64, 64, &[(0, 44, 20, 20)]);
        let al = align_pages(&[a, b, c], &[a, b, c]);
        assert!(al.is_identity(), "{:?}", al.matches);
        assert_eq!(al.note(), None, "identity says nothing extra");
        assert_eq!(al.paired_count(), 3);
    }

    #[test]
    fn a_small_edit_keeps_a_page_paired() {
        // A sheet with a tiny change must still pair — never be reported as a
        // removed + inserted sheet, which would lose its pixel diff.
        let a = fp(64, 64, &[(0, 0, 20, 20)]);
        let b = fp(64, 64, &[(44, 0, 20, 20)]);
        let b_edited = fp(64, 64, &[(44, 0, 20, 20), (30, 60, 2, 2)]);
        let al = align_pages(&[a, b], &[a, b_edited]);
        assert!(al.is_identity(), "{:?}", al.matches);
    }

    #[test]
    fn a_mid_document_insertion_is_found_and_the_rest_still_pairs() {
        // #249: old [A, B, C], new [A, X, B, C]. Index pairing would compare
        // B↔X and C↔B and call all three heavily changed.
        let a = fp(64, 64, &[(0, 0, 20, 20)]);
        let b = fp(64, 64, &[(44, 0, 20, 20)]);
        let c = fp(64, 64, &[(0, 44, 20, 20)]);
        let x = fp(64, 64, &[(22, 22, 20, 20)]);
        let al = align_pages(&[a, b, c], &[a, x, b, c]);
        assert_eq!(
            al.matches,
            vec![
                PageMatch::Paired { old: 0, new: 0 },
                PageMatch::NewOnly { new: 1 },
                PageMatch::Paired { old: 1, new: 2 },
                PageMatch::Paired { old: 2, new: 3 },
            ]
        );
        assert_eq!(al.inserted_pages(), vec![2], "1-based new page 2");
        assert!(al.removed_pages().is_empty());
        assert!(al.is_shifted(), "pages moved — the report must say so");
        let note = al
            .note()
            .expect("a non-identity alignment is always stated");
        assert!(note.contains("inserted"), "{note}");
        assert!(note.contains('2'), "the note names the page: {note}");
    }

    #[test]
    fn a_mid_document_deletion_is_found() {
        let a = fp(64, 64, &[(0, 0, 20, 20)]);
        let b = fp(64, 64, &[(44, 0, 20, 20)]);
        let c = fp(64, 64, &[(0, 44, 20, 20)]);
        let al = align_pages(&[a, b, c], &[a, c]);
        assert_eq!(
            al.matches,
            vec![
                PageMatch::Paired { old: 0, new: 0 },
                PageMatch::OldOnly { old: 1 },
                PageMatch::Paired { old: 2, new: 1 },
            ]
        );
        assert_eq!(al.removed_pages(), vec![2]);
        assert!(al.note().unwrap().contains("removed"));
    }

    #[test]
    fn a_trailing_append_pairs_the_leading_pages() {
        let a = fp(64, 64, &[(0, 0, 20, 20)]);
        let b = fp(64, 64, &[(44, 0, 20, 20)]);
        let z = fp(64, 64, &[(22, 22, 20, 20)]);
        let al = align_pages(&[a, b], &[a, b, z]);
        assert_eq!(
            al.matches,
            vec![
                PageMatch::Paired { old: 0, new: 0 },
                PageMatch::Paired { old: 1, new: 1 },
                PageMatch::NewOnly { new: 2 },
            ]
        );
        assert!(!al.is_shifted(), "an append shifts nothing");
    }

    #[test]
    fn one_empty_side_is_all_gaps() {
        let a = fp(32, 32, &[(0, 0, 8, 8)]);
        assert_eq!(
            align_pages(&[], &[a, a]).matches,
            vec![PageMatch::NewOnly { new: 0 }, PageMatch::NewOnly { new: 1 }]
        );
        assert_eq!(
            align_pages(&[a], &[]).matches,
            vec![PageMatch::OldOnly { old: 0 }]
        );
        assert_eq!(align_pages(&[], &[]).matches, vec![]);
        assert!(align_pages(&[], &[]).is_identity(), "nothing to explain");
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

    #[test]
    fn every_page_is_accounted_for_exactly_once() {
        let pages: Vec<PageFingerprint> = (0..6)
            .map(|k| fp(64, 64, &[(k * 8, k * 8, 12, 12)]))
            .collect();
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
            let al = align_pages(&olds, &news);
            assert_total_accounting(&al, olds.len(), news.len());
        }
    }

    #[test]
    fn the_alignment_is_deterministic() {
        let pages: Vec<PageFingerprint> =
            (0..5).map(|k| fp(64, 64, &[(k * 10, 4, 14, 14)])).collect();
        let old = [pages[0], pages[1], pages[2], pages[3]];
        let new = [pages[0], pages[4], pages[2], pages[3], pages[1]];
        let first = align_pages(&old, &new);
        for _ in 0..5 {
            assert_eq!(align_pages(&old, &new), first, "alignment must be stable");
        }
    }

    #[test]
    fn duplicate_pages_still_account_and_stay_in_order() {
        // Repeated identical sheets are the degenerate case for any alignment:
        // many equal-cost paths exist, so the tie-breaks must still yield a
        // monotone, complete accounting.
        let a = fp(32, 32, &[(4, 4, 8, 8)]);
        let al = align_pages(&[a, a, a, a], &[a, a]);
        assert_total_accounting(&al, 4, 2);
        assert_eq!(al.paired_count(), 2);
        assert_eq!(al.removed_pages().len(), 2);
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    /// Digests built straight from bit patterns — the alignment only ever sees
    /// fingerprints, so this exercises it over arbitrary page content cheaply.
    fn arb_fp() -> impl Strategy<Value = PageFingerprint> {
        // 24 distinct synthetic pages is plenty of variety for the invariants,
        // and keeps the DP tiny.
        (0u32..24).prop_map(|k| {
            let mut rgba = vec![255u8; 32 * 32 * 4];
            for y in 0..32usize {
                for x in 0..32usize {
                    // A deterministic pseudo-pattern keyed by k.
                    if (x * 7 + y * 13 + k as usize * 31) % (5 + k as usize % 7) == 0 {
                        let i = (y * 32 + x) * 4;
                        rgba[i] = 0;
                        rgba[i + 1] = 0;
                        rgba[i + 2] = 0;
                    }
                }
            }
            fingerprint(&Image::new(32, 32, rgba).unwrap())
        })
    }

    proptest! {
        /// Trust invariant #2: no alignment, on any input, may drop or duplicate
        /// a page. Both sides come out complete and in order.
        #[test]
        fn alignment_accounts_for_every_page(
            old in proptest::collection::vec(arb_fp(), 0..7),
            new in proptest::collection::vec(arb_fp(), 0..7),
        ) {
            let al = align_pages(&old, &new);
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
            let al = align_pages(&pages, &pages);
            prop_assert!(al.is_identity(), "{:?}", al.matches);
            prop_assert_eq!(al.note(), None);
        }

        /// Deterministic: repeated runs over the same input agree.
        #[test]
        fn alignment_is_reproducible(
            old in proptest::collection::vec(arb_fp(), 0..6),
            new in proptest::collection::vec(arb_fp(), 0..6),
        ) {
            prop_assert_eq!(align_pages(&old, &new), align_pages(&old, &new));
        }
    }
}
