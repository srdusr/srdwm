//! `_NET_WM_STRUT_PARTIAL`/`_NET_WM_STRUT` tracking - the X11 half of
//! monitor usable-area reservation, matching what the Wayland backends
//! already do for a `zwlr_layer_shell_v1` bar/dock's exclusive zone (see
//! `udev/platform.rs`'s `monitors()` and its own `non_exclusive_zone`
//! doc comment). Confirmed missing entirely, live, by a peer session
//! (`aegis`) building its own X11-backend bar: an override-redirect
//! `_NET_WM_WINDOW_TYPE_DOCK` window setting a correct `_NET_WM_STRUT_
//! PARTIAL` never shrank `srd monitors`' own reported usable rect at
//! all, so a real tiled client would have had its window placed right
//! underneath the bar. `grep -rl STRUT crates/x11/src` found nothing at
//! all before this file - the feature had simply never been built for
//! this backend, not a narrower bug in existing logic.

use super::*;

/// A strut reservation changed, so whatever `monitors()` would report has
/// too - `crates/srdwm/src/main.rs`'s own event loop already re-queries
/// `Platform::monitors()` and refreshes `WindowManager`'s cached list on
/// exactly this event (it's how output hotplug is handled), so reusing it
/// here is what actually makes a fresh strut reservation visible to `srd
/// monitors`/tiling/placement at all - the cached list otherwise only
/// changes on a real hotplug, never on a dock mapping or resizing itself.
/// The zero-sized placeholder `Monitor` is never read: that event handler
/// discards the payload and re-queries the real list unconditionally, the
/// same "cheap sentinel, ignored on arrival" shape the Wayland backends'
/// own equivalent trigger already uses (`layer_shell.rs`'s `layer_
/// destroyed`, on an exclusive-zone change).
pub(super) fn monitors_changed_event() -> Event {
    Event::MonitorAdded(Monitor::new(0, "", Rect::new(0, 0, 0, 0)))
}

impl X11Platform {
    /// Reads `window`'s current strut reservation, preferring `_NET_WM_
    /// STRUT_PARTIAL` (12 `CARDINAL`s: left, right, top, bottom, then each
    /// edge's own start/end span) and falling back to the older, span-free
    /// `_NET_WM_STRUT` (4 `CARDINAL`s: left, right, top, bottom) for a
    /// client that only sets that - per the EWMH spec, a plain `_STRUT`
    /// with no `_PARTIAL` reserves its margin across the *entire* length
    /// of that edge, which is what defaulting its span to `0..root extent`
    /// below encodes. `None` if neither property is set, or both are
    /// present but empty/malformed - the same "nothing reserved" answer
    /// either way.
    pub(super) fn read_strut(&self, window: XWindow) -> Option<Strut> {
        if let Ok(cookie) = self.conn.get_property(false, window, self.atoms._NET_WM_STRUT_PARTIAL, x11rb::protocol::xproto::AtomEnum::CARDINAL, 0, 12) {
            if let Ok(reply) = cookie.reply() {
                if let Some(v) = reply.value32().map(|it| it.collect::<Vec<u32>>()) {
                    if v.len() >= 12 {
                        let s = Strut {
                            left: v[0],
                            right: v[1],
                            top: v[2],
                            bottom: v[3],
                            left_start_y: v[4] as i32,
                            left_end_y: v[5] as i32,
                            right_start_y: v[6] as i32,
                            right_end_y: v[7] as i32,
                            top_start_x: v[8] as i32,
                            top_end_x: v[9] as i32,
                            bottom_start_x: v[10] as i32,
                            bottom_end_x: v[11] as i32,
                        };
                        return if s == Strut::default() { None } else { Some(s) };
                    }
                }
            }
        }
        let Ok(cookie) = self.conn.get_property(false, window, self.atoms._NET_WM_STRUT, x11rb::protocol::xproto::AtomEnum::CARDINAL, 0, 4) else {
            return None;
        };
        let reply = cookie.reply().ok()?;
        let v: Vec<u32> = reply.value32()?.collect();
        if v.len() < 4 || v[..4] == [0, 0, 0, 0] {
            return None;
        }
        let screen = &self.conn.setup().roots[0];
        let (w, h) = (screen.width_in_pixels as i32, screen.height_in_pixels as i32);
        Some(Strut {
            left: v[0],
            right: v[1],
            top: v[2],
            bottom: v[3],
            left_start_y: 0,
            left_end_y: h,
            right_start_y: 0,
            right_end_y: h,
            top_start_x: 0,
            top_end_x: w,
            bottom_start_x: 0,
            bottom_end_x: w,
        })
    }

    /// Called on `MapNotify` for any window this backend isn't already
    /// managing as a regular client (see that call site's own comment for
    /// why: a real panel/dock is typically override-redirect specifically
    /// to skip window management entirely, so it never reaches `manage_
    /// new_window`/`xid_to_core` the way an ordinary top-level does).
    /// Selecting `PROPERTY_CHANGE` here is what makes a later live resize
    /// of the bar (`update_strut_property`, on `PropertyNotify`) actually
    /// get noticed - without it, only the reservation this window had at
    /// the moment it first mapped would ever be seen. Returns `true` when
    /// a real reservation was found, so the caller can fire the same
    /// `Event::MonitorAdded` re-query trigger `monitors()`'s own live
    /// hotplug path already uses (see that call site's own comment for
    /// why this can't just call `monitors()` again itself - the cached
    /// list `srd monitors` actually reads lives in `WindowManager`, one
    /// layer up, not in this struct).
    pub(super) fn track_strut_window(&mut self, window: XWindow) -> bool {
        let Some(strut) = self.read_strut(window) else { return false };
        self.struts.insert(window, strut);
        let _ = self.conn.change_window_attributes(window, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE));
        let _ = self.conn.flush();
        true
    }

    /// `PropertyNotify` on `_NET_WM_STRUT`/`_NET_WM_STRUT_PARTIAL` for a
    /// window already being watched (`track_strut_window` selected the
    /// event mask that makes this fire at all) - re-reads and either
    /// updates or drops the reservation, matching whatever the client's
    /// new property value actually says (a bar shrinking its own reserved
    /// strip live, or clearing the property to reserve nothing at all).
    /// Returns `true` only when the reservation actually changed (not
    /// merely present) - see `track_strut_window`'s own doc comment for
    /// what the caller does with that.
    pub(super) fn update_strut_property(&mut self, window: XWindow) -> bool {
        let new = self.read_strut(window);
        let changed = self.struts.get(&window).copied() != new;
        match new {
            Some(strut) => {
                self.struts.insert(window, strut);
            }
            None => {
                self.struts.remove(&window);
            }
        }
        changed
    }

    /// `UnmapNotify`/`DestroyNotify` for a tracked strut window - a no-op
    /// `HashMap::remove` for every other window, so this is safe to call
    /// unconditionally from both handlers rather than needing its own
    /// "was this actually a strut window" check first. Returns `true`
    /// only when a real reservation actually existed and was removed --
    /// see `track_strut_window`'s own doc comment for what the caller
    /// does with that.
    pub(super) fn forget_strut_window(&mut self, window: XWindow) -> bool {
        self.struts.remove(&window).is_some()
    }

    /// `full`, shrunk by every tracked strut whose reserved band actually
    /// overlaps it - the X11 equivalent of `udev/platform.rs`'s `non_
    /// exclusive_zone`-based `usable` computation, called once per
    /// monitor from `monitors()`. See the free [`usable_rect`] function
    /// below (the actual math, kept separate so it's unit-testable
    /// without a live X11 connection) for exactly how.
    pub(super) fn usable_rect_for(&self, full: Rect) -> Rect {
        let screen = &self.conn.setup().roots[0];
        let screen_size = (screen.width_in_pixels as i32, screen.height_in_pixels as i32);
        usable_rect(full, screen_size, self.struts.values().copied())
    }
}

/// The actual shrink math behind [`X11Platform::usable_rect_for`], pulled
/// out as a free function of plain values (no live X11 connection needed)
/// specifically so it can be unit tested directly - `usable_rect_for`
/// itself needs `self.conn.setup()` for the root screen's own dimensions,
/// which only a real connected server can answer.
///
/// Every strut value is a distance in from the *screen's* own edge
/// (ICCCM/EWMH `_NET_WM_STRUT_PARTIAL`: `top` reserves root-absolute
/// `y ∈ [0, top)`, `bottom` reserves `y ∈ [screen_height - bottom,
/// screen_height)`, and so on) - not from this monitor's own edge, so
/// the reserved boundary is computed in root-absolute coordinates first
/// (needing `screen_size` for the bottom/right cases) and only then
/// intersected against `full`. A strut anchored on an edge this monitor
/// doesn't border at all, or whose own start/end span doesn't overlap
/// this monitor's extent on the perpendicular axis (a bar on a different
/// monitor entirely, in a multi-monitor setup), correctly contributes no
/// shrink either way, since its reserved boundary then falls outside
/// `full` on that axis.
pub(super) fn usable_rect(full: Rect, screen_size: (i32, i32), struts: impl Iterator<Item = Strut>) -> Rect {
    let (screen_w, screen_h) = screen_size;
    let (full_left, full_top) = (full.x, full.y);
    let (full_right, full_bottom) = (full.x + full.width as i32, full.y + full.height as i32);
    let (mut left, mut top, mut right, mut bottom) = (full_left, full_top, full_right, full_bottom);
    for strut in struts {
        if strut.top > 0 && strut.top_end_x > full_left && strut.top_start_x < full_right {
            top = top.max(strut.top as i32);
        }
        if strut.bottom > 0 && strut.bottom_end_x > full_left && strut.bottom_start_x < full_right {
            bottom = bottom.min(screen_h - strut.bottom as i32);
        }
        if strut.left > 0 && strut.left_end_y > full_top && strut.left_start_y < full_bottom {
            left = left.max(strut.left as i32);
        }
        if strut.right > 0 && strut.right_end_y > full_top && strut.right_start_y < full_bottom {
            right = right.min(screen_w - strut.right as i32);
        }
    }
    // Clamped against the monitor's own opposite edge, not just `0`: a
    // strut reservation declared against the *whole screen* can still
    // exceed a single monitor's own extent in a multi-monitor layout
    // (e.g. a bar `top=32` on a screen where this particular monitor's
    // usable band is otherwise smaller than that) - without this, an
    // overshoot on one axis could invert `left > right`/`top > bottom`
    // into a negative-size rect instead of clamping to "no usable space
    // left on this monitor". Each axis's two edges are clamped as
    // separate statements, in order (`left` before `right`, `top` before
    // `bottom`), not one combined tuple `let` - a first version of this
    // used `let (top, bottom) = (top.min(full_bottom), bottom.max(top))`
    // in one statement, which reads `top` on the right-hand side of both
    // tuple elements from the *pre-clamp* binding (a `let` only shadows
    // once the whole statement finishes), silently clamping `bottom`
    // against the wrong, oversized `top` - caught by a test asserting
    // `usable.height == 0` for a strut taller than the monitor, which
    // instead came back as `400`, not `0`.
    let left = left.min(full_right);
    let right = right.max(full_left).max(left);
    let top = top.min(full_bottom);
    let bottom = bottom.max(full_top).max(top);
    Rect::new(left, top, (right - left).max(0) as u32, (bottom - top).max(0) as u32)
}
