//! `wl_shm`/`wl_buffer`/`zwp_linux_dmabuf_v1`: the three buffer-transport
//! protocols, grouped together since none has more than a handful of lines
//! on its own.

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::ImportDma;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::shm::{ShmHandler, ShmState};

use crate::state::CompState;

impl ShmHandler for CompState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl BufferHandler for CompState {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl DmabufHandler for CompState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    /// Validates a client's dmabuf by actually importing it wherever a
    /// renderer is reachable from here, so a genuinely bad buffer (wrong
    /// modifier, format the renderer doesn't support) gets the protocol
    /// error instead of silently rendering garbage later.
    ///
    /// That's only the udev backend: its `PixmanRenderer` lives inside
    /// `self.udev` (`UdevState`), a field of this same struct.
    /// `PixmanRenderer` supports dmabuf import despite being a pure
    /// software renderer - `dmabuf_formats()` only advertises the Linear
    /// modifier, which it imports by mmap'ing the buffer and reading it
    /// directly as pixels, no GPU involved. This is what actually answers
    /// `docs/PANEL_SUPPORT_TODO.md`'s P0.3: GTK4 allocates via its own
    /// EGL/gbm path against the real DRM render node (untouched by this
    /// compositor either way) and hands the result here as a Linear-
    /// modifier dmabuf, which pixman can read straight off.
    ///
    /// The winit (nested/dev) backend's `GlesRenderer` lives on
    /// `WaylandPlatform`, a sibling of `CompState`, not reachable from a
    /// method on `CompState` itself. Accepted there without eager
    /// validation - the buffer still gets imported the same way every
    /// other buffer type already is, lazily, the first time it is actually
    /// rendered via `render_elements_from_surface_tree`. Real hardware,
    /// where P0.3 actually bites, always goes through the udev path.
    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        match self.udev.as_mut() {
            Some(udev) => match udev.renderer.import_dmabuf(&dmabuf, None) {
                Ok(_) => {
                    let _ = notifier.successful::<CompState>();
                }
                Err(e) => {
                    log::warn!("udev: rejecting dmabuf import: {e}");
                    notifier.failed();
                }
            },
            None => {
                let _ = notifier.successful::<CompState>();
            }
        }
    }
}
