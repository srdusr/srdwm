# srdwm (legacy C++ prototype)

This directory holds the original C++ prototype of srdwm, preserved for reference
during the Rust rewrite. It is no longer built or maintained - see the repository
root for the active Rust implementation.

Status when archived (see `docs/IMPLEMENTATION_STATUS.md` and the root
`docs/PRIOR_ART.md` porting notes for detail):

- X11 backend: partially functional (reparenting decorations, EWMH atoms, no
  drag/resize, hardcoded titlebar width, RandR monitor bug).
- Windows backend: partially functional (DWM border color, global hooks, Win32
  window ops); no virtual desktop support.
- Wayland backend: architecture only - wlroots object graph created but no
  listeners ever wired up, no windows are actually managed.
- macOS backend: stub except Accessibility permission request and CGDisplay
  monitor enumeration.
- Lua config (`lua_manager.cc`): functional for scalar config get/set; key
  bindings stored the combo string but not the actual Lua closure; several
  `srd.*` functions (`spawn`, `load`, `notify`) were logging-only placeholders.
