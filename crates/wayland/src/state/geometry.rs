use super::*;

/// How long `sync_geometry` waits for a client to catch up to a previous
/// size-changing configure before giving up on the throttle and sending a
/// new one anyway - see `pending_size_configure`'s own doc comment for
/// the throttle itself. Generous relative to any real client's own
/// resize-and-recommit latency (a terminal reflowing text, a browser
/// re-laying-out a page), so this essentially never fires in practice;
/// it exists purely as the same kind of bounded self-heal this session's
/// DRM flip-pending watchdog already uses, not a tuning knob expected to
/// matter day to day.
const CONFIGURE_THROTTLE_TIMEOUT: Duration = Duration::from_millis(100);

impl CompState {

    /// Re-raises always-on-top windows in the `Space`.
    ///
    /// `WindowManager` keeps pinned windows last in its own stacking order,
    /// but the `Space` has an order of its own that decides what actually
    /// draws on top - so pinning is only real once it is pushed here.
    /// Called after anything that raises a window.
    pub(crate) fn raise_pinned(&mut self) {
        let pinned: Vec<WindowId> = self.wm.borrow().stacking_order().filter(|w| w.always_on_top).map(|w| w.id).collect();
        for id in pinned {
            if let Some(w) = self.id_to_window.get(&id).cloned() {
                self.space.raise_element(&w, false);
            }
        }
    }

    /// `Self::effective_frame`, but as a free function taking only the two
    /// fields it actually needs (`wm`, `id_to_window`) instead of `&self` --
    /// a render loop holding `self.udev`/`self.backend` mutably borrowed
    /// can't also pass `&self` to a method, since Rust can't see through a
    /// method call to know it only touches two unrelated fields. Called
    /// through the inherent method below wherever a plain `&self` is
    /// available (input handling, `redraw_decoration_buffer`); this
    /// version exists for the render loops specifically.
    pub(crate) fn effective_frame_of(wm: &Rc<RefCell<WindowManager>>, id_to_window: &HashMap<WindowId, DWindow>, id: WindowId, geom: srdwm_core::Rect) -> srdwm_core::Rect {
        // A version of this function briefly (this same session) skipped
        // the committed-size correction below entirely during an active
        // resize, on the reasoning that trusting the client's stale last
        // commit over this compositor's own live drag target was what made
        // the border visibly lag behind content while dragging. Reverted:
        // that fix was real for *position*-independent reasoning but wrong
        // in a more important way - every caller of this function that
        // reads a *bitmap*-backed element (the titlebar, the top/bottom
        // border strip's own rounded-corner bitmap, both built by `redraw_
        // decoration_buffer`) uses this rect's width/height to size the
        // `src` crop rectangle it samples that bitmap with. Making this
        // function return the *live* drag target while the underlying
        // bitmap was still sized for whatever the *last commit* actually
        // was meant that crop could end up larger than the real bitmap's
        // own stored dimensions - `MemoryRenderBufferRenderElement::from_
        // buffer` does not validate `src` against the texture's real size,
        // so an oversized crop reads as an out-of-bounds texture sample
        // (stretched/repeated/garbage pixels, not a clean error).
        //
        // Now reinstated, safely: this returns the *live* drag target
        // (`geom`, unmodified) while a resize of this specific window is
        // active, same as the reverted attempt did - but two things are
        // different this time, together closing the gap that made it
        // unsafe rather than just re-taking the risk:
        // 1. `input::pointer::handle_pointer_position` now calls
        //    `redraw_decoration_buffer` on every resize motion tick (see
        //    its own doc comment), not just on a real client commit, so
        //    the bitmap itself keeps catching up to this same live value
        //    almost every frame instead of staying pinned to the last
        //    commit for the resize's whole duration.
        // 2. Independent of how well-synced that keeps the two, every
        //    render-loop call site that turns this rect's width/height
        //    into a `src` crop now clamps it against `decoration_
        //    signatures`' own recorded `(width, height)` - the bitmap's
        //    own *actual* last-built size, tracked there already for
        //    unrelated caching reasons - before handing it to `from_
        //    buffer`. That clamp is what actually prevents the out-of-
        //    bounds read now, structurally, regardless of any remaining
        //    timing gap between this function and the next rebuild; this
        //    branch existing is what keeps that gap small in practice
        //    rather than a full commit-cycle wide.
        if wm.borrow().resizing_window() == Some(id) {
            return geom;
        }
        let Some(w) = wm.borrow().window(id).cloned() else { return geom };
        let Some(dwindow) = id_to_window.get(&id) else { return geom };
        // `dwindow.geometry()` - `xdg_surface::set_window_geometry` - is,
        // per smithay's own implementation, that cached hint *intersected*
        // with `bbox()`, falling back to `bbox()` only if the client never
        // set one. Nothing in the protocol obliges a client to resend the
        // hint on every resize (only when the visible-content-vs-buffer
        // relationship itself changes), and `intersection()` can never
        // return something larger than its smaller operand - so once a
        // client's cached hint is smaller than its current real buffer,
        // `geometry()` stays clamped there permanently, no matter how much
        // larger the buffer grows afterward. Confirmed live: after a
        // *passive* tiling reflow (this window resized only as a side
        // effect of a sibling moving, no direct action on this window
        // itself), Firefox's real content filled the correct, much larger
        // area immediately, but its border/decoration - both driven by
        // this function, see `redraw_decoration_buffer`'s and the render
        // loops' own call sites - stayed rendered at a small fraction of
        // that, unchanged for several seconds (long past both animation
        // settling and any reasonable commit-throttle window), until an
        // unrelated maximize/restore cycle on the same window happened to
        // prompt Firefox into resending a fresh hint and self-correcting.
        //
        // A first fix switched outright to `bbox()` - the real bounding
        // box of the window's current surface tree, which updates on every
        // commit unconditionally - on the reasoning that `sync_geometry`
        // already unconditionally tells every window it is tiled on all
        // four sides specifically so a compliant client reserves no
        // invisible shadow margin, so nothing would be lost by no longer
        // excluding one. Wrong: confirmed live via temporary diagnostic
        // logging, Chrome reserves a real, correctly-current 10px margin on
        // all four sides regardless of the tiled hint (Firefox, the window
        // that exposed the original bug, does not - the two disagree on
        // this, not just on how quickly they resend the hint). Raw `bbox()`
        // gave the border/content mask Chrome's *entire* buffer, margin
        // included - 20px wider and taller than its real visible chrome on
        // each axis, with no compensating position shift - so the rounded
        // border curve traced a rectangle Chrome's real content never
        // reached, and its true, still-square corner poked straight through
        // the curve instead of being hidden by it.
        //
        // The fix keeps both properties at once: a margin, `dwindow_
        // geometry.loc`, assumed symmetric (left == right, top == bottom --
        // true of every real CSD shadow margin observed here: a fixed
        // design constant, not something that scales with window size) is
        // far more durable than the rest of that same hint. A resize
        // changes a client's real content size; it does not change how
        // large that client's own shadow is, so the margin has no
        // equivalent staleness window even while the hint's absolute size
        // does. Subtracting it from the always-fresh `bbox()` - instead of
        // trusting the hint's own absolute size (stale-prone) or using
        // bbox() raw (margin-blind) - gets a content rect that is both
        // current and correctly excludes the invisible margin, for a
        // client that reserves one (Chrome) and one that doesn't (Firefox,
        // where the hint's `.loc` is always `(0, 0)` and this reduces to
        // plain `bbox()`) alike.
        let dwindow_geometry = dwindow.geometry();
        let bbox = dwindow.bbox();
        let margin = (dwindow_geometry.loc.x.max(0), dwindow_geometry.loc.y.max(0));
        let content = Rectangle::new(bbox.loc, (bbox.size.w - 2 * margin.0, bbox.size.h - 2 * margin.1).into());
        if content.size.w <= 0 || content.size.h <= 0 {
            // No real committed content yet - racing the first commit
            // right after creation, most likely. Nothing to correct
            // against, so fall back to the requested rect rather than
            // collapsing every dimension down to (near) zero.
            return geom;
        }
        // `content` (`bbox()`) is in the same *logical* points as
        // `xdg_surface::set_window_geometry` would have been, same as
        // `sync_geometry`'s own `size` going the other direction (see that
        // function's matching doc comment). Every caller of this method
        // (border, shadow, occlusion,
        // resize-margin hit-test) works in this compositor's own physical
        // convention, same as `geom` - so `content.size` needs converting
        // back to physical here, the same `* scale` `sync_geometry` divides
        // by on the way out, or a window on a scaled monitor gets a
        // border/shadow drawn at the *logical* size while its real content
        // renders at a different *physical* one. On a monitor with
        // `scale == 1.0` logical and physical are numerically identical, so
        // this was invisible until this session's own auto-scale feature
        // gave a monitor a non-1.0 value - reported live as a purple
        // border sitting visibly detached, to the east and south, from an
        // undecorated (CSD) window's real content once that happened.
        let scale = wm.borrow().monitors().iter().find(|m| m.id == w.monitor).map(|m| m.scale).unwrap_or(1.0);
        let content_physical = ((content.size.w as f64 * scale).round() as i32, (content.size.h as f64 * scale).round() as i32);
        let band = if w.decorated { TITLEBAR_HEIGHT as i32 } else { 0 };
        srdwm_core::Rect { x: geom.x, y: geom.y, width: content_physical.0.max(0) as u32, height: (band + content_physical.1.max(0)) as u32 }
    }

    /// The rect a window's border, shadow, occlusion test, and resize-
    /// margin hit-test should actually use - `geom` (the requested target,
    /// or mid-animation the interpolated rect) with its width/height
    /// replaced by what the client's own surface really committed, when
    /// that's known and non-degenerate. `x`/`y` are left untouched: the
    /// top-left corner is already correctly anchored by `content_offset`
    /// elsewhere (`sync_geometry`/the render loops), only the far edge can
    /// end up wrong.
    ///
    /// `Window.geometry` (what `geom`'s width/height ultimately come from)
    /// is this compositor's own *request* - what `sync_geometry` asked the
    /// client to become via `xdg_toplevel::configure`'s `size`. Nothing
    /// before this ever read back whether the client actually complied.
    /// Most do, to the pixel - but a client with its own internal size
    /// quantization (a terminal emulator, snapping its real content to a
    /// whole number of character cells) can settle on a slightly different
    /// real size than what was requested, without that being any kind of
    /// protocol violation. Every caller of this method used to read `geom`
    /// directly regardless, so the border (and the shadow, and the resize-
    /// margin hit-test) kept drawing/testing at the *asked-for* edge while
    /// the client's real content stopped a few pixels short of it --
    /// reported live as a transparent gap between a terminal's content and
    /// srdwm's own border, letting the desktop show through underneath.
    ///
    /// Niri's own `LayoutElement::size` (`src/window/mapped.rs` in its
    /// source) is the model this follows: its entire layout - tile size,
    /// border, focus ring - is driven by `self.window.geometry().size`,
    /// the client's real, committed value, never by whatever niri itself
    /// originally requested. This mirrors that for the specific things
    /// srdwm draws that have to visually hug the real edge. Deliberately
    /// narrow, not a wholesale switch: `Space` positioning, the
    /// `xdg_toplevel::configure` math itself, and tiling layout all keep
    /// reading `Window.geometry` unchanged - those are about this
    /// compositor's own bookkeeping staying self-consistent, not about
    /// matching a client's real pixels.
    pub(crate) fn effective_frame(&self, id: WindowId, geom: srdwm_core::Rect) -> srdwm_core::Rect {
        Self::effective_frame_of(&self.wm, &self.id_to_window, id, geom)
    }

    pub(crate) fn sync_geometry(&mut self, id: WindowId) {
        // A pending `anim_from` (set by `toggle_maximize`/`toggle_fullscreen`,
        // or by `new_managed_window` for the open-slide) means the target
        // geometry below is where this window is *headed*, not where it
        // should appear right now - register (or replace) a tween and use
        // `WindowAnim::current_rect` in its place for this call and every
        // `tick_animations` call afterward, until it completes. `take()`
        // both reads and clears it, so a later, non-animated `sync_geometry`
        // call for the same window (an ordinary drag/resize frame) goes
        // straight back to applying `geometry` immediately, as before.
        let anim_from = self.wm.borrow_mut().window_mut(id).and_then(|w| w.anim_from.take());
        let Some((target, decorated, maximized, fullscreen, monitor)) =
            self.wm.borrow().window(id).map(|w| (w.geometry, w.decorated, w.maximized, w.fullscreen, w.monitor))
        else {
            return;
        };
        // This compositor's own placement/geometry tracking is physical
        // pixels throughout (see `Platform::monitors()`'s own doc comment
        // on that choice); `xdg_toplevel::configure`'s `size` is specified
        // to carry *logical* points, always, independent of which output a
        // window is on. Every output was `1.0` before this session's own
        // auto-scale feature existed, so physical and logical were
        // numerically identical and this conversion's absence was
        // invisible. Falls back to `1.0` (no conversion) if this window's
        // own monitor can't be resolved - the same "assume unscaled
        // rather than guess" default `MonitorInfo::scale`'s own doc
        // comment already uses for a disabled output.
        let scale = self.wm.borrow().monitors().iter().find(|m| m.id == monitor).map(|m| m.scale).unwrap_or(1.0);
        if let Some(from) = anim_from {
            let duration_ms = self.wm.borrow().animation_duration_ms;
            if from != target && duration_ms > 0 {
                self.window_anims
                    .insert(id, WindowAnim { from, to: target, start: Instant::now(), duration: Duration::from_millis(duration_ms as u64) });
            }
        }
        let geom = self.window_anims.get(&id).map(WindowAnim::current_rect).unwrap_or(target);
        // The titlebar band is only actually reserved when there is one --
        // an undecorated window (client-side decoration, see
        // `set_decorated_from_mode`) gets the whole of `geom` as content,
        // not `geom` minus a band that's no longer being drawn. Without
        // this, a window that negotiated client-side decoration kept the
        // same 30px gap at its top anyway: our titlebar wasn't drawn there
        // (correctly), but the content was still offset down and told it
        // was 30px shorter than the window actually is, leaving a blank
        // strip and the frame sitting visibly wrong relative to what's
        // inside it.
        let band = if decorated { TITLEBAR_HEIGHT as i32 } else { 0 };
        // Position always moves with the pointer; only a size change needs a
        // client configure or a titlebar re-render (see `last_synced_size`'s
        // doc comment).
        //
        // Converted to logical points here, before anything below reads
        // `size` - `xdg_toplevel::configure` is specified to carry
        // logical points, and `w.geometry()` (what the throttle check
        // below compares a client's real commit against) is a client's own
        // `xdg_surface::set_window_geometry`, logical by the same
        // specification - so keeping the rest of this function in that
        // one space, not switching back to physical partway through, is
        // what actually keeps every comparison here meaningful.
        //
        // This has a real, desirable second effect beyond fixing the unit
        // mismatch itself: a window that crosses onto a monitor with a
        // different scale, at the *same* physical size (an ordinary drag
        // never changes `geom.width`/`geom.height`), now computes a
        // *different* logical size purely from `scale` changing --
        // correctly triggering a fresh configure asking the client to
        // resize to match, the same way real desktop environments keep a
        // window's true on-screen footprint consistent across a DPI
        // change. Before this, a plain cross-monitor drag sent no configure
        // at all (physical size hadn't changed), so the client kept
        // rendering its old logical size at the new monitor's different
        // scale while this compositor's own border kept drawing at the
        // physical rect it always had - reported live as a window's
        // border ending up visibly detached from its own content after
        // being dragged to the other monitor.
        let size_physical = (geom.width as i32, geom.height as i32 - band);
        let size = ((size_physical.0 as f64 / scale).round() as i32, (size_physical.1 as f64 / scale).round() as i32);
        // Peeked, not inserted yet - only actually updated once a
        // configure for `size` is decided below, so a size that keeps
        // changing tick to tick while throttled (an active drag didn't
        // stop just because the client hasn't caught up yet) is still
        // correctly seen as "different from what's actually been sent"
        // on every later tick, not just the first.
        let size_changed = self.last_synced_size.get(&id).copied() != Some(size);
        let mut moved = false;
        if let Some(w) = self.id_to_window.get(&id) {
            // `w.geometry().loc` is the client's own `xdg_surface::
            // set_window_geometry` offset - a CSD client (GTK4/Firefox
            // concretely) declares its real visible content as a sub-rect
            // inset within a larger buffer that also reserves an invisible
            // shadow margin, even once the tiled-state hint below has told
            // it to skip drawing that shadow.
            //
            // This used to be subtracted from `location` right here, on the
            // reasoning that `space` needed to be told about it explicitly,
            // the same way `render_udev_frame`/`winit/render.rs` do for
            // drawing. That reasoning was wrong about `Space` specifically:
            // smithay's own `SpaceElement for Window` reports `geometry()`
            // as `self.geometry()` (this exact `content_offset`, non-zero
            // `.loc` included), and `Space`'s internal `render_location()`
            // (what every hit-test - `element_under`, and so `refresh_
            // pointer_focus`'s `win_relative = pos - loc` - actually reads)
            // already computes `location - element.geometry().loc` on its
            // own, unconditionally, for every mapped element. Subtracting
            // `content_offset` again here meant `Space`'s own tracked
            // position ended up short by *two* `content_offset`s, not one --
            // confirmed live via temporary diagnostic logging on both sides:
            // this call computing a correct, single-subtraction position,
            // and `Space::element_under` reporting a position exactly one
            // more `content_offset` short of it for the same window on the
            // very same commit. The render loops' own manual subtraction is
            // unaffected and stays - they position elements by hand,
            // entirely bypassing `Space`'s automatic handling, so they still
            // have to do this themselves; `xwayland.rs`'s own `map_element`
            // calls already never did this (X11 windows have no equivalent
            // shadow-margin geometry), which in hindsight was the correct
            // pattern being followed there all along.
            self.space.map_element(w.clone(), (geom.x, geom.y + band), false);
            moved = true;
            if let Some(top) = w.toplevel() {
                // xdg-shell position is a purely compositor-side concept --
                // the client is never told it - so only a size change
                // needs a configure here.
                //
                // Throttled to at most one size-changing configure "in
                // flight" per window, the same way niri does (`window/
                // mapped.rs`'s `ConfigureIntent::Throttled`) - see
                // `pending_size_configure`'s own doc comment for why: this
                // used to send a fresh configure on every single pointer-
                // motion tick of an active resize regardless of whether the
                // client had caught up to the *previous* one yet, which a
                // fast pointer (a real high-poll-rate mouse, niri's own
                // stated motivation for the same throttle) could easily
                // outrun into a real backlog. `w.geometry().size` is the
                // client's actual last-committed content size - once it
                // matches whatever was last sent, that configure is
                // considered caught up and the throttle clears on its own,
                // no separate ack-tracking needed. Bounded by
                // `CONFIGURE_THROTTLE_TIMEOUT` regardless, so a client that
                // never catches up for any reason (slow, buggy, wedged)
                // can't jam resizing shut forever - the same self-healing
                // shape as this session's own DRM flip-pending watchdog.
                let throttled = self.pending_size_configure.get(&id).is_some_and(|(pending_size, sent_at)| {
                    let caught_up = w.geometry().size.w == pending_size.0 && w.geometry().size.h == pending_size.1;
                    !caught_up && sent_at.elapsed() < CONFIGURE_THROTTLE_TIMEOUT
                });
                if size_changed && !throttled {
                    self.last_synced_size.insert(id, size);
                    self.pending_size_configure.insert(id, (size, Instant::now()));
                    top.with_pending_state(|state| {
                        state.size = Some(size.into());
                        // No configure from this compositor, ever, set any
                        // `xdg_toplevel` state bit at all before this --
                        // confirmed by grepping the whole crate for
                        // `xdg_toplevel::State`, zero hits. GTK4 (Firefox
                        // concretely) reads the tiled bits to decide whether
                        // to reserve its own invisible client-side shadow
                        // margin around its actual content, independent of
                        // whether decoration is server- or client-side --
                        // with none ever sent, it always assumed "floating,
                        // might need a shadow" and kept reserving one. That
                        // margin sits inside the committed buffer but is
                        // functionally invisible, so this compositor's own
                        // border - drawn at the *full* geometry, margin
                        // included, since nothing here knew the margin
                        // existed - ended up visibly offset from where the
                        // client's real chrome began. Reported live as
                        // Firefox's border "not with the window," and more
                        // generally never feeling like part of it. Setting
                        // all four unconditionally (the same technique
                        // river/dwl use) tells every window it's flush
                        // against something and should skip its own shadow,
                        // regardless of whether it's actually in a tiled
                        // layout - which is the outcome actually wanted:
                        // this compositor draws the frame, so nothing else
                        // should also be reserving room for one.
                        state.states.set(xdg_toplevel::State::TiledLeft);
                        state.states.set(xdg_toplevel::State::TiledRight);
                        state.states.set(xdg_toplevel::State::TiledTop);
                        state.states.set(xdg_toplevel::State::TiledBottom);
                        // Same "no configure from this compositor ever set
                        // this" gap as the tiled bits above, confirmed the
                        // same way (grepped the whole crate for `State::
                        // Maximized`/`State::Fullscreen` outside foreign-
                        // toplevel-management, which is a *different*
                        // protocol read by external tools like a taskbar,
                        // not the client's own `xdg_toplevel` configure --
                        // zero hits there before this). The window was
                        // resized to the full monitor rect and told it was
                        // tiled on every side, but never actually told via
                        // the real protocol mechanism for it that it was
                        // maximized or fullscreen at all - indistinguishable
                        // from an ordinary tiled-to-the-edges floating
                        // window as far as the client could tell. Reported
                        // live as fullscreen leaving a persistent gap along
                        // one edge (Firefox keeping some of its own chrome
                        // logic that specifically keys off genuinely
                        // *knowing* it's fullscreen, not just being resized
                        // to fullscreen-sized). `unset` the other explicitly
                        // when only one applies - `WindowManager::
                        // toggle_fullscreen`/`toggle_maximize` are mutually
                        // exclusive, but nothing here should assume that
                        // holds forever just because it does today.
                        if maximized {
                            state.states.set(xdg_toplevel::State::Maximized);
                        } else {
                            state.states.unset(xdg_toplevel::State::Maximized);
                        }
                        if fullscreen {
                            state.states.set(xdg_toplevel::State::Fullscreen);
                        } else {
                            state.states.unset(xdg_toplevel::State::Fullscreen);
                        }
                    });
                    top.send_configure();
                }
            } else if let Some(x11) = w.x11_surface() {
                // Unlike xdg-shell, an X11 client's real on-screen position
                // is part of its own window state - it has to be told on
                // every move, not just every resize, the same way a real
                // X11 window manager sends continuous `ConfigureNotify`
                // during an interactive drag. Without this branch at all,
                // `sync_geometry` never reconfigured an XWayland window a
                // second time past its initial map: `space.map_element`
                // above still moved smithay's own tracked position (see
                // `resync_stacking_order`'s doc comment for the real
                // z-order side effect that has, since fixed below) and the
                // border/titlebar still redrew at the new `Window.geometry`
                // (both read it fresh every frame), but the real X11
                // client window was never told to move or resize - any
                // drag, resize, maximize, edge-snap, or tiling re-layout of
                // an XWayland-backed app left its actual content frozen at
                // its original position/size forever while srdwm's own
                // decoration moved freely around it.
                let _ = x11.configure(Rectangle::new((geom.x, geom.y + band).into(), size.into()));
            }
        }
        // Not gated on `self.decorations.contains_key(&id)` - that map only
        // ever holds an entry for a *decorated* window (see
        // `redraw_decoration_buffer`, which only inserts into it when
        // `w.decorated`), so that gate was permanently false for every
        // undecorated/CSD window, even one with `border_width > 0`. Its
        // border bitmaps were rendered once at creation and never rebuilt on
        // any later resize - reported live as the border "not truly around"
        // the window after resizing. `redraw_decoration_buffer` already
        // self-guards via `decoration_signatures` (see its own doc comment),
        // so calling it unconditionally here costs nothing once the size
        // genuinely hasn't changed the rasterized output.
        if size_changed {
            self.redraw_decoration_buffer(id);
        }
        // See `resync_stacking_order`'s doc comment: `map_element` above
        // always re-stacks its target to the top of `Space`'s own order as
        // a side effect of updating position, `activate` or not - and
        // `sync_geometry` runs for reasons with nothing to do with raising
        // a window (a title changing, an ordinary resize frame), so left
        // uncorrected this silently, non-deterministically desynced
        // `Space`'s notion of "on top" from `WindowManager`'s.
        if moved {
            self.resync_stacking_order();
        }
    }
}
