//! `wl_compositor`/`wl_surface`: surface creation and the per-commit
//! bookkeeping every other protocol handler in this module tree depends on
//! (window mapping, layer-surface visibility, popup lifecycle).

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Client;
use smithay::wayland::compositor::{add_pre_commit_hook, with_states, CompositorClientState, CompositorHandler, CompositorState};
use smithay::wayland::shell::wlr_layer::LayerSurfaceCachedState;

use crate::state::{ClientState, CompState};

impl CompositorHandler for CompState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // Two possible client kinds now: our own `ClientState` for regular
        // Wayland clients, or smithay's `XWaylandClientData` for the single
        // XWayland client (see `xwayland.rs`) - both carry a
        // `CompositorClientState`, just under different wrapper types.
        if let Some(state) = client.get_data::<ClientState>() {
            return &state.compositor_state;
        }
        &client.get_data::<smithay::xwayland::XWaylandClientData>().expect("client is neither ours nor XWayland's").compositor_state
    }

    /// Workaround for a real smithay bug (see docs/PANEL_SUPPORT_TODO.md and
    /// `layer_destroyed` below): destroying a `zwlr_layer_surface_v1` role
    /// resets the surface's `LayerSurfaceCachedState` to
    /// `Default::default()` (size 0x0, no anchor) rather than removing it,
    /// but the pre-commit hook smithay itself registers at
    /// `get_layer_surface` time keeps validating that state against every
    /// future commit regardless of whether the role still exists --
    /// tripping its own `width/height 0 requested without ... anchors`
    /// check and posting `invalid_size`, which kills the client's whole
    /// connection over what is protocol-legal (committing a now-roleless
    /// surface).
    ///
    /// Fixed by registering our own pre-commit hook here, in `new_surface`
    /// - called at `wl_compositor.create_surface`, strictly before any
    /// later `get_layer_surface` on the same surface could register
    /// smithay's own hook. Hooks run in registration order (`tree.rs`:
    /// `pre_commit_hooks` is a plain `Vec`, pushed and iterated in order),
    /// so ours always runs first and can neutralize the stale reset state
    /// before smithay's hook ever inspects it. This depends on that
    /// ordering guarantee holding in future smithay versions - it isn't
    /// documented as an API contract, just an implementation detail
    /// confirmed against 0.7.0's source - so re-check this file against
    /// whatever smithay version replaces it.
    ///
    /// Cost: one closure registered per `wl_surface` (not just layer
    /// surfaces, since we don't know in advance which ones will become
    /// one), each a no-op unless that exact surface is in
    /// `dead_layer_surfaces`.
    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<CompState, _>(surface, |state, _dh, surface| {
            if !state.dead_layer_surfaces.contains(surface) {
                return;
            }
            with_states(surface, |states| {
                let mut cached = states.cached_state.get::<LayerSurfaceCachedState>();
                let pending = cached.pending();
                if pending.size.w == 0 && !pending.anchor.anchored_horizontally() {
                    pending.size.w = 1;
                }
                if pending.size.h == 0 && !pending.anchor.anchored_vertically() {
                    pending.size.h = 1;
                }
            });
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        smithay::backend::renderer::utils::on_commit_buffer_handler::<CompState>(surface);
        // XWayland's association of an X11 window with this wl_surface can
        // arrive at any point relative to the map request (see
        // `xwayland.rs`'s module docs); `surface_associated` handles the
        // common ordering, this retries the surfaces still waiting on a
        // commit to actually make that association queryable.
        self.retry_pending_x11_windows();
        if let Some(&id) = self.surface_to_id.get(surface) {
            if let Some(w) = self.id_to_window.get(&id) {
                w.on_commit();
            }
            // See `Window::size_is_provisional`'s own doc comment: before
            // anything below reads `Window::geometry` (`redraw_decoration_
            // buffer`, `sync_geometry`), give a window still waiting on its
            // own first real size a chance to adopt it from what `on_commit`
            // just recomputed, rather than keep rendering/configuring
            // against the guessed placeholder for one more round-trip.
            // The first commit that actually carries a buffer is when this
            // window becomes visible, so it is also when the open-slide
            // should start - see `windows_shown_once`. Registered here
            // rather than in `new_managed_window` because a role is created
            // well before a client paints (measured at ~800ms for a cold
            // terminal), which is long enough for the whole tween to finish
            // against an empty frame and for the window to simply appear,
            // already at rest, with no animation at all.
            if !self.windows_shown_once.contains(&id) && self.window_has_content(id) {
                self.windows_shown_once.insert(id);
                let mut wm = self.wm.borrow_mut();
                if wm.animations_enabled {
                    if let Some(win) = wm.window_mut(id) {
                        let g = win.geometry;
                        win.anim_from = Some(srdwm_core::Rect { y: g.y + crate::state::OPEN_SLIDE_OFFSET, ..g });
                    }
                }
            }
            self.adopt_provisional_size(id);
            // See `content_epoch`'s doc comment: this is the only per-commit
            // signal the udev backend's rounded-corner mask cache has to
            // invalidate itself, since content can change every frame,
            // independent of the geometry-driven points `redraw_decoration_
            // buffer` already runs at.
            *self.content_epoch.entry(id).or_insert(0) += 1;
            crate::state::sync_toplevel_metadata(self, id, surface);
            // `redraw_decoration_buffer` reads `dwindow.geometry()` (via
            // `effective_frame`) to size the border/titlebar/shadow against
            // what the client's surface *really* committed - but nothing
            // updates that value except this very commit
            // (`on_commit()` above). Without a call here, a client whose
            // first real commit settles at a different size than what was
            // requested (a terminal snapping to a whole character-cell
            // grid) wouldn't get corrected decoration until some unrelated
            // trigger (a resize, a focus change) happened to call this
            // again - cheap regardless, since the signature check inside
            // makes every commit that didn't actually change the *visible*
            // size an early return, not a real rebuild.
            self.redraw_decoration_buffer(id);
            // `sync_geometry` is what actually maps this window into
            // `self.space` at `geom.x - content_offset.x, ...` - the same
            // `content_offset` (`dwindow.geometry().loc`) the render loop
            // (`udev/render.rs`'s per-frame `pos` computation) reads fresh
            // on every single frame, straight off the live surface, not
            // from any cache. Before this call existed here, `self.space`
            // only got a fresh position from whichever *other* trigger last
            // called `sync_geometry` (a resize, `maximize_request`, a
            // decoration-mode change) - so a client that recommits a
            // *different* `xdg_surface::set_window_geometry` on its own,
            // with no accompanying resize (a GTK4/Firefox CSD window
            // shrinking its declared shadow margin once real content
            // replaces its first, provisional paint, concretely), left
            // `self.space`'s cached position silently stale while the
            // render loop kept self-correcting every frame - confirmed
            // live via temporary diagnostic logging: a window's real render
            // position and `self.space`'s own `element_under`-reported
            // position for it disagreed by exactly one `content_offset`,
            // 10 physical pixels on both axes for the Firefox window that
            // exposed it. `refresh_pointer_focus`'s content-click path
            // (`input.rs`) computes `win_relative` from *that* stale
            // position, not the render loop's fresh one - every click on
            // such a window was silently off by the same 10px the whole
            // time it stayed unmapped-and-remapped-by-nothing-else, which
            // reads as "clicks land near, but not on, whatever's visibly
            // there" - worst for a window's own small CSD buttons,
            // exactly what was reported live. Same idempotent-when-nothing-
            // moved shape as `redraw_decoration_buffer` above: `map_element`
            // itself is unconditional and cheap (a hashmap insert), and the
            // one potentially-expensive part - sending a fresh
            // `xdg_toplevel::configure` - stays gated on `size_changed`
            // and the existing throttle, both untouched, so a commit that
            // didn't change size never sends one just because this call is
            // now here too.
            self.sync_geometry(id);
        } else {
            // `surface` itself isn't a tracked window's root, but may be a
            // descendant (subsurface) of one - a real commit still
            // happened, just not on the surface `surface_to_id` keys off.
            // `masked_content_buffer`'s own resolver
            // (`rounded_corners_pixman::resolve_content_surface`) reads a
            // *child* subsurface's buffer directly for the common GTK4/
            // WebRender pattern (confirmed live: Firefox), so a repaint
            // that only ever commits that child - which is the normal
            // case, that's where the real content lives - must still
            // bump this window's own `content_epoch`, or the masked-
            // corner cache never sees a reason to invalidate and freezes
            // on whatever the first frame happened to show. Bounded to a
            // handful of hops purely as a safety net against a malformed
            // subsurface tree looping back on itself - a real one is
            // never more than one or two levels deep.
            let mut ancestor = smithay::wayland::compositor::get_parent(surface);
            for _ in 0..8 {
                let Some(parent) = ancestor else { break };
                if let Some(&id) = self.surface_to_id.get(&parent) {
                    *self.content_epoch.entry(id).or_insert(0) += 1;
                    break;
                }
                ancestor = smithay::wayland::compositor::get_parent(&parent);
            }
        }
        // Before `ensure_layer_initial_configure`: if this commit just
        // hid or re-showed a layer surface, `sync_layer_visibility` needs
        // to unmap/re-map it first, so the lookup that function does via
        // `layer_for_surface` sees the corrected state rather than acting
        // on stale membership in `LayerMap`'s own list.
        self.sync_layer_visibility(surface);
        self.ensure_layer_initial_configure(surface);
        // Advances a just-created popup from unmapped to mapped (needed for
        // `PopupManager::popups_for_surface`, which `popup_render_elements`
        // reads at render time) and prunes dead ones. Cheap and only does
        // real work on a popup-role surface, so doing it on every commit
        // rather than throttling is not worth the extra bookkeeping.
        self.popups.commit(surface);
        self.popups.cleanup();
    }
}
