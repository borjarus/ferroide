# FerroIDE application icon

Part of one icon family shared with FerroDb (`ferrodb/assets/icons/`), whose glyph is a database cylinder with a rust lid.
This app's glyph: **code brackets with a rust slash**.

Shared look: dark "iron" squircle (`#3a3d43 → #1b1c1f`), light gray glyph `#dfe1e5`,
exactly one element in the "rust" gradient `#f59a5b → #c2410c`.

| File | Purpose |
|---|---|
| `ferroide.svg` | master, 1024×1024 — source for 48 px and up |
| `ferroide-small.svg` | simplified master (heavier strokes) for 16–32 px |
| `ferroide-glyph.svg` | 24×24 monochrome glyph (`currentColor`) for in-app use (title bar, About) |
| `ferroide.png` | 1024×1024 PNG |
| `png/NxN.png` | 16, 24, 32, 48, 64, 128, 256, 512, 1024 |
| `ferroide.ico` | Windows, 16–256 px |
| `ferroide.icns` | macOS |

Rasters are generated — edit the SVGs, then run `python assets/icons/generate.py`.
Changes to the shared frame or accent must be mirrored in the sister app.
