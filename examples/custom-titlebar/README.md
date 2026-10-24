# custom-titlebar

The reference for drawing your own window chrome in Frust. The OS title bar is
removed and the app paints a 40 px bar itself, using only baseline widgets:
`window_drag_region` for dragging and double-click maximize, and
`WindowCommand`s for the Minimize / Maximize / Close buttons. The body shows a
counter and a live `WindowState` readout (maximized / fullscreen / focused /
minimized) plus a "Toggle fullscreen" button.

Read `src/main.rs`: one root component, a single tree in `build` composed from
`Self::title_bar`, `Self::body` and `Self::resize_frame`.

## Run

```
cargo run -p custom-titlebar
```

from the repository root (desktop only; no Android or iOS shell).

## Per-OS expectations

- macOS: `Decorations::NoTitleBar`. The traffic lights stay, placed at leading
  16 / band 40 by `TitleButtons`. The system resizes from the edges; double-click
  on the bar zooms. No resize strips are built.
- Windows: `Decorations::None`. Frameless; the bar draws its own buttons.
  Eight resize strips (6 px edges, 10 px corners) start the OS resize, and
  dragging the bar uses the OS move loop (Aero Snap).
- Linux (X11 and Wayland): `Decorations::None`. Same as Windows; the move and
  resize gestures are handed to the window manager or compositor (snap and
  tiling come from there).

On every non-macOS OS the strips are hidden while the window is maximized or
fullscreen.

## Gate checklist

Cells record each gate's observed result (date in the gate notes of
`docs/SHELLS_DEVELOPMENT.md`); `owed` means that gate has not run.

| OS | frame gone | drag moves | double-click toggles | buttons work | strips resize | maximize fills work area | fullscreen round-trip | close honours policy | shadow present |
|----|-----------|------------|----------------------|--------------|---------------|--------------------------|-----------------------|----------------------|----------------|
| macOS | PASS | PASS | PASS (zoom) | PASS (traffic lights at 16/40) | PASS (AppKit edges; `None`: FAIL) | PASS (visible frame) | PASS (×2) | PASS | PASS (`None` too) |
| Windows 11 | PASS | PASS (Snap too) | PASS | PASS | PASS (all 8) | PASS | PASS | PASS | PASS (DWM) |
| Linux X11 (KWin) | PASS | PASS | PASS | PASS | PASS (all 8) | PASS | PASS | PASS | none drawn |
| Linux Wayland (KWin) | PASS | PASS | PASS | PASS | PASS (5 of 8 run) | PASS | PASS | PASS | none drawn |
