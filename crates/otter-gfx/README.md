# otter-gfx

OtterOS's 2D graphics engine: premultiplied-ARGB surfaces and compositing,
anti-aliased vector shapes and paths, a zlib/PNG codec, and a TrueType text
engine (glyf outlines, `cmap`, `kern`/`GPOS` kerning). `#![no_std]` + `alloc`,
zero external dependencies (DECISIONS.md D27) -- everything here is original
OtterOS logic operating on plain byte slices and in-memory buffers, with no
OS/hardware dependency, which is why it is unit-tested on the host instead of
under QEMU.

## Layout

| Module | What it does |
| --- | --- |
| `color` | Straight/premultiplied colour, exact integer `div255`/src-over blending |
| `surface` | `Surface` (owned pixel buffer) and `Canvas` (a view with a clip stack) |
| `geom` | `Point`, `Rect`, `Affine`, and `floor`/`ceil`/`round`/`sqrt` for `f32` (not in `core`) |
| `path` | Vector paths, adaptive Bezier flattening, stroking |
| `raster` | Non-zero-winding scanline rasterizer (signed-area accumulation) |
| `shapes` | Analytic rects/rounded-rects/circles/lines, blits |
| `gradient` | Linear/radial gradients with 4x4 ordered dithering |
| `shadow` | Box shadows: rasterize + separable box blur, cached by size |
| `checksum` | CRC-32, Adler-32 |
| `inflate` / `deflate` | DEFLATE/zlib decode (full) and encode (stored + fixed-Huffman) |
| `png` | PNG decode (colour types 0/2/3/4/6, depths 1-16, all filters, tRNS) and encode |
| `font` | TrueType parsing: table directory, `cmap`, `hhea`/`hmtx`, `glyf`, `kern`, `GPOS` |
| `text` | Glyph cache, UTF-8 decoding, measuring/drawing, word wrap, ellipsis, tabs |

## Testing

```sh
cd crates
cargo test -p otter-gfx                                 # unit + integration tests
cargo clippy -p otter-gfx --tests -- -D warnings         # lint
cargo build -p otter-gfx --target x86_64-unknown-none    # proves it's genuinely no_std
```

Two of the integration test files need real font files and generated
fixtures, both produced by tooling (not committed as large binaries):

```sh
../scripts/fetch-fonts.sh        # -> third_party/fonts/{inter,jetbrains-mono}/*.ttf
../scripts/gen-gfx-fixtures.py   # -> tests/fixtures/*.{zlib,png,json,expected}
```

`tests/specimen.rs` renders `../../artifacts/gfx-specimen.png` (via this
crate's own PNG encoder) to visually check the output against
`docs/DESIGN.md`.
