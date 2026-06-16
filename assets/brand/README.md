# etchy — brand assets

Visual identity for **etchy**, the PCB visual + geometric diff tool.
Primary mark: a **stencil “E”** with a **fiducial** dot, in gold ENIG copper.

## Files

### Vector source (SVG)
| File | Use |
|------|-----|
| `etchy-icon.svg` | Primary mark, copper gradient, transparent bg |
| `etchy-icon-mono.svg` | Mark in `currentColor` — set `color:` to recolor |
| `etchy-app-icon.svg` | Mark on dark rounded-square tile (512) |
| `etchy-favicon.svg` | Simplified mark on dark tile (scalable favicon) |

### Raster (PNG) — `png/`
| File | Size | Use |
|------|------|-----|
| `etchy-favicon-16/32/48/64.png` | 16–64 | Browser favicons |
| `etchy-icon-256/512.png` | 256, 512 | Transparent copper mark |
| `etchy-app-icon-180/512/1024.png` | 180, 512, 1024 | App / store icons (180 = Apple touch) |
| `etchy-social-1280x640.png` | 1280×640 | GitHub social preview (Settings → Social preview) |
| `etchy-banner-1280x320.png` | 1280×320 | README hero banner |
| `etchy-wordmark-dark-1280x400.png` | 1280×400 | Wordmark lockup on dark |
| `etchy-wordmark-light-1280x400.png` | 1280×400 | Wordmark lockup on light |

## Palette
- Copper / ENIG gold gradient: `#f6c873` → `#e8a33d` → `#b9762a`
- Board dark: `#0b0f0e`  ·  Paper: `#f4f1e8`
- Diff accents: added `#46d18a`, removed `#ff5d73`

## Type
- Wordmark: **Zilla Slab** 700 (alternates explored: Space Mono, Space Grotesk)
- Mono / code / tagline: **JetBrains Mono**

## Notes
- The full interactive brand board (with live wordmark + finish controls) lives in `Etchy Brand Assets.dc.html`.
- SVGs are the source of truth; PNGs were generated from them. Re-rasterize from SVG for other sizes.
