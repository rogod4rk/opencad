# OpenCAD

An open-source, AutoCAD-style 2D CAD application, built native in Rust (egui/eframe).
The goal: switch over from AutoCAD without relearning anything, open your DXF files, and
run fast on Linux, Windows and macOS.

> Status: early but real. The command engine, drafting tools, object snaps, grips,
> layers, undo/redo and DXF import/export work. Not yet done: full layer-manager UI,
> DWG support, and signed/distributed installers for every OS. See the backlog below.

## Why it feels like AutoCAD

- A real **command line** with the aliases you already know: `L` line, `C` circle,
  `REC` rectangle, `M` move, `CO` copy, `O` offset, `TR` trim, `F` fillet, and more.
- **Coordinate entry** the AutoCAD way: absolute `x,y`, relative `@dx,dy`, polar `@dist<angle`,
  or just click.
- **Object snaps** (endpoint, midpoint, center, quadrant, intersection, nearest) with on-screen
  markers, plus grid snap and ortho.
- **Grips**: select an object, drag the blue squares to edit it.
- **Window / crossing** selection (drag left-to-right vs right-to-left).

## Commands (25 live)

`LINE/L  PLINE/PL  RECTANG/REC  CIRCLE/C  ARC/A  ELLIPSE/EL  POLYGON/POL  TEXT/T`
`MOVE/M  COPY/CO  ROTATE/RO  SCALE/SC  MIRROR/MI  OFFSET/O  TRIM/TR  EXTEND/EX`
`FILLET/F  CHAMFER/CHA  ARRAY/AR  ARRAYPOLAR  ERASE/E  DIMLINEAR/DLI  ZOOM/Z  UNDO/U  REDO`

Type a command and press Enter (or Space). Empty Enter repeats the last command.
For FILLET/CHAMFER, type a number first to set the radius/distance.

## AutoCAD-file compatibility

- **Import** DXF (R12 / common R2000 ASCII): LINE, CIRCLE, ARC, POINT, TEXT, MTEXT,
  LWPOLYLINE, legacy POLYLINE/VERTEX, ELLIPSE, SPLINE (polyline-approximated), and the
  LAYER table. Use the side-panel button or just **drag a `.dxf` onto the window**.
- **Export** DXF (R12) including a LAYER table.
- DWG is on the roadmap (via the OpenCASCADE kernel / ODA).

## Build and run

Requires the Rust toolchain (`https://rustup.rs`).

```sh
cargo run --release
```

On Linux you also need the usual GUI dev libraries (see `.github/workflows/release.yml`
for the exact apt packages).

## Install

- **macOS**: `cargo install cargo-bundle && cargo bundle --release` produces
  `target/release/bundle/osx/OpenCAD.app` (double-clickable; currently unsigned).
- **Windows / Linux**: push to a GitHub repo and the included
  `.github/workflows/release.yml` builds `.exe` (Windows), `.deb` + binary (Linux) and a
  `.dmg` (macOS) on every `v*` tag, and attaches them to a GitHub Release.

## Architecture

- `src/model.rs` — the CAD core: entity model, layers, undo/redo, geometry
  (transforms, intersections, trim/fillet/offset), DXF import/export, coordinate parsing.
  Pure logic, unit-tested headlessly (`cargo test`). This is where an OpenCASCADE FFI will
  plug in for exact solids and DWG.
- `src/main.rs` — the egui UI: viewport, command engine, snaps, grips, panels.

## Roadmap

1. Interactive layer-manager UI (color / on / freeze / lock / add / rename)
2. App theme + status-bar polish toward 1:1 AutoCAD look
3. More dimensions (aligned, angular, radius, diameter), hatch, blocks
4. DWG + exact solids via OpenCASCADE (OCCT) FFI; 3D
5. Signed/notarized installers and auto-update
6. Natural-language command bar (describe it, get geometry)

## License

Open source (license TBD by the maintainer).
