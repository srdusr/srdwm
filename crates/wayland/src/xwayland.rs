//! XWayland integration: lets legacy X11-only clients (anything that can't
//! speak the Wayland protocol natively) run inside the Wayland session,
//! bridged into the same `srdwm_core::WindowManager`/`Space` pipeline as
//! native `xdg-shell` windows.
//!
//! Only wired up for the udev/DRM backend (`udev.rs`) for now: XWayland's
//! window-manager side (`X11Wm::start_wm`) is driven entirely through a
//! `calloop` event loop, which only the udev backend has - the nested
//! winit backend still drives its own manual poll loop (see `lib.rs`'s
//! module docs). Adding a second, XWayland-only `calloop::EventLoop` to the
//! winit backend too is possible but left as a follow-up.
//!
//! Scope: regular (server-managed) windows go through the exact same
//! `WindowManager::add_window`/decoration/hit-test path as xdg-shell
//! windows - an X11 app gets tiled, placed by `SmartPlacement`, and
//! decorated with our drawn titlebar exactly like a native Wayland client.
//! Override-redirect windows (menus, tooltips, drag images) are
//! deliberately *not* run through `WindowManager` at all - matching real
//! ICCCM semantics, no WM is ever supposed to manage or decorate them --
//! they're mapped into `Space` at whatever geometry the client itself
//! requests. Selections/clipboard, XSETTINGS, and RandR primary-output
//! sync are not implemented (all have harmless no-op default trait
//! methods in `XwmHandler`).

use smithay::reexports::calloop::LoopHandle;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{Reorder, ResizeEdge as X11ResizeEdge, XwmId};
use smithay::xwayland::{X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler};
use smithay::{delegate_xwayland_shell, desktop::Window as DWindow};

use srdwm_core::{Event as CoreEvent, ResizeEdge, Window as CoreWindow, TITLEBAR_HEIGHT};

use crate::CompState;

pub(crate) type X11Window = smithay::xwayland::xwm::X11Window;

/// Spawns XWayland and registers the calloop sources that drive it: the
/// `XWayland` process/readiness source, and (once ready) `X11Wm`'s own
/// internal X11-connection source. Both are owned by the event loop after
/// `insert_source`, not by any struct here - dropping the loop (or the
/// `X11Wm` on disconnect) is what shuts things down.
///
/// Before spawning, arranges for XWayland to run with `-shm`: this
/// compositor only ever supports `wl_shm` (see `udev.rs`'s module docs on
/// why it's deliberately software-only, no GBM/DMA-BUF), and XWayland's
/// default behavior of trying `glamor` first and falling back to
/// shared-memory buffers on failure does *not* fall back to the
/// `xwayland_shell_v1` protocol for associating X11 windows with
/// `wl_surface`s - confirmed by tracing the actual Wayland protocol
/// exchange with `WAYLAND_DEBUG=1`. Starting with `-shm` from the outset
/// avoids the failed glamor attempt entirely, which keeps XWayland on the
/// code path that does use `xwayland_shell_v1` correctly.
///
/// `smithay::xwayland::XWayland::spawn` builds its `Xwayland` command line
/// internally with a fixed argument list (no way to add `-shm` directly),
/// and can't be bypassed either: the `XWaylandClientData` type it inserts
/// as the spawned client's data has private fields, so nothing outside
/// smithay can construct one, and `X11Wm`/the internal surface-association
/// commit hook both depend on the client's data specifically being that
/// type. Instead, a tiny wrapper script shadows `Xwayland` on `PATH`
/// (`Command::new("Xwayland")`'s lookup honors the `PATH` smithay copies
/// from this process's own environment) and always re-execs the real
/// binary with `-shm` prepended.
pub(crate) fn spawn(handle: &LoopHandle<'static, CompState>, display_handle: &smithay::reexports::wayland_server::DisplayHandle) -> std::io::Result<()> {
    if let Err(e) = ensure_shm_wrapper_on_path() {
        log::warn!("could not set up an -shm wrapper for XWayland ({e}); XWayland windows will likely fail to render - see xwayland.rs's `spawn` docs");
    }

    let (xwayland, client) = XWayland::spawn(display_handle, None, std::iter::empty::<(String, String)>(), true, std::process::Stdio::null(), std::process::Stdio::null(), |_| ())?;

    let handle_for_ready = handle.clone();
    handle
        .insert_source(xwayland, move |event, _, data: &mut CompState| match event {
            XWaylandEvent::Ready { x11_socket, display_number } => {
                log::info!("XWayland ready on display :{display_number}");
                match X11Wm::start_wm(handle_for_ready.clone(), x11_socket, client.clone()) {
                    Ok(wm) => data.xwm = Some(wm),
                    Err(e) => log::error!("failed to start X11 window manager for XWayland: {e}"),
                }
            }
            XWaylandEvent::Error => log::error!("XWayland exited unexpectedly during startup"),
        })
        .map_err(|e| std::io::Error::other(format!("failed to register XWayland source: {e}")))?;
    Ok(())
}

/// Writes a small shell script named `Xwayland` to a private directory and
/// prepends that directory to this process's own `PATH` - the next
/// `Command::new("Xwayland")` (namely `XWayland::spawn`'s, which copies
/// `PATH` from this process's environment into the child's) resolves to
/// the wrapper instead of the real binary. The wrapper always re-execs the
/// real `Xwayland` with `-shm` prepended to whatever arguments it was
/// given, so it's transparent to everything else `spawn` sets up.
fn ensure_shm_wrapper_on_path() -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let real_xwayland = find_on_path("Xwayland").ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "Xwayland not found on PATH"))?;

    let wrapper_dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("srdwm-xwayland-shm-wrapper");
    std::fs::create_dir_all(&wrapper_dir)?;

    let wrapper_path = wrapper_dir.join("Xwayland");
    let quoted = shell_single_quote(&real_xwayland.to_string_lossy());
    std::fs::write(&wrapper_path, format!("#!/bin/sh\nexec {quoted} -shm \"$@\"\n"))?;
    let mut perms = std::fs::metadata(&wrapper_path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&wrapper_path, perms)?;

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let mut new_path = wrapper_dir.into_os_string();
    new_path.push(":");
    new_path.push(old_path);
    // SAFETY: called once, synchronously, before any XWayland process (or
    // any other thread) is spawned.
    unsafe { std::env::set_var("PATH", new_path) };
    Ok(())
}

fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var).map(|dir| dir.join(name)).find(|candidate| candidate.is_file())
}

/// POSIX single-quoting: safe for any byte sequence, including embedded
/// single quotes (`'` -> `'\''`).
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_single_quote_handles_embedded_quotes() {
        assert_eq!(shell_single_quote("/usr/bin/Xwayland"), "'/usr/bin/Xwayland'");
        assert_eq!(shell_single_quote("/it's/here"), r"'/it'\''s/here'");
    }
}

fn to_core_resize_edge(edge: X11ResizeEdge) -> ResizeEdge {
    match edge {
        X11ResizeEdge::Top => ResizeEdge::Top,
        X11ResizeEdge::Bottom => ResizeEdge::Bottom,
        X11ResizeEdge::Left => ResizeEdge::Left,
        X11ResizeEdge::Right => ResizeEdge::Right,
        X11ResizeEdge::TopLeft => ResizeEdge::TopLeft,
        X11ResizeEdge::TopRight => ResizeEdge::TopRight,
        X11ResizeEdge::BottomLeft => ResizeEdge::BottomLeft,
        X11ResizeEdge::BottomRight => ResizeEdge::BottomRight,
    }
}

impl CompState {
    /// Retries `finish_x11_window_setup` for every mapped X11 window still
    /// waiting on its `wl_surface` association - called on every
    /// compositor commit, since that association can complete without ever
    /// invoking `surface_associated` (see the module docs).
    pub(crate) fn retry_pending_x11_windows(&mut self) {
        if self.xwayland_pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.xwayland_pending);
        for surface in pending {
            self.finish_x11_window_setup(&surface);
            let done = self.xwayland_windows.get(&surface.window_id()).is_some_and(|id| self.id_to_window.contains_key(id));
            if !done {
                self.xwayland_pending.push(surface);
            }
        }
    }

    /// Finishes setting up a *server-managed* (non-override-redirect) X11
    /// window once both halves are known: it's been granted its map
    /// request, and XWayland has associated it with a `wl_surface`. Safe to
    /// call from either order's callback; idempotent.
    fn finish_x11_window_setup(&mut self, surface: &X11Surface) {
        let Some(wl_surface) = surface.wl_surface() else {
            log::debug!("xwayland: finish_x11_window_setup xid={:?} - no wl_surface yet", surface.window_id());
            return;
        };
        let Some(&id) = self.xwayland_windows.get(&surface.window_id()) else {
            log::debug!("xwayland: finish_x11_window_setup xid={:?} - not in xwayland_windows", surface.window_id());
            return;
        };
        if self.id_to_window.contains_key(&id) {
            log::debug!("xwayland: finish_x11_window_setup xid={:?} id={id} - already set up", surface.window_id());
            return;
        }
        log::info!("xwayland: finishing setup for xid={:?} id={id}", surface.window_id());
        let geom = self.wm.borrow().window(id).map(|w| w.geometry).unwrap_or_default();

        let dwindow = DWindow::new_x11_window(surface.clone());
        let _ = surface.configure(Rectangle::new((geom.x, geom.y + TITLEBAR_HEIGHT as i32).into(), (geom.width as i32, (geom.height - TITLEBAR_HEIGHT) as i32).into()));

        self.space.map_element(dwindow.clone(), (geom.x, geom.y + TITLEBAR_HEIGHT as i32), true);
        self.surface_to_id.insert(wl_surface, id);
        self.id_to_window.insert(id, dwindow);
        self.redraw_decoration_buffer(id);
        self.pending.borrow_mut().push(CoreEvent::WindowCreated(id));
    }

    fn remove_x11_window(&mut self, xid: X11Window) {
        let Some(id) = self.xwayland_windows.get(&xid).copied() else { return };
        if let Some(w) = self.id_to_window.remove(&id) {
            self.space.unmap_elem(&w);
        }
        self.decorations.remove(&id);
        self.wm.borrow_mut().remove_window(id);
        self.pending.borrow_mut().push(CoreEvent::WindowDestroyed(id));
    }
}

impl XWaylandShellHandler for CompState {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }

    fn surface_associated(&mut self, _xwm: XwmId, _wl_surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface, surface: X11Surface) {
        log::debug!("xwayland: surface_associated xid={:?}", surface.window_id());
        self.finish_x11_window_setup(&surface);
    }
}

delegate_xwayland_shell!(CompState);

impl XwmHandler for CompState {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().expect("XwmHandler callback fired without an X11Wm")
    }

    fn new_window(&mut self, _xwm: XwmId, window: X11Surface) {
        // Created but not (yet) mapped - nothing to do until a map request.
        log::debug!("xwayland: new_window xid={:?} title={:?}", window.window_id(), window.title());
    }

    fn new_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        // Not managed until it actually maps - see `mapped_override_redirect_window`.
        log::debug!("xwayland: new_override_redirect_window xid={:?}", window.window_id());
    }

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        log::debug!("xwayland: map_window_request xid={:?} title={:?} class={:?}", window.window_id(), window.title(), window.class());
        let id = {
            let mut wm = self.wm.borrow_mut();
            let id = wm.alloc_window_id();
            let mut w = CoreWindow::new(id, window.title());
            w.app_id = window.class();
            // Not `window.geometry()`: at `MapRequest` time this can still
            // be whatever tiny/default size the X11 window was *created*
            // with, before XWayland ever applies a `ConfigureRequest` --
            // and our own `configure_request` handler is deliberately a
            // no-op (we own layout for managed windows, matching
            // `new_managed_window`'s xdg-shell path below, which doesn't
            // trust the client's initial size either).
            w.geometry = srdwm_core::Rect::new(0, 0, 800, 600 + TITLEBAR_HEIGHT);
            wm.add_window(w);
            id
        };
        self.xwayland_windows.insert(window.window_id(), id);
        // Grant the map request *now*, unconditionally: per `X11Surface`'s
        // docs this is what tells XWayland the window may proceed, and it
        // does so before ever finishing our own wl_surface-dependent setup
        // (`finish_x11_window_setup` bails out until `wl_surface()`
        // resolves). Deferring `set_mapped` until after that check would
        // deadlock - XWayland doesn't seem to advance the window past
        // surface creation (no `get_xwayland_surface`/`set_serial`, no
        // buffer attach) until the map is granted.
        let _ = window.set_mapped(true);
        self.finish_x11_window_setup(&window);
        if !self.id_to_window.contains_key(&id) {
            self.xwayland_pending.push(window);
        }
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let Some(wl_surface) = window.wl_surface() else { return };
        let geom = window.geometry();
        let dwindow = DWindow::new_x11_window(window.clone());
        self.space.map_element(dwindow.clone(), (geom.loc.x, geom.loc.y), true);
        // Allocated only for the surface_to_id/id_to_window bookkeeping
        // `commit()` needs - deliberately never passed to
        // `WindowManager::add_window`: override-redirect windows are not
        // managed, per ICCCM.
        let id = self.wm.borrow_mut().alloc_window_id();
        self.xwayland_windows.insert(window.window_id(), id);
        self.surface_to_id.insert(wl_surface, id);
        self.id_to_window.insert(id, dwindow);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.remove_x11_window(window.window_id());
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let xid = window.window_id();
        self.remove_x11_window(xid);
        self.xwayland_windows.remove(&xid);
    }

    fn configure_request(&mut self, _xwm: XwmId, _window: X11Surface, _x: Option<i32>, _y: Option<i32>, _w: Option<u32>, _h: Option<u32>, _reorder: Option<Reorder>) {
        // We own layout for managed windows; smithay always sends back a
        // synthetic configure with the window's actual current geometry
        // after this callback returns (see `xwayland::xwm`'s `handle_event`
        // for `ConfigureRequest`), so there is nothing to do here - this
        // mirrors how `srdwm_x11::X11Platform` acks `ConfigureRequest` with
        // the client's real geometry rather than whatever it asked for.
    }

    fn configure_notify(&mut self, _xwm: XwmId, window: X11Surface, geometry: Rectangle<i32, Logical>, _above: Option<X11Window>) {
        // Only override-redirect windows are allowed to reposition
        // themselves at will; managed windows' geometry is owned by us.
        if !window.is_override_redirect() {
            return;
        }
        let Some(&id) = self.xwayland_windows.get(&window.window_id()) else { return };
        if let Some(w) = self.id_to_window.get(&id) {
            self.space.map_element(w.clone(), (geometry.loc.x, geometry.loc.y), false);
        }
    }

    fn resize_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32, resize_edge: X11ResizeEdge) {
        let Some(&id) = self.xwayland_windows.get(&window.window_id()) else { return };
        let pos = self.seat.get_pointer().map(|p| p.current_location()).unwrap_or_default();
        self.wm.borrow_mut().start_resize(id, to_core_resize_edge(resize_edge), pos.x as i32, pos.y as i32);
    }

    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
        let Some(&id) = self.xwayland_windows.get(&window.window_id()) else { return };
        let pos = self.seat.get_pointer().map(|p| p.current_location()).unwrap_or_default();
        self.wm.borrow_mut().start_drag(id, pos.x as i32, pos.y as i32);
    }
}
