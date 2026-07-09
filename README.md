# AlphaView

A native desktop RAW photo browser and developer for Sony `.ARW` files, built with Tauri (Rust) + React.

Point it at a folder of Sony RAW files and it decodes them natively — no external RAW engine, no cloud upload — with a real-time adjustable develop pipeline (exposure, white balance, contrast, highlights/shadows, saturation) and export to full-resolution JPEG/PNG.

## Features

- **Folder browser** — open a folder, virtual-scrolling grid of every `.ARW` file inside, with fast EXIF-embedded thumbnails (no full RAW decode needed just to browse)
- **Metadata panel** — camera make/model, lens, shutter speed, aperture, ISO, focal length, resolution, file size
- **Native RAW decode** — Sony ARW sensor data decoded and demosaiced directly in Rust via [`rawler`](https://github.com/dnglab/dnglab), parallelised with `rayon`
- **Adjustable develop pipeline**, applied live on top of the raw sensor data (debounced re-decode as you drag):
  - Exposure (EV)
  - Contrast
  - Highlights / Shadows (tone-region specific)
  - Saturation
  - White balance — temperature and tint
- **Auto-exposure baseline** — interquartile-mean metering targets linear middle gray before your manual exposure adjustment is applied, so a fresh RAW doesn't open black or blown out
- **Export** — bake the current adjustments into a full-resolution JPEG or PNG and save to disk

## Stack

- [Tauri v2](https://tauri.app/) (Rust backend + React/TypeScript frontend, native webview — no Electron)
- [`rawler`](https://crates.io/crates/rawler) — RAW file parsing (Sony ARW / TIFF-based formats)
- [`kamadak-exif`](https://crates.io/crates/kamadak-exif) — EXIF metadata + embedded-thumbnail extraction for the browser grid
- [`image`](https://crates.io/crates/image) — JPEG/PNG encoding
- `rayon` — parallel per-row pixel processing

## Honest notes on quality

- The demosaic step is an area-averaged nearest-block reconstruction, not a full AHD/VNG-class algorithm — it favors decode speed over maximum per-pixel sharpness.
- Auto white balance and auto exposure are simple heuristics (camera as-shot WB coefficients + interquartile-mean metering), not a scene-aware algorithm.
- Only tested against Sony `.ARW` files from a full-frame body (7008×4672). Other Sony sensor sizes and other manufacturers' RAW formats are not verified.
- This is a personal project, not a commercial product — no installer/auto-update, run it from source.

## Running it

```bash
npm install
npm run tauri dev
```

Requires the [Tauri prerequisites](https://tauri.app/start/prerequisites/) (Rust toolchain + platform WebView).
