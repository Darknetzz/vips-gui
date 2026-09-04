# vips-gui

A small Windows desktop app for batch-converting images to **JPEG**, **PNG**,
**WebP** or **AVIF**, using [libvips](https://www.libvips.org/) as the engine.

Drag images or folders in, choose a format and its encoder settings, and
convert. A preview panel shows the selected image before and after, together
with the exact size the output file will be.

## Requirements

**libvips must be installed separately.** It is not bundled.

The app ships as a single `.exe` with no DLLs of its own, and drives the
`vips` command-line tool as a subprocess. That keeps this program small and
avoids shipping a copy of libvips that would go stale.

### Installing libvips on Windows

1. Download `vips-dev-w64-web-<version>.zip` from the
   [libvips releases page](https://github.com/libvips/libvips/releases).
2. Unzip it somewhere permanent, for example `C:\Program Files`.
3. Add the `vips-<version>\bin` folder to your `PATH`.

libvips **8.13 or newer** is required.

If you would rather not touch `PATH`, either:

- use the **Locate vips.exe** button the app offers when it cannot find it, or
- set the `VIPS_GUI_VIPS_EXE` environment variable to the full path of
  `vips.exe`.

The app also looks in a few common install locations (`C:\Program Files`,
`%LOCALAPPDATA%\Programs`, scoop and chocolatey shim directories, and the
roots of `C:` and `D:`), so an unzipped copy is often found without any setup.

### Checking the installation

```
vips-gui --check
```

reports which libvips was found, its version, and which output formats it
supports, then exits. Useful when the app says libvips is missing and you are
sure it is not.

## Features

- **Batch conversion.** Drop in files or whole folder trees; non-images are
  filtered out automatically.
- **Full encoder control.** Every relevant libvips save option is exposed, and
  each control names the option it drives so it can be looked up in the libvips
  reference.
  - JPEG: quality, progressive, optimised Huffman tables, optimised scans,
    trellis quantisation, overshoot deringing, quantisation table, chroma
    subsampling.
  - PNG: compression level, interlacing, palette quantisation (quality,
    dither, effort), bit depth.
  - WebP: quality, lossless, near-lossless, effort, alpha quality, smart
    subsampling, minimum-size search, presets.
  - AVIF: quality, lossless, effort, bit depth, chroma subsampling.
- **Resizing.** Fit inside a bounding box, with an option never to enlarge.
  Uses libvips `thumbnail`, which shrinks on load so large sources stay cheap.
- **Accurate size prediction.** The preview encodes the image at full
  resolution with your actual settings, so the predicted file size is the size
  you will get, not an estimate.
- **Parallel conversion** with a progress bar and a cancel that stops promptly
  and cleans up partial files.
- **Safe by default.** Output goes next to each original, and a name collision
  produces `name (1).ext` rather than overwriting anything. A JPEG-to-JPEG
  conversion in place cannot destroy its own input.
- **Format gating.** A libvips built without libheif has AVIF disabled with an
  explanation, rather than failing when you press Convert.

## Building

Requires a Rust toolchain (1.95 or newer) and, on Windows, the Windows SDK for
the resource compiler.

```
cargo build --release
```

The result is `target/release/vips-gui.exe`: one self-contained executable.
The Microsoft C runtime is linked statically (see `.cargo/config.toml`), so it
runs without the Visual C++ Redistributable installed.

To regenerate the application icon:

```
pwsh -File assets/make-icon.ps1
```

## Testing

```
cargo test
```

Unit tests run anywhere. The integration tests drive a real `vips` binary; on a
machine without libvips they print why they are skipping and pass, so the suite
stays green on a bare checkout.

Some tests deliberately exercise slow AVIF settings and real subprocess
cancellation, so the full suite takes around a minute.

## Licence

MIT. libvips itself is LGPL-2.1+ and is neither bundled nor linked here; this
app only runs it as a separate program.
