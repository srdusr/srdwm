use super::*;

impl CompState {
    /// Renders and (if there was damage) page-flips a new frame on every
    /// head that is ready for one. A head with a flip still in flight is
    /// skipped this pass and picked up when its page-flip event arrives, so
    /// monitors on different refresh rates each run at their own pace
    /// instead of the slowest one gating the rest.
    pub(crate) fn render_udev_frame(&mut self) {
        // Real perf instrumentation, not a guess-fix: reported live as
        // "resizing seems slow", and this session's own investigation
        // (checked decoration-buffer caching, motion-path logging levels,
        // GPU-path config) found no smoking gun without an actual
        // measurement. Cheap when nothing's slow (one `Instant::now()` and
        // one comparison per frame, no allocation, no formatting unless
        // the threshold trips) - logs only when a frame actually misses a
        // 60fps budget, tagged with whether a resize/drag was in progress
        // at the time, so the next real resize either produces real
        // evidence this is a genuine per-frame cost during resize
        // specifically, or rules that out in favor of something else
        // (input latency, client-side redraw cost, a specific app).
        let frame_start = Instant::now();
        self.tick_animations();
        self.tick_hover_glyph_animation();
        self.tick_dirty_broadcasts();
        let locked = self.lock.locked;
        let elapsed = self.start_time.elapsed();
        // Drained before the `&mut self.udev` borrow below, so screencopy can
        // be serviced with the renderer that borrow owns.
        let mut captures = std::mem::take(&mut self.screencopy_pending);
        // Same reason: the cursor needs the renderer that borrow owns.
        let cursor_status = self.cursor_status.clone();
        let cursor_buffers = self.cursor_buffers.clone();
        // Same reason again: a native lock's capture step (below) needs
        // this, and `self.wm` can't be borrowed once `self.udev` is.
        let lock_blur_radius = self.wm.borrow().lock.blur_radius;
        // Same "capture before `self.udev`'s borrow starts" reason as
        // `cursor_status`/`cursor_buffers` above: global-space, so computed
        // once here rather than per head, each head's own per-frame push
        // below just re-offsets these same positions by its own `origin`.
        self.ensure_desktop_icons();
        let desktop_icon_render_list = self.desktop_icon_render_list();
        // Same "gather immutable state before `self.udev` is borrowed
        // mutably" reason as everything else in this block - used by the
        // per-window shadow push below to keep a shadow off any monitor its
        // own window does not occupy (`decoration::shadow_rect_clipped`).
        let monitor_bounds: Vec<srdwm_core::Rect> = self.wm.borrow().monitors().iter().map(|m| m.full_geometry).collect();
        // Same "gather immutable state before `self.udev` is borrowed"
        // reason again - both are read inside the per-head loop below,
        // which holds that borrow for its whole body.
        let drag_snap_preview = self.wm.borrow().drag_snap_preview();
        let accent_color = self.wm.borrow().theme.default_border_color;
        // Captured-and-blurred backgrounds collected during the per-head
        // loop below, applied via `self.capture_output` only after it
        // ends - `self.udev`'s mutable borrow is held for the whole loop
        // body, and that method needs the whole of `self`, not just the
        // one field the loop already has.
        let mut new_captures: Vec<(String, smithay::backend::renderer::element::memory::MemoryRenderBuffer)> = Vec::new();

        // Border geometry is in global space, independent of which head
        // renders it, so it's gathered once here rather than per head.
        // Buffers are pre-built for the same reason as `cursor_buffers`:
        // Rendered per window, front-to-back (topmost first), each window's
        // content immediately followed by its decoration and border --
        // fixes the same cross-window ordering bug documented in
        // `winit/render.rs`'s render loop: a background window's titlebar could
        // otherwise show through in front of the actually-focused window on
        // top of it, since decorations/borders used to be a single flat
        // layer drawn unconditionally above *every* window's content
        // regardless of real stacking order. `visible_windows_front_to_back`
        // is `WindowManager.order` reversed - not `visible_windows`, which
        // iterates the `windows` HashMap with no ordering guarantee - the
        // same "topmost first" convention `hit_test`/`window_at` use.
        // Fetched once here (`&mut self` fields, id_to_window/space lookups
        // happen per head below without needing `self` itself mutably) --
        // see the per-head loop for why decoration/border buffers still get
        // looked up fresh per head (head-local `origin` translation).
        // `layout_signature` folds in every visible window's id and rect
        // (order-sensitive, so a restack counts as a change too) - compared
        // against `udev.last_rendered_layout` below to force `ages = [0, 0]`
        // whenever a window moves, resizes, opens, closes, or restacks
        // between one frame and the next. Without this, a window vacating
        // part of the screen (closing, moving away, un-maximizing) can leave
        // stale content from its old position baked into whichever DRM
        // buffer isn't due to be written again for a while - confirmed
        // live: maximizing a window over a second one, then un-maximizing,
        // left a persistent ghost of the second window's old titlebar/status
        // text sitting in the vacated corner, unchanged across multiple
        // otherwise-idle frames, because `OutputDamageTracker`'s own element-
        // level diffing (relied on by the workspace-switch reset's own doc
        // comment above) evidently doesn't always catch a vacated region on
        // its own - most visible right where a rounded corner's mask should
        // have revealed the desktop underneath but instead revealed this.
        // Same "defensive, not a fix for a proven bug in the diffing itself"
        // reasoning as the workspace-switch reset already documents; this is
        // just a second, complementary trigger for the same reset.
        let (ids, layout_signature): (Vec<srdwm_core::WindowId>, u64) = if locked {
            (Vec::new(), 0)
        } else {
            let wm = self.wm.borrow();
            let mut ids = Vec::new();
            let mut sig: u64 = 0xcbf29ce484222325;
            for w in wm.visible_windows_front_to_back() {
                ids.push(w.id);
                for part in [w.id, w.geometry.x as u64, w.geometry.y as u64, w.geometry.width as u64, w.geometry.height as u64] {
                    sig ^= part;
                    sig = sig.wrapping_mul(0x100000001b3);
                }
            }
            (ids, sig)
        };
        let focused = self.wm.borrow().focused_id();
        // Stays default `false` here, unlike winit's `unwrap_or(true)` --
        // see `rounded_corners_pixman`'s module doc comment for the real
        // CPU cost this backend's masking technique has: a full row-by-row
        // buffer copy on *every commit* of a constantly-repainting client,
        // and that doc comment names video specifically as the case that
        // pays it in full, every frame, for as long as the feature is on.
        // Flipping this default was tried and reverted in the same pass
        // that fixed this backend's render-loop latency (see `poll_events`'
        // own history) - turning it on here would have directly undone
        // that fix for exactly the content (video) it mattered most for.
        // The actual "not all windows curved" complaint this was meant to
        // address (an undecorated/CSD window like Firefox, with no
        // compositor-drawn titlebar and only a thin border strip to look
        // rounded at all) is better addressed by giving that border strip
        // enough rows to show a real curve - see `ThemeConfig::
        // default_border_width`'s own doc comment.
        let rounded_corners_enabled = self.wm.borrow().rounded_corners_enabled.unwrap_or(false);
        let popup_targets = if locked { Vec::new() } else { crate::elements::popup_targets(self) };

        // Which heads are eligible, and what each needs, gathered before the
        // mutable borrow of `self.udev`. Both early-outs below give the
        // `captures` taken above nowhere to go this pass - put them back
        // rather than silently dropping a client's pending screenshot
        // because a VT switch happened to be in progress at that instant.
        let Some(udev) = self.udev.as_mut() else {
            self.screencopy_pending.extend(captures);
            return;
        };
        if !udev.active {
            self.screencopy_pending.extend(captures);
            return;
        }
        // A workspace switch changes *which windows* `custom_elements`
        // includes as drastically as a VT switch changes what's been
        // scanned out in the meantime (see `register_session_notifier`'s
        // own `head.ages = [0, 0]` for that case) - reported live as
        // visible corruption (stale, wrong-coloured blocks, worst on a
        // window that was actively repainting - a scrolling terminal --
        // right as the switch happened) confined to exactly the frame or
        // two around a switch, then never self-correcting, consistent with
        // one transient frame's content getting baked into a buffer slot
        // and never fully overwritten again since later frames only patch
        // whatever's *actually* still changing. `render_output`'s own
        // per-element diffing (`elements_gone`/moved-element damage, plus
        // each element's own `damage_since`) should in principle already
        // produce correct total damage for a completely different element
        // list - this is a defensive belt-and-braces reset, not a
        // fallback for a specific proven bug in that diffing, matched to
        // the one other place in this codebase that already resets `ages`
        // for the same underlying reason ("what's in this buffer might not
        // be what the tracker's own history thinks it is").
        let current_workspace = self.wm.borrow().current_workspace();
        if udev.last_rendered_workspace != Some(current_workspace) {
            udev.last_rendered_workspace = Some(current_workspace);
            for head in &mut udev.heads {
                head.ages = [0, 0];
            }
        }
        // See `layout_signature`'s own doc comment above for what this
        // catches that the workspace-switch reset above doesn't: any move,
        // resize, open, close, or restack within the *same* workspace.
        if !locked && udev.last_rendered_layout != Some(layout_signature) {
            udev.last_rendered_layout = Some(layout_signature);
            for head in &mut udev.heads {
                head.ages = [0, 0];
            }
        }
        // See `UdevState::last_cursor_head`'s own doc comment: neither reset
        // above notices the pointer crossing from one monitor to another,
        // so that head's own vacated cursor-sized region was left entirely
        // to `OutputDamageTracker`'s own diffing - reported live as an
        // intermittent cursor "ghost" briefly left behind on the monitor
        // just departed. Only the head being *left* needs the forced
        // repaint; the one being entered draws a genuinely new element
        // there this frame regardless, which diffs correctly on its own.
        let current_cursor_head = udev.heads.iter().position(|h| {
            let local = (udev.pointer_pos.x as i32 - h.location.x, udev.pointer_pos.y as i32 - h.location.y);
            local.0 >= 0 && local.1 >= 0 && local.0 < h.size.0 && local.1 < h.size.1
        });
        if !locked && udev.last_cursor_head != current_cursor_head {
            if let Some(old) = udev.last_cursor_head.and_then(|i| udev.heads.get_mut(i)) {
                old.ages = [0, 0];
            }
            udev.last_cursor_head = current_cursor_head;
        }
        // A head whose page-flip event never arrives (kernel-dropped, or a
        // DRM event this driver never sends for reasons this backend has no
        // visibility into) would otherwise sit in `flip_pending` forever:
        // `session.rs`'s DRM-fd handler is the only other place that clears
        // it, and it can only do that in response to an event that actually
        // shows up. A head stuck this way is excluded from `ready` below on
        // every single tick from then on - silently frozen on whatever it
        // last displayed, with no error logged anywhere (the flip that set
        // `flip_pending` had already succeeded when it was issued), which
        // is exactly what a real second monitor did live: it rendered
        // nothing but its own initial clear colour for the rest of the
        // session, from moments after being connected. `FLIP_TIMEOUT` is
        // far above any real vblank interval (even 30Hz is ~33ms) but short
        // enough that a genuine loss is invisible in practice; forcing
        // `flip_pending` back to `false` here just lets the normal path
        // below retry - if a flip is still genuinely in flight, the
        // kernel's own EBUSY on the next `page_flip` call surfaces as the
        // existing "udev: page flip failed" log line instead of a silent
        // freeze.
        const FLIP_TIMEOUT: Duration = Duration::from_millis(200);
        for head in udev.heads.iter_mut() {
            if head.flip_pending && head.flip_pending_since.elapsed() > FLIP_TIMEOUT {
                log::warn!(
                    "udev: no page-flip event for output {} after {:?}; forcing recovery",
                    head.output.name(),
                    head.flip_pending_since.elapsed()
                );
                head.flip_pending = false;
            }
        }
        let now = Instant::now();
        let ready: Vec<(usize, Output)> = udev
            .heads
            .iter()
            .enumerate()
            .filter(|(_, h)| !h.flip_pending && h.flip_retry_after.is_none_or(|t| now >= t))
            .map(|(i, h)| (i, h.output.clone()))
            .collect();
        // Kept separately from `presented` below: layer-shell surfaces
        // (bars, docks) get their frame callback every pass regardless of
        // `has_damage`, unlike toplevel windows - see the callback loop at
        // the end of this function for why the two can't share one gate.
        let ready_outputs: Vec<Output> = ready.iter().map(|(_, o)| o.clone()).collect();

        // Damage rects travel alongside each presented output so the
        // frame-callback loop below (after `udev` is no longer borrowed)
        // can notify only the windows that damage actually overlapped --
        // see `windows_touched_by_damage`'s doc comment in elements.rs.
        #[allow(clippy::type_complexity)]
        let mut presented: Vec<(Output, Point<i32, Logical>, Vec<Rectangle<i32, Physical>>)> = Vec::new();
        for (index, output) in ready {
            let lock_surface = self.lock_surface_for(&output).cloned();
            // Extracted before the `self.udev` borrow below starts - see
            // `native_lock::native_lock_render_elements`'s own doc comment
            // for why (cheap `MemoryRenderBuffer` clones, not a pixel copy).
            let native_bg = self.native_lock_background(&output.name()).cloned();
            let native_ui = self.native_lock_ui().map(|(buf, size)| (buf.clone(), size));
            let native_needs_capture = self.native_lock_needs_capture(&output.name());
            let native_header = self.native_lock_header().map(|(buf, size)| (buf.clone(), size));
            let native_shadow = self.native_lock_shadow().cloned();
            let native_keyboard = self.native_lock_keyboard().map(|(buf, size)| (buf.clone(), size));
            let native_shake_offset = self.native_lock_shake_offset();

            // Content/decoration elements are built per head: both need the
            // renderer, and geometry is translated into head-local space.
            let origin = self.udev.as_ref().map(|u| u.heads[index].location).unwrap_or_default();

            let Some(udev) = self.udev.as_mut() else { return };
            let head_crtc = udev.heads[index].crtc;
            // Phase 2 of the GPU-rendering plan (`gpu.rs`'s own module doc
            // comment), extended to every head `initialize_output`
            // succeeded for (`GpuContext::outputs`' own doc comment) and
            // now, past clear-color-only, to the real cursor too - see
            // this block's own `elements` for the still-missing pieces
            // (window content, decorations). Any head `output_for_mut`
            // finds nothing for - either `SRDWM_GPU` was never set, or
            // `initialize_output` failed for this specific crtc - falls
            // straight through to the existing, untouched Pixman path
            // unchanged.
            if let Some(gpu) = udev.gpu.as_mut() {
                // Direct field access (`gpu.outputs`), not `GpuContext::
                // output_for_mut` - that method takes `&mut self`, which
                // the borrow checker treats as borrowing all of `gpu`,
                // including `gpu.renderer` needed a few lines below. Rust's
                // disjoint-field-borrow analysis only sees through *direct*
                // field access, not a method call, even one that (like
                // this one) only actually touches `self.outputs`.
                if let Some(gpu_output) = gpu.outputs.iter_mut().find(|(c, _)| *c == head_crtc).map(|(_, o)| o) {
                    // Cursor first (topmost - `render_frame` draws
                    // earliest-pushed on top, same convention the Pixman
                    // path's own `custom_elements` uses), content after.
                    // `ids` is already front-to-back (topmost window
                    // first), and pushing in that same order is what
                    // makes plain painter's-algorithm draw order occlude
                    // correctly between windows, same as the Pixman
                    // path's own `custom_elements` relies on (see its own
                    // comment on `occluders` above). Plain
                    // `surface_content_elements` - unrounded, no
                    // decorations - not the masked/rounded path the
                    // Pixman branch uses (that's built against
                    // `PixmanRenderer` specifically) or the GLES shader
                    // `winit/render.rs` has (real, separate scope to wire
                    // in here too). A GPU-driven head shows real window
                    // content now, square corners and no border/titlebar,
                    // rather than none at all.
                    let mut elements: Vec<crate::elements::OverlayElement<smithay::backend::renderer::gles::GlesRenderer>> =
                        crate::cursor::render_elements(&cursor_status, &cursor_buffers, &mut gpu.renderer, udev.pointer_pos, origin, udev.heads[index].size);
                    for &id in &ids {
                        let Some(w) = self.wm.borrow().window(id).cloned() else { continue };
                        let Some(dwindow) = self.id_to_window.get(&id) else { continue };
                        let Some(surface) = crate::elements::window_wl_surface(dwindow) else { continue };
                        let geom = self.window_anims.get(&id).map(crate::state::WindowAnim::current_rect).unwrap_or(w.geometry);
                        let band = if w.decorated { srdwm_core::TITLEBAR_HEIGHT as i32 } else { 0 };
                        let raw_offset = dwindow.geometry().loc;
                        let content_offset = Point::<i32, Logical>::from((raw_offset.x.max(0), raw_offset.y.max(0)));
                        let pos = (geom.x - origin.x - content_offset.x, geom.y + band - origin.y - content_offset.y);
                        elements.extend(crate::elements::surface_content_elements(&mut gpu.renderer, &surface, pos, w.opacity));
                        // Border strips (top/bottom) and the titlebar bitmap,
                        // reusing the exact same cached `MemoryRenderBuffer`s
                        // the Pixman path already builds in `redraw_
                        // decoration_buffer` (renderer-agnostic: they're
                        // plain rasterized pixel buffers, imported here for
                        // `GlesRenderer` the same generic way `cursor::
                        // render_elements` already imports the cursor
                        // bitmap for either renderer). Deliberately simpler
                        // than the Pixman path in two ways, both documented
                        // gaps rather than oversights: no occlusion-fragment
                        // clipping (each window's own border/titlebar draws
                        // in full, front-to-back painter's-order the same
                        // as content above - correct when windows don't
                        // overlap, imprecise when they do), and no left/
                        // right side strips or shadow yet. `border_curve_
                        // is_safe` is unconditionally `w.decorated` here,
                        // not the Pixman path's content-masking-aware
                        // check, since this path has no content-masking
                        // concept at all yet (`w.decorated || content_will_
                        // be_masked` degenerates to just `w.decorated` when
                        // masking can never succeed).
                        let border_curve_is_safe = w.decorated;
                        // No border on a maximized window. A maximized window's own
                        // edges are the screen's edges, so a border has nothing to
                        // separate it from - and where maximize *does* stop short (the
                        // strip a top bar reserves) the border lands in that gap, drawn
                        // as a hard line right against the bar. Reported live with a
                        // screenshot: a 4px accent line between the bar and the window,
                        // and none anywhere else, because the left/right/bottom strips
                        // fall off-screen. Fullscreen is already borderless for the
                        // same reason, via `decorated` being cleared.
                        if w.border_width > 0 && !w.maximized {
                            let strips = decoration::border_strips(geom, w.border_width);
                            if let Some(buffer) = self.border_top_decorations.get(&id) {
                                let (row0, rows, shift) = decoration::border_top_visible_rows(border_curve_is_safe, w.border_width, w.corner_radius);
                                let pos = ((strips[0].x - origin.x) as f64, (strips[0].y - origin.y + shift as i32) as f64);
                                let crop_w = self.decoration_signatures.get(&id).map(|s| s.width + 2 * s.border_width).unwrap_or(strips[0].width).min(strips[0].width);
                                if strips[0].width > 0 && strips[0].height > 0 {
                                    let src = Some(Rectangle::new(Point::from((0.0, row0 as f64)), Size::from((crop_w as f64, rows as f64))));
                                    match MemoryRenderBufferRenderElement::from_buffer(&mut gpu.renderer, pos, buffer, None, src, None, Kind::Unspecified) {
                                        Ok(elem) => elements.push(crate::elements::OverlayElement::Memory(elem)),
                                        Err(e) => log::warn!("udev: SRDWM_GPU=1 failed to import top border buffer: {e}"),
                                    }
                                }
                            }
                            if let Some(buffer) = self.border_bottom_decorations.get(&id) {
                                let (row0, rows, shift) = decoration::border_bottom_visible_rows(border_curve_is_safe, w.border_width, w.corner_radius);
                                let pos = ((strips[1].x - origin.x) as f64, (strips[1].y - origin.y - shift as i32) as f64);
                                let crop_w = self.decoration_signatures.get(&id).map(|s| s.width + 2 * s.border_width).unwrap_or(strips[1].width).min(strips[1].width);
                                if strips[1].width > 0 && strips[1].height > 0 {
                                    let src = Some(Rectangle::new(Point::from((0.0, row0 as f64)), Size::from((crop_w as f64, rows as f64))));
                                    match MemoryRenderBufferRenderElement::from_buffer(&mut gpu.renderer, pos, buffer, None, src, None, Kind::Unspecified) {
                                        Ok(elem) => elements.push(crate::elements::OverlayElement::Memory(elem)),
                                        Err(e) => log::warn!("udev: SRDWM_GPU=1 failed to import bottom border buffer: {e}"),
                                    }
                                }
                            }
                        }
                        if let Some(deco) = self.decorations.get(&id) {
                            let titlebar_w = self.decoration_signatures.get(&id).map(|s| s.width).unwrap_or(geom.width).min(geom.width);
                            let pos = ((geom.x - origin.x) as f64, (geom.y - origin.y) as f64);
                            let src = Some(Rectangle::new(Point::from((0.0, 0.0)), Size::from((titlebar_w as f64, srdwm_core::TITLEBAR_HEIGHT as f64))));
                            match MemoryRenderBufferRenderElement::from_buffer(&mut gpu.renderer, pos, deco, None, src, None, Kind::Unspecified) {
                                Ok(elem) => elements.push(crate::elements::OverlayElement::Memory(elem)),
                                Err(e) => log::warn!("udev: SRDWM_GPU=1 failed to import titlebar buffer: {e}"),
                            }
                        }
                    }
                    let clear_color = [0.05, 0.05, 0.08, 1.0];
                    match gpu_output.render_frame(&mut gpu.renderer, &elements, clear_color, smithay::backend::drm::compositor::FrameFlags::DEFAULT) {
                        Ok(res) if !res.is_empty => match gpu_output.queue_frame(None) {
                            Ok(()) => {}
                            Err(e) => log::warn!("udev: SRDWM_GPU=1 queue_frame failed for crtc {head_crtc:?}: {e:?}"),
                        },
                        Ok(_) => {}
                        Err(e) => log::warn!("udev: SRDWM_GPU=1 render_frame failed for crtc {head_crtc:?}: {e:?}"),
                    }
                    continue;
                }
            }

            let head = &mut udev.heads[index];
            let back = 1 - head.front;

            let mut custom_elements: Vec<crate::elements::OverlayElement<PixmanRenderer>> = Vec::new();
            if !locked {
                // Cursor first: `render_output` draws custom elements
                // front-to-back, so the earliest element is topmost. On a
                // bare TTY nothing else draws a pointer - see `cursor.rs`.
                let pointer_pos = udev.pointer_pos;
                let hsize = udev.heads[index].size;
                custom_elements.extend(crate::cursor::render_elements(
                    &cursor_status,
                    &cursor_buffers,
                    &mut udev.renderer,
                    pointer_pos,
                    origin,
                    hsize,
                ));
                // Multi-cursor mode, Phase 1: one extra sprite per *other*
                // physical pointer device's own last-known position (see
                // `UdevState::secondary_cursors`'s own doc comment) - the
                // device that drove `pointer_pos` itself is skipped so its
                // cursor isn't drawn twice at the same spot. All secondary
                // sprites share the one real cursor image/theme
                // (`cursor_status`/`cursor_buffers`) rather than each
                // device getting its own - a real visual distinction
                // between devices is a later-phase refinement, not needed
                // to prove multiple live positions render at all. Gated
                // on `general.multi_cursor` (off by default) and on each
                // entry's own recency: a device that reported a position
                // once and then never moved again - the real, reported
                // live bug - stops rendering after `SECONDARY_CURSOR_
                // TIMEOUT` instead of sitting frozen on screen forever.
                if self.wm.borrow().multi_cursor_enabled {
                    let now = std::time::Instant::now();
                    let active_device = udev
                        .secondary_cursors
                        .iter()
                        .find(|&(_, &(p, _))| p == pointer_pos)
                        .map(|(d, _)| d.clone());
                    for (device, &(pos, seen)) in &udev.secondary_cursors {
                        if Some(device) == active_device.as_ref() {
                            continue;
                        }
                        if now.duration_since(seen) >= super::SECONDARY_CURSOR_TIMEOUT {
                            continue;
                        }
                        custom_elements.extend(crate::cursor::render_elements(&cursor_status, &cursor_buffers, &mut udev.renderer, pos, origin, hsize));
                    }
                }
                // Night light/reading mode - a translucent full-output
                // overlay, pushed right after the cursor so it colours
                // everything else (windows, bars, menus) but never the
                // pointer itself. See `color_filter::render_element` for
                // why an overlay rather than a true per-pixel shader.
                let color_filter = self.wm.borrow().color_filter;
                let buf = self.color_filter_buffers.entry(output.name()).or_default();
                if let Some(elem) = crate::color_filter::render_element(buf, color_filter, hsize) {
                    custom_elements.push(crate::elements::OverlayElement::Solid(elem));
                }
                // The right-click titlebar menu, if open - pushed right
                // after the cursor so it's still topmost over every window
                // but never hides the pointer itself (you need to see what
                // you're about to click).
                if let (Some(menu), Some(buffer)) = (self.context_menu.as_ref(), self.context_menu_buffer.as_ref()) {
                    let pos = ((menu.pos.0 - origin.x) as f64, (menu.pos.1 - origin.y) as f64);
                    match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, pos, buffer, None, None, None, Kind::Unspecified) {
                        Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                        Err(e) => log::warn!("udev: failed to import context menu buffer: {e}"),
                    }
                }
                // The drag snap preview - below the flyout (which the
                // pointer is actively aiming at) but above every window,
                // since it is showing where one of them is about to go.
                if let Some(rect) = drag_snap_preview {
                    custom_elements.extend(
                        crate::elements::snap_preview_elements(&mut self.snap_preview_buffers, rect, accent_color, (origin.x, origin.y))
                            .into_iter()
                            .map(crate::elements::OverlayElement::Solid),
                    );
                }
                // The Snap-Layouts flyout, if open - same "topmost but
                // never hides the cursor" placement as the context menu.
                if let (Some(flyout), Some(buffer)) = (self.snap_flyout.as_ref(), self.snap_flyout_buffer.as_ref()) {
                    let pos = ((flyout.pos.0 - origin.x) as f64, (flyout.pos.1 - origin.y) as f64);
                    match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, pos, buffer, None, None, None, Kind::Unspecified) {
                        Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                        Err(e) => log::warn!("udev: failed to import snap flyout buffer: {e}"),
                    }
                }
                // The desktop-icon/bare-desktop right-click menu, if open --
                // same "topmost but never hides the cursor" placement.
                if let (Some(menu), Some(buffer)) = (self.desktop_menu.as_ref(), self.desktop_menu_buffer.as_ref()) {
                    let pos = ((menu.pos.0 - origin.x) as f64, (menu.pos.1 - origin.y) as f64);
                    match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, pos, buffer, None, None, None, Kind::Unspecified) {
                        Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                        Err(e) => log::warn!("udev: failed to import desktop menu buffer: {e}"),
                    }
                }
                // Popups next: always above every window's own content,
                // matching this codebase's long-standing behavior from
                // before content moved into this same `custom_elements`
                // list (see below) - pushing them here, ahead of every
                // window and every layer-shell surface, is what keeps that
                // true now that "above everything in `self.space`" is no
                // longer a free property of a separate tier.
                custom_elements.extend(crate::elements::popup_render_elements(&popup_targets, &mut udev.renderer, (origin.x, origin.y)));

                // The bar/dock/launcher (`Layer::Top`/`Overlay`): rendered
                // ourselves via `output_layer_elements`, not through
                // `render_output`'s automatic inclusion of `self.space` +
                // `layer_map_for_output` - see this function's own call
                // site further down for why content had to stop flowing
                // through that convenience wrapper at all (per-window
                // opacity), which took layer-shell inclusion down with it as
                // a side effect. Skipped entirely - not just covered - for
                // a fullscreen window: `we should not see the bar at all`,
                // and unmapping it (`gtk_shell`) or covering it are two
                // different guarantees. `ids` is already front-to-back, so
                // checking every id for `fullscreen` here (rather than just
                // the frontmost) covers a fullscreen window stacked behind
                // an always-on-top one too.
                let hide_top_layers = ids.iter().any(|&id| self.wm.borrow().window(id).is_some_and(|w| w.fullscreen));
                if !hide_top_layers {
                    custom_elements.extend(crate::elements::output_layer_elements(
                        &mut udev.renderer,
                        &output,
                        |layer| matches!(layer, Layer::Top | Layer::Overlay),
                    ));
                }

                // Windows stacked in front of whichever one border/
                // decoration is being built right now - `ids` is already
                // front-to-back, so this only ever needs appending to, not
                // recomputing. A window's own *content*, pushed inside this
                // same loop below, needs no separate occlusion test: it
                // draws in the same front-to-back push order as everything
                // else here, so ordinary painter's-algorithm draw order
                // already occludes it correctly (this is exactly why content
                // used to occlude correctly via `self.space`'s own order,
                // before it had to move into this list for per-window
                // opacity to be possible at all). The border strips and
                // titlebar bitmap are different: outside `geometry`, drawn
                // via a bitmap that isn't itself window-shaped, so they
                // still need `occluders`' explicit clip against whichever
                // window is stacked in front.
                let mut occluders: Vec<srdwm_core::Rect> = Vec::with_capacity(ids.len());
                for &id in &ids {
                    let Some(w) = self.wm.borrow().window(id).cloned() else { continue };
                    // `w.geometry` is the animation's *target*, not
                    // necessarily where the window is actually drawn this
                    // frame - during a maximize/fullscreen/open-slide tween,
                    // `sync_geometry` already renders the window's own
                    // content at `window_anims`' interpolated rect (see its
                    // doc comment), but this loop used to read `w.geometry`
                    // straight from the model regardless, so the border and
                    // titlebar sat at the final rect while the content they
                    // were supposed to outline slid past underneath them --
                    // reported live as the border "not flush" with the
                    // window during any animated transition. Every use of
                    // this window's geometry below (titlebar/border
                    // placement *and* the occlusion test against later
                    // windows) has to agree with what `sync_geometry` mapped
                    // the content to, or they drift apart again.
                    let geom = self.window_anims.get(&id).map(crate::state::WindowAnim::current_rect).unwrap_or(w.geometry);
                    // `geom` above is this compositor's own request/target;
                    // `frame` corrects its far edge to match what the
                    // client's surface really committed (a terminal's
                    // cell-quantized size, most commonly) - see
                    // `effective_frame`'s own doc comment. Everything below
                    // that has to visually hug the real edge (titlebar/
                    // border placement, the shadow, the occlusion test
                    // against windows behind this one) reads `frame`; only
                    // the actual content position still reads `geom`/`band`
                    // directly, since that's already correctly anchored via
                    // `content_offset` below regardless of this correction.
                    let frame = crate::state::CompState::effective_frame_of(&self.wm, &self.id_to_window, &self.pending_size_configure, id, geom);
                    // Computed here, ahead of the border strips below,
                    // purely so they can know it - the actual content
                    // element that reads this same masked buffer is still
                    // pushed later, in its own usual place in the loop, and
                    // gets a cheap cache hit from `rounded_content_buffer`'s
                    // own `epoch`/`radius_bits` check rather than doing the
                    // masking work twice. `w.decorated` alone used to gate
                    // whether the border strips' own "extra" rows (see
                    // `decoration::border_top_visible_rows`'s doc comment)
                    // were safe to draw past their nominal `border_width` --
                    // correct for a decorated window (a titlebar band
                    // absorbs them) but not for an undecorated one, which
                    // relies on content-masking instead, and several real
                    // clients (Firefox, confirmed live) never actually get
                    // masked at all (`masked_content_buffer`'s own
                    // subsurface early-out). Cropping unconditionally
                    // whenever undecorated (the fix's first version) closed
                    // the wedge bug but cost every undecorated window its
                    // own visible corner curve even when masking *did*
                    // succeed, which is unnecessary - this makes that
                    // decision follow the real per-window, per-frame
                    // outcome instead of just the static `decorated` flag.
                    // `masked.is_some()` alone used to be the whole check,
                    // back when masking meant identifying and reading one
                    // specific client subsurface directly - wrong the
                    // moment the resolved child excluded more of the root
                    // than the client's own declared shadow margin (a GTK4
                    // client legitimately reserves an invisible margin for
                    // its own drop shadow, but Firefox's tab strip/title row
                    // is painted on the *root* surface outside its content
                    // child, and once that surface-picking heuristic got
                    // permissive enough to mask Firefox too, it silently
                    // deleted Firefox's real tab strip - reported live as
                    // "Firefox's titlebar turned invisible", confirmed by
                    // toggling `general.rounded_corners` off, which brought
                    // it straight back). `rounded_corners_pixman::masked_
                    // content_buffer` no longer has that failure mode at
                    // all: it renders the window's *whole* surface tree into
                    // its own off-screen buffer and masks the composited
                    // result, the same thing a GPU shader-based compositor
                    // does by construction - so `.is_some()` is genuinely
                    // the whole answer again. `loc`/`content_size` mirror
                    // the real content push's own `content_offset`/`band`
                    // correction below (`pos`'s own doc comment) - both
                    // call sites have to agree on the origin/size a mask was
                    // built at, or `rounded_content_buffer`'s cache would
                    // never consider one stale after a resize.
                    let content_will_be_masked = if rounded_corners_enabled && self.wm.borrow().resizing_window() != Some(id) {
                        // Clamped to non-negative - see the matching clamp
                        // on the real content push's own `content_offset`
                        // further down for why: a real CSD shadow margin is
                        // never negative, but a live Firefox window was
                        // observed reporting `loc = (-10, -10)` despite the
                        // tiled-state hint telling it to reserve no margin
                        // at all (`TEMP-DIAG5`, confirmed live). Treating
                        // that raw negative value as a real offset shifts
                        // this mask's own origin the *wrong* way - away
                        // from alignment with the border, not toward it.
                        let raw_offset = self.id_to_window.get(&id).map(|dw| dw.geometry().loc).unwrap_or_default();
                        let content_offset = Point::<i32, Logical>::from((raw_offset.x.max(0), raw_offset.y.max(0)));
                        let band = if w.decorated { srdwm_core::TITLEBAR_HEIGHT as i32 } else { 0 };
                        let content_size = (frame.width as i32, (frame.height as i32 - band).max(0));
                        let loc = (-content_offset.x, -content_offset.y);
                        self.id_to_window
                            .get(&id)
                            .and_then(crate::elements::window_wl_surface)
                            .map(|surface| {
                                let epoch = self.content_epoch.get(&id).copied().unwrap_or(0);
                                let corners = if w.decorated { crate::rounded_corners::RoundedCorners::BOTTOM_ONLY } else { crate::rounded_corners::RoundedCorners::ALL };
                                crate::elements::rounded_content_buffer(&mut self.rounded_content_buffers, &mut udev.renderer, epoch, id, &surface, loc, content_size, w.corner_radius as f32, corners).is_some()
                            })
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    let border_curve_is_safe = w.decorated || content_will_be_masked;
                    // Pushed *before* the titlebar band below, deliberately --
                    // unlike the bottom/side strips further down, this one
                    // isn't confined to `geometry`'s own outside: whenever
                    // `corner_radius > border_width` (the common case: 12 vs
                    // 4 by default), `border_top_visible_rows` deliberately
                    // extends this buffer `corner_radius - border_width` rows
                    // *past* its nominal thickness, straight down into the
                    // titlebar band's own top rows, so the one shared curve
                    // has room to finish (see that function's and `render_
                    // border_top`'s own doc comments). For that overlap to
                    // read as one continuous curve rather than the titlebar's
                    // own, differently-centred corner mask poking a square
                    // notch through it, this element's border-coloured
                    // corner columns have to actually paint over the
                    // titlebar's own attempt at those same pixels - which
                    // only happens if this pushes first. Reported live,
                    // confirmed via a zoomed screenshot: pushed after the
                    // titlebar (the previous order), the titlebar's own
                    // smaller, square-under-the-curve corner rendered on top
                    // instead, since `custom_elements` composites earlier-
                    // pushed entries over later ones - exactly backwards
                    // from what this overlap needs.
                    if w.border_width > 0 {
                        let strips = decoration::border_strips(frame, w.border_width);
                        // Strip 0 (top) rounded on its own two corners - see
                        // `render_border_top`'s own doc comment - so it's a
                        // cached bitmap (rebuilt only in `redraw_decoration_
                        // buffer`, same as the titlebar itself), not
                        // rasterized fresh here every frame. Not fragment-
                        // clipped like the left/right strips further down --
                        // cropping a bitmap's source rect per fragment is
                        // real extra work for a strip that's only `border_
                        // width` pixels tall to begin with, so this only
                        // handles the all-or-nothing case: skip entirely
                        // once *fully* covered, accept a small residual
                        // bleed while only partially covered.
                        if strips[0].width > 0 && strips[0].height > 0 && !strips[0].subtract_all(&occluders).is_empty() {
                            if let Some(buffer) = self.border_top_decorations.get(&id) {
                                // See `decoration::border_top_visible_rows`'s
                                // own doc comment: an undecorated window's
                                // top strip crops away this buffer's
                                // titlebar-band-only "extra" rows, which
                                // otherwise paint a border-coloured wedge
                                // straight onto its real content - reported
                                // live on a real Firefox window, confirmed
                                // via a screenshot to be neither Firefox's
                                // own rendering nor the separate content-
                                // mask feature.
                                let (row0, rows, shift) = decoration::border_top_visible_rows(border_curve_is_safe, w.border_width, w.corner_radius);
                                let pos = ((strips[0].x - origin.x) as f64, (strips[0].y - origin.y + shift as i32) as f64);
                                // Clamped to the buffer's own actual last-
                                // built width, not trusted at face value --
                                // see `effective_frame_of`'s own doc comment
                                // on why: during an active resize `frame`
                                // (and so `strips[0]`) tracks the *live*
                                // drag target, which can outrun however far
                                // `redraw_decoration_buffer` has actually
                                // gotten rebuilding this same buffer to
                                // match. An uncrapped `src` wider than the
                                // real texture is an out-of-bounds sample,
                                // not a clean error - this is what actually
                                // prevents that, not just the rebuild-on-
                                // every-tick call in `input::pointer` trying
                                // to keep the gap small.
                                let crop_w = self.decoration_signatures.get(&id).map(|s| s.width + 2 * s.border_width).unwrap_or(strips[0].width).min(strips[0].width);
                                let src = Some(Rectangle::new(Point::from((0.0, row0 as f64)), Size::from((crop_w as f64, rows as f64))));
                                match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, pos, buffer, None, src, None, Kind::Unspecified) {
                                    Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                                    Err(e) => log::warn!("udev: failed to import top border buffer: {e}"),
                                }
                            }
                        }
                    }
                    if let Some(deco) = self.decorations.get(&id) {
                        // Fragment-clipped, same as the three solid border
                        // strips below - an *all-or-nothing* version of
                        // this (skip only once fully covered) was tried
                        // first and reported live as still showing "the
                        // behind window's bar": a titlebar only *partially*
                        // covered - the common case for cascaded/
                        // overlapping windows - drew in full regardless,
                        // bleeding through the covered part. `from_buffer`'s
                        // `src` parameter crops the bitmap itself, so each
                        // visible fragment can come from the matching
                        // sub-rect of the source image rather than the
                        // whole thing.
                        // Width clamped to the buffer's own actual last-
                        // built width - see `effective_frame_of`'s own doc
                        // comment and the matching clamp on the top border
                        // strip just above for why: during an active resize
                        // `frame.width` tracks the *live* drag target, which
                        // can outrun however far `redraw_decoration_buffer`
                        // has actually gotten. Clamped here, before
                        // `visible_border_fragments` runs, so every fragment
                        // it produces is already within the real texture --
                        // an unclamped one could ask for a crop past it,
                        // an out-of-bounds sample rather than a clean error.
                        let titlebar_w = self.decoration_signatures.get(&id).map(|s| s.width).unwrap_or(frame.width).min(frame.width);
                        let titlebar_rect = srdwm_core::Rect::new(frame.x, frame.y, titlebar_w, srdwm_core::TITLEBAR_HEIGHT);
                        for fragment in crate::elements::visible_border_fragments(titlebar_rect, &occluders) {
                            let pos = ((fragment.x - origin.x) as f64, (fragment.y - origin.y) as f64);
                            let src = Rectangle::new(
                                Point::from(((fragment.x - titlebar_rect.x) as f64, (fragment.y - titlebar_rect.y) as f64)),
                                Size::from((fragment.width as f64, fragment.height as f64)),
                            );
                            match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, pos, deco, None, Some(src), None, Kind::Unspecified) {
                                Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                                Err(e) => log::warn!("udev: failed to import titlebar buffer: {e}"),
                            }
                        }
                    }
                    // The bottom strip sits entirely outside this window's
                    // own `geometry` with no titlebar-style overlap into
                    // content the way the top strip's own "extra" rows do
                    // above, so push order against the titlebar doesn't
                    // matter for it - only against other windows', which
                    // iterating `ids` in stacking order already gets right
                    // *for windows also drawn via this same custom_elements
                    // loop* - but not against any window's own *content*,
                    // which is why `occluders` below is still needed even
                    // with that ordering.
                    //
                    // The left/right side strips are a different story --
                    // see their own push site further down for why they
                    // (unlike the bottom strip) *do* need cropping against
                    // this same top/bottom-strip overlap, a real bug this
                    // comment used to claim didn't exist here at all.
                    if w.border_width > 0 {
                        let color = crate::state::effective_border_color(w.border_color, focused == Some(id), self.wm.borrow().theme.border_inactive_dim);
                        let strips = decoration::border_strips(frame, w.border_width);
                        // Strip 1 (bottom), the top strip's own mirror --
                        // see `decoration::render_border_bottom`'s doc
                        // comment. Same all-or-nothing bitmap treatment.
                        if strips[1].width > 0 && strips[1].height > 0 && !strips[1].subtract_all(&occluders).is_empty() {
                            if let Some(buffer) = self.border_bottom_decorations.get(&id) {
                                // See `decoration::border_bottom_visible_
                                // rows`'s own doc comment: relies on
                                // `BOTTOM_ONLY` content-masking having made
                                // this corner of a decorated window's
                                // content transparent already, which several
                                // real undecorated clients (Firefox,
                                // confirmed live) never actually get - same
                                // wedge bug as the top strip, confirmed on
                                // the same window's bottom-left corner via a
                                // real screenshot, not assumed.
                                let (row0, rows, shift) = decoration::border_bottom_visible_rows(border_curve_is_safe, w.border_width, w.corner_radius);
                                let pos = ((strips[1].x - origin.x) as f64, (strips[1].y - origin.y - shift as i32) as f64);
                                // Same live-resize safety clamp as the top
                                // strip above: bound against the buffer's
                                // own actual last-built width, not the
                                // possibly-ahead-of-it live `strips[1].width`.
                                let crop_w = self.decoration_signatures.get(&id).map(|s| s.width + 2 * s.border_width).unwrap_or(strips[1].width).min(strips[1].width);
                                let src = Some(Rectangle::new(Point::from((0.0, row0 as f64)), Size::from((crop_w as f64, rows as f64))));
                                match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, pos, buffer, None, src, None, Kind::Unspecified) {
                                    Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                                    Err(e) => log::warn!("udev: failed to import bottom border buffer: {e}"),
                                }
                            }
                        }
                        // The remaining two strips (left/right) are
                        // persistent `SolidColorBuffer`s updated in place,
                        // not rebuilt with a fresh `Id` every frame - see
                        // `elements::border_side_render_element`'s doc
                        // comment for why that distinction is load-bearing
                        // for damage tracking, not cosmetic. Each strip is
                        // further split into whatever fragments remain
                        // visible after subtracting `occluders`, since a
                        // whole unclipped strip is exactly the bug fixed
                        // here.
                        //
                        // Cropped top and bottom by `extra` - the same
                        // `corner_radius - border_width` gap `border_top_
                        // visible_rows`/`border_bottom_visible_rows` extend
                        // the top/bottom strips *into* whenever the radius
                        // exceeds the border's own nominal thickness (the
                        // common case: 12+ vs 4 at this theme's defaults).
                        // These side strips are plain flat fills with no
                        // curve awareness of their own (see this file's own
                        // stale comment just below, corrected here: "sit
                        // entirely outside... no titlebar-style overlap"
                        // was wrong - they *do* overlap the top/bottom
                        // strip's own extended, curved region), and used to
                        // span the window's full nominal height
                        // unconditionally. Since the top/bottom strip is
                        // pushed *before* these (earlier = topmost, see
                        // this loop's own ordering), its own curve's
                        // transparent cutout should be what shows through
                        // there - but a flat, uncropped side strip sitting
                        // directly underneath filled that same "supposed to
                        // be cut away" region with solid colour instead,
                        // which the curve's transparency does nothing to
                        // hide, since the side strip isn't part of what the
                        // curve is cutting *out of*. Reported live as a
                        // straight vertical line poking out from inside an
                        // otherwise-correctly-curved corner, confirmed via
                        // raw pixel sampling: solid border colour at a
                        // fixed x, starting right at the window's nominal
                        // top edge, running in parallel with the real
                        // curve rather than being replaced by it.
                        let extra = if border_curve_is_safe { w.border_width.max(w.corner_radius).saturating_sub(w.border_width) } else { 0 };
                        let mut side_strips = [strips[2], strips[3]];
                        for s in &mut side_strips {
                            s.y += extra as i32;
                            s.height = s.height.saturating_sub(2 * extra);
                        }
                        let pool = self.border_side_buffers.entry(id).or_default();
                        let mut buf_index = 0;
                        for strip in &side_strips {
                            if strip.width == 0 || strip.height == 0 {
                                continue;
                            }
                            for fragment in crate::elements::visible_border_fragments(*strip, &occluders) {
                                let buf = crate::elements::border_fragment_buffer(pool, buf_index);
                                buf_index += 1;
                                custom_elements.push(crate::elements::OverlayElement::Solid(crate::elements::border_side_render_element(buf, fragment, color, (origin.x, origin.y))));
                            }
                        }
                    }
                    // Shadow, positioned from the same animated `geom` as
                    // everything else here - not `w.geometry` - for the
                    // same reason a stale-position border read as detached
                    // from a mid-tween window before that fix: see `geom`'s
                    // own doc comment above. Pushed *after* the titlebar/
                    // border above, not before - `custom_elements` treats
                    // earlier-pushed as topmost (see `border_side_render_
                    // element`'s doc comment), and a shadow pushed first
                    // rendered on top of this same window's own border
                    // strips, alpha-blending black over them and muting the
                    // configured border colour into a hazy, indistinct
                    // smear instead of a crisp line. Reported live as
                    // "spacing before the border" - confirmed by sampling
                    // pixels straight across a window's edge: no run of the
                    // configured border colour appeared anywhere, just a
                    // gradient straight from content black into the
                    // shadow's own falloff. `shadow_bitmap`'s own doc
                    // comment already assumed "the window's own border/
                    // titlebar/content always draws over it" - true for
                    // content (spatially disjoint from the shadow's
                    // rendered rect either way) but not for the border,
                    // which sits inside the shadow's footprint and needs
                    // the *later* push, not the earlier one, to actually
                    // end up on top of it.
                    //
                    // Fragment-clipped against `occluders` now, same as the
                    // titlebar/border above - this used to skip that on
                    // the reasoning that `SHADOW_MAX_ALPHA`'s low opacity
                    // would read as a soft edge, not the hard-line bleed-
                    // through that made the titlebar/border need it. True
                    // along a shadow's straight edges, false at its
                    // corners: `shadow_bitmap` falls off by Chebyshev
                    // (square-ring) distance, not radial, so each corner is
                    // a hard-edged square block at up to ~35% opacity, not
                    // a soft vignette - reported live as a small dark
                    // rectangular patch sitting on top of whatever window a
                    // floating/cascaded window's own corner happened to
                    // overlap, most visible exactly where two windows'
                    // corners nearly meet, which this compositor's default
                    // cascade placement does constantly.
                    if let Some(shadow) = self.shadow_buffers.get(&id) {
                        // `full` is the bitmap's own extent and stays
                        // unclipped, because `src` below indexes into that
                        // bitmap; `rect` is the same box clipped to the
                        // monitors this window actually occupies, and only
                        // decides which fragments get drawn.
                        let full = decoration::shadow_rect(frame);
                        let rect = decoration::shadow_rect_clipped(frame, &monitor_bounds);
                        for fragment in crate::elements::visible_border_fragments(rect, &occluders) {
                            let pos = ((fragment.x - origin.x) as f64, (fragment.y - origin.y) as f64);
                            let src = Rectangle::new(
                                Point::from(((fragment.x - full.x) as f64, (fragment.y - full.y) as f64)),
                                Size::from((fragment.width as f64, fragment.height as f64)),
                            );
                            match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, pos, shadow, None, Some(src), None, Kind::Unspecified) {
                                Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                                Err(e) => log::warn!("udev: failed to import shadow buffer: {e}"),
                            }
                        }
                    }
                    // The window's own content, at its own `opacity` --
                    // this, not decoration, is the entire reason content
                    // moved into this loop at all (see the doc comment on
                    // the popup push above). Positioned the same way
                    // `sync_geometry` maps it into `self.space` (band added
                    // for a decorated window's titlebar reservation), so
                    // switching rendering paths doesn't also shift content
                    // relative to where clicks still land (hit-testing is
                    // untouched, still `w.geometry`/`self.space`-based).
                    if let Some(dwindow) = self.id_to_window.get(&id) {
                        if let Some(surface) = crate::elements::window_wl_surface(dwindow) {
                            let band = if w.decorated { srdwm_core::TITLEBAR_HEIGHT as i32 } else { 0 };
                            // `dwindow.geometry().loc` is the client's own
                            // `xdg_surface.set_window_geometry` offset --
                            // GTK4 CSD clients (Firefox concretely) declare
                            // their real visible content as a sub-rect
                            // inset within a larger buffer that also holds
                            // an invisible shadow-reservation margin, even
                            // once the tiled-state hint (`sync_geometry`,
                            // `xdg_toplevel::State::Tiled*`) has told them
                            // to skip drawing that shadow - the margin
                            // itself, not just its decoration, stays
                            // reserved in the buffer. Never subtracting
                            // this meant every such client's buffer origin
                            // (0,0) - the *outer* edge of that invisible
                            // margin - landed exactly at `geom.x,geom.y`,
                            // leaving the margin's width/height of genuine
                            // gap (wallpaper visible through it) between
                            // the border this compositor draws and where
                            // the client's actual visible content begins.
                            // Reported live as "an extra layer or border
                            // over each window" - confirmed by diffing a
                            // corner crop against the real wallpaper at
                            // that exact screen position, pixel for pixel.
                            //
                            // Clamped to non-negative: a real shadow margin
                            // is never negative, but a live Firefox window
                            // was observed reporting `loc = (-10, -10)`
                            // despite `sync_geometry`'s tiled-state hint
                            // telling it to reserve no margin at all
                            // (`TEMP-DIAG5`, confirmed live). Subtracting
                            // that raw negative value shifts content the
                            // *wrong* way - further from the border this
                            // window's own `frame`/`effective_frame_of`
                            // (which already clamps this same value the
                            // same way for its own size calc) draws around,
                            // not toward it - reported live as Firefox's
                            // border sitting visibly detached, up and to
                            // the left, from its own real content.
                            let raw_offset = dwindow.geometry().loc;
                            let content_offset = Point::<i32, Logical>::from((raw_offset.x.max(0), raw_offset.y.max(0)));
                            // Where the window's real, margin-excluded
                            // visible content belongs on screen - what the
                            // masked/rounded buffer below is placed at,
                            // since `rounded_content_buffer`'s own `loc`
                            // parameter (`-content_offset`, just below)
                            // already renders that buffer's surface tree
                            // shifted so the buffer's *own* `(0, 0)` lands
                            // exactly on this same real content top-left --
                            // the margin has already been compensated for
                            // once, inside the buffer itself.
                            let content_pos = (geom.x - origin.x, geom.y + band - origin.y);
                            // Only for the *fallback* path below
                            // (`surface_content_elements`, pushed when
                            // masking is off or fails): that renders the
                            // client's raw surface tree directly, with no
                            // prior margin compensation of its own, so it's
                            // the one place `content_offset` still needs
                            // subtracting here. Reusing `content_pos` for
                            // both (what an earlier version of this did)
                            // double-applied the shift for the masked
                            // buffer - its own internal `loc`-based
                            // compensation, then this `pos` subtracting the
                            // same margin a second time - landing the
                            // masked buffer `content_offset` px too far up
                            // and left of the border wrapping it. Confirmed
                            // live via pixel sampling a real Firefox window
                            // (`raw_offset = (10, 10)`): its own chrome
                            // rendered starting 10px above where the
                            // border's nominal top edge began, fully
                            // exposed, square, with no border over it at
                            // all - reported as "border isn't correctly
                            // over the window."
                            let pos = (content_pos.0 - content_offset.x, content_pos.1 - content_offset.y);
                            let mut rounded_elem = None;
                            // Skipped for whichever window is being
                            // interactively resized right now, specifically
                            // (not gated on `is_resizing()` alone, which
                            // would also blank every *other* window's own
                            // masking for the duration): `rounded_content_
                            // buffer`'s own doc comment already flagged this
                            // backend's real CPU cost - a full row-by-row
                            // copy of the surface's *entire* pixel buffer on
                            // every commit, unlike the free-on-GPU winit/
                            // GLES path - and a resize is exactly the case
                            // that pays it hardest: content reflows and
                            // recommits on every single frame of the drag,
                            // not just once. Reported live as "resizing is
                            // very laggy" the first time this session real
                            // hardware actually exercised `general.
                            // rounded_corners` turned on at all (it defaults
                            // off for exactly this reason). The corner mask
                            // is cosmetic and this is the one moment its
                            // absence is least likely to be noticed --
                            // attention is on the edge being dragged, not
                            // the opposite corner's curve - so skipping it
                            // for the resize's duration and letting it
                            // reappear the instant it ends (no cache
                            // invalidation needed either way: `epoch`
                            // already only rebuilds on a real content
                            // change) is a real fix, not a visible
                            // regression.
                            let being_resized = self.wm.borrow().resizing_window() == Some(id);
                            if rounded_corners_enabled && !being_resized {
                                let epoch = self.content_epoch.get(&id).copied().unwrap_or(0);
                                // Bottom-only for a decorated window, same
                                // reasoning as `winit/render.rs`'s identical split:
                                // the top two corners are already hidden
                                // under the titlebar band's own rounded
                                // bitmap.
                                let corners = if w.decorated { crate::rounded_corners::RoundedCorners::BOTTOM_ONLY } else { crate::rounded_corners::RoundedCorners::ALL };
                                // `content_offset`/`band` above already give
                                // this window's own content origin/size; the
                                // mask's own off-screen buffer is rendered
                                // and sized to match exactly, so (unlike the
                                // old per-subsurface-buffer approach) the
                                // result can simply be placed at plain `pos`
                                // below - see `rounded_corners_pixman`'s own
                                // module doc comment for why this no longer
                                // needs a separate offset or a safety check
                                // against `content_offset` at all: the whole
                                // surface tree is what gets masked now, not
                                // one guessed-at subsurface, so there is
                                // nothing left it could silently exclude.
                                let content_size = (frame.width as i32, (frame.height as i32 - band).max(0));
                                let loc = (-content_offset.x, -content_offset.y);
                                let masked = crate::elements::rounded_content_buffer(&mut self.rounded_content_buffers, &mut udev.renderer, epoch, id, &surface, loc, content_size, w.corner_radius as f32, corners);
                                if let Some(buffer) = masked {
                                    match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, (content_pos.0 as f64, content_pos.1 as f64), buffer, Some(w.opacity), None, None, Kind::Unspecified) {
                                        Ok(elem) => rounded_elem = Some(elem),
                                        Err(e) => log::warn!("udev: failed to import rounded content buffer: {e}"),
                                    }
                                }
                            }
                            match rounded_elem {
                                Some(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                                None => {
                                    custom_elements.extend(crate::elements::surface_content_elements(&mut udev.renderer, &surface, pos, w.opacity));
                                }
                            }
                        }
                    }
                    occluders.push(frame);
                }
                // Real desktop icons - above the wallpaper, below every
                // window: pushed after the windows loop above (so nothing
                // here can occlude a real window) but before the
                // background-layer push just below (so the wallpaper still
                // shows through everywhere an icon doesn't draw). See
                // `desktop_icons.rs`'s own module doc comment.
                // The rubber-band marquee outline, above the icons it's
                // selecting - four thin solid-colour strips (the same
                // `border_side_render_element` primitive window borders
                // already use), not a translucent fill: `SolidColorRender
                // Element` has no alpha-blend path, and a plain accent-
                // coloured outline is still a real, visible selection
                // indicator without needing a new element type for one
                // feature.
                if let Some((start, current)) = self.desktop_marquee {
                    let (x0, y0) = (start.0.min(current.0), start.1.min(current.1));
                    let (x1, y1) = (start.0.max(current.0), start.1.max(current.1));
                    let color = self.wm.borrow().theme.default_border_color;
                    const T: i32 = 1;
                    let strips = [
                        srdwm_core::Rect::new(x0, y0, (x1 - x0).max(0) as u32, T as u32),
                        srdwm_core::Rect::new(x0, y1 - T, (x1 - x0).max(0) as u32, T as u32),
                        srdwm_core::Rect::new(x0, y0, T as u32, (y1 - y0).max(0) as u32),
                        srdwm_core::Rect::new(x1 - T, y0, T as u32, (y1 - y0).max(0) as u32),
                    ];
                    for (strip, buf) in strips.into_iter().zip(self.marquee_buffers.iter_mut()) {
                        custom_elements.push(crate::elements::OverlayElement::Solid(crate::elements::border_side_render_element(
                            buf,
                            strip,
                            color,
                            (origin.x, origin.y),
                        )));
                    }
                }
                for (pos, buffer) in &desktop_icon_render_list {
                    let local_pos = ((pos.0 - origin.x) as f64, (pos.1 - origin.y) as f64);
                    match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, local_pos, buffer, None, None, None, Kind::Unspecified) {
                        Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                        Err(e) => log::warn!("udev: failed to import desktop icon buffer: {e}"),
                    }
                }
                // Background/bottom layer-shell (wallpaper engines) last --
                // bottommost, matching smithay's own `space_render_elements`
                // ordering, which this whole custom loop now replaces.
                custom_elements.extend(crate::elements::output_layer_elements(
                    &mut udev.renderer,
                    &output,
                    |layer| matches!(layer, Layer::Background | Layer::Bottom),
                ));
            }
            // Three genuinely different element types (external `LockSurface`
            // content, srdwm's own memory-backed background+UI, or the
            // normal desktop's `custom_elements`), so each is built and
            // passed to its own `render_output` call below rather than
            // forced into one shared, unified element list.
            let is_native = self.lock.native.is_some();
            let lock_elements = if locked && !is_native {
                crate::lock::lock_render_elements(lock_surface.as_ref(), &mut udev.renderer)
            } else {
                Vec::new()
            };
            let native_elements = if locked && is_native {
                let size = udev.heads[index].size;
                let frame = crate::native_lock::NativeLockFrame {
                    background: native_bg.as_ref(),
                    header: native_header.as_ref().map(|(b, s)| (b, *s)),
                    shadow: native_shadow.as_ref(),
                    ui: native_ui.as_ref().map(|(b, s)| (b, *s)),
                    keyboard: native_keyboard.as_ref().map(|(b, s)| (b, *s)),
                    shake_offset: native_shake_offset,
                };
                crate::native_lock::native_lock_render_elements(frame, size, &mut udev.renderer)
            } else {
                Vec::new()
            };

            // The pointer, on a locked head too. A locked head renders only
            // the lock element list below, and the cursor push further up
            // is inside `if !locked`, so a locked screen had no pointer
            // drawn at all - and on a bare TTY nothing else draws one.
            // The on-screen keyboard's clicks were being handled correctly
            // the whole time (`native_lock_click`); they simply could not
            // be aimed. Reported live as "there is no mouse input to
            // interact with the screen keyboard in lockscreen".
            //
            // Built here rather than reusing `custom_elements` because that
            // list is assembled inside the same `if !locked` branch. First
            // in the list, so it stays above the lock UI it is used to
            // click on.
            let locked_cursor: Vec<crate::elements::OverlayElement<PixmanRenderer>> = if locked {
                crate::cursor::render_elements(&cursor_status, &cursor_buffers, &mut udev.renderer, udev.pointer_pos, origin, udev.heads[index].size)
            } else {
                Vec::new()
            };
            let head = &mut udev.heads[index];
            let mut framebuffer = match udev.renderer.bind(&mut head.buffers[back].image) {
                Ok(fb) => fb,
                Err(e) => {
                    log::error!("udev: pixman bind failed: {e}");
                    continue;
                }
            };

            // Locked heads draw either srdwm's own native lock UI (over
            // opaque black - the background element covers the visible
            // area, but the clear colour is still what shows through if a
            // capture failed or hasn't happened for this output yet) or an
            // external locker's surface the same way, and nothing else;
            // unlocked heads draw the normal scene.
            let result = if locked && is_native {
                let mut elements = locked_cursor;
                elements.extend(native_elements.into_iter().map(crate::elements::OverlayElement::Memory));
                head.damage_tracker
                    .render_output(&mut udev.renderer, &mut framebuffer, 0, &elements, [0.0, 0.0, 0.0, 1.0])
                    .map(|r| (r.damage.is_some(), Vec::new()))
                    .map_err(|e| e.to_string())
            } else if locked {
                let mut elements = locked_cursor;
                elements.extend(lock_elements.into_iter().map(crate::elements::OverlayElement::Surface));
                head.damage_tracker
                    .render_output(&mut udev.renderer, &mut framebuffer, 0, &elements, [0.0, 0.0, 0.0, 1.0])
                    .map(|r| (r.damage.is_some(), Vec::new()))
                    .map_err(|e| e.to_string())
            } else {
                // Not `smithay::desktop::space::render_output`: that
                // convenience wrapper draws `self.space`'s window content at
                // one `alpha` for the whole frame and pulls every
                // layer-shell surface in unconditionally, neither of which
                // leaves room for per-window opacity or hiding the bar/dock
                // during fullscreen. `custom_elements` above already carries
                // everything that wrapper would have built - window
                // content (`surface_content_elements`, one call per window,
                // each with its own `w.opacity`) and layer-shell surfaces
                // (`output_layer_elements`, split Top/Overlay above content
                // and Background/Bottom below it) - assembled by hand in
                // the correct front-to-back order instead. `self.space`
                // itself is untouched and still authoritative for
                // hit-testing/stacking bookkeeping (`sync_geometry`'s
                // `map_element` calls); only the *render* path stopped
                // reading from it.
                head.damage_tracker
                    .render_output(&mut udev.renderer, &mut framebuffer, head.ages[back], &custom_elements, [0.05, 0.05, 0.08, 1.0])
                    .map(|r| (r.damage.is_some(), r.damage.cloned().unwrap_or_default()))
                // Both arms reduce to "was there damage" plus the damage
                // rects themselves; the two error types differ, so they are
                // flattened to a message here.
                .map_err(|e| e.to_string())
            };
            if !locked {
                // Only this head's own captures: `captures` holds requests
                // for every output, and each must be read back from the
                // framebuffer it was actually requested against, not
                // whichever head happens to render first in this loop (a
                // multi-monitor capture would otherwise silently read the
                // wrong screen). Whatever doesn't match `output` stays in
                // `captures` for a later head this same pass.
                let (mine, rest): (Vec<_>, Vec<_>) = captures.into_iter().partition(|c| c.output == output);
                captures = rest;
                crate::screencopy::service_pending(mine, &mut udev.renderer, &framebuffer);

                // A native lock is waiting on this output's background --
                // this same freshly-rendered framebuffer (the ordinary
                // desktop scene, not a lock scene: `locked` is still
                // `false` here because `begin_native_lock` deliberately
                // doesn't flip it until every output has one, see that
                // function's own doc comment) is exactly "what's on
                // screen right now" for this output.
                if native_needs_capture {
                    let name = output.name();
                    let size = head.size;
                    match crate::native_lock::capture_and_blur(&mut udev.renderer, &framebuffer, size, lock_blur_radius) {
                        Ok(blurred) => new_captures.push((name, blurred)),
                        Err(e) => log::warn!("native lock: capture failed for output {name}: {e}"),
                    }
                }
            }
            drop(framebuffer);

            let (has_damage, damage_rects) = match result {
                Ok(v) => v,
                Err(e) => {
                    log::error!("udev: render_output failed: {e}");
                    continue;
                }
            };
            if has_damage {
                let head = &mut udev.heads[index];
                if let Err(e) = head.copy_and_flip(&udev.card, back, &damage_rects) {
                    // Backed off, not retried on the very next poll tick --
                    // see `UdevHead::flip_retry_after`'s own doc comment for
                    // the real, live-reproduced incident this prevents: a
                    // failing flip (confirmed live as `EBUSY` right after a
                    // VT-switch resume, while the kernel's own `set_crtc`
                    // commit was still settling) used to be retried
                    // immediately, forever, since nothing else gated
                    // `ready` on anything but `flip_pending` - which a
                    // failed `page_flip` call never sets. A fixed, short
                    // cooldown is enough to ride out that kind of transient
                    // kernel-side race without needing to distinguish it
                    // from a real, permanent failure - either way, hammering
                    // the same doomed `page_flip` call in a tight loop with
                    // no backoff at all was never the right response.
                    head.flip_retry_after = Some(Instant::now() + Duration::from_millis(200));
                    log::error!("udev: page flip failed: {e} - retrying in 200ms");
                    continue;
                }
                head.flip_retry_after = None;
                // This buffer is now fully up to date. It won't be rendered
                // into again until the *other* slot has also been presented
                // once (strict two-buffer alternation), so by then it will
                // be exactly 2 damage-producing renders stale - matching
                // `damage_tracker`'s own history, which only advances on
                // calls that actually found damage (see `ages`' doc
                // comment).
                head.ages[back] = 2;
                // Only a head that actually presented a new frame should
                // tell its windows they may render their next one - this
                // used to run unconditionally for every "ready" head (i.e.
                // every head not already mid-flip) on every single call to
                // this function, which is every ~16ms regardless of
                // activity. A client that renders on the standard
                // wait-for-frame-callback pattern (which is most of
                // them - confirmed live: wezterm-gui pinned at 140%+ CPU
                // sitting on a fully idle, unchanged terminal) had no
                // reason not to redraw at whatever rate this loop cycled,
                // forever, since it kept getting told a new frame was
                // wanted whether or not the screen had changed at all.
                presented.push((output, origin, damage_rects));
            }
        }

        // Applies every background captured during the loop above, now
        // that `self.udev`'s borrow has ended and `self` (specifically
        // `self.lock`) can be borrowed as a whole again - see
        // `capture_output`'s own doc comment for what happens once every
        // output has one (the lock actually engages).
        for (name, blurred) in new_captures {
            self.capture_output(&name, blurred);
        }

        // Frame callbacks + lock confirmation, once the `udev` borrow is done.
        for (output, origin, damage_rects) in presented {
            if locked {
                let surface = self.lock_surface_for(&output).cloned();
                crate::lock::send_lock_frame(surface.as_ref(), &output, elapsed);
                self.confirm_lock_if_presented(&output);
            } else {
                let out = output.clone();
                let scale = Scale::from(out.current_scale().fractional_scale());
                for w in crate::elements::windows_touched_by_damage(&self.space, &damage_rects, origin, scale) {
                    w.send_frame(&out, elapsed, None, |_, _| Some(out.clone()));
                }
            }
        }
        // Deliberately unconditional - not folded into the `presented`
        // loop above, and not gated on any head having had damage this
        // tick at all. The whole point of `always_notify` is covering the
        // case where the output has *no* damage whatsoever (a fully idle
        // desktop, cursor not moving) but the focused/hovered window still
        // has a pending callback it needs answered to unblock an input-
        // driven redraw - GTK's frame-clock model (Firefox's Wayland
        // vsync source included) paces every repaint through that
        // callback, even the first one after being idle, with no "just
        // commit immediately" fallback. Nesting this inside the `presented`
        // loop (the first version of this fix) meant it only ever ran on a
        // tick that already had damage from something else happening --
        // i.e. never in the exact scenario it exists for. Reported live as
        // clicks in Firefox still doing nothing at all, not just
        // intermittently, after the first version of this fix.
        if !locked {
            let pointer_pos = self.udev.as_ref().map(|u| u.pointer_pos).unwrap_or_default();
            let wm = self.wm.borrow();
            let always_notify = [wm.focused_id(), wm.window_at(pointer_pos.x as i32, pointer_pos.y as i32)];
            drop(wm);
            let outputs: Vec<Output> = self.outputs.iter().map(|e| e.output.clone()).collect();
            for w in always_notify.into_iter().flatten().filter_map(|id| self.id_to_window.get(&id)) {
                for out in &outputs {
                    w.send_frame(out, elapsed, None, |_, _| Some(out.clone()));
                }
            }
        }
        // Layer-shell surfaces (bars, docks, launchers) get their frame
        // callback on every pass, unconditionally - NOT folded into the
        // `presented`/`has_damage` gate above.
        //
        // That gate exists because most toplevel clients redraw on the
        // standard wait-for-callback loop regardless of whether their own
        // content changed (confirmed live: wezterm-gui pinned at 140%+ CPU
        // on a fully idle terminal when it got a callback every ~16ms
        // whether or not the screen had changed). Gating toplevel callbacks
        // on real output damage fixed that.
        //
        // Applying the same gate to layer surfaces creates a real deadlock
        // instead: many (GTK4/AGS among them) drive their *entire* repaint
        // loop off frame callbacks with no independent timer fallback --
        // paint once, request a callback, wait. If nothing ELSE on the
        // desktop ever produces damage again (a static terminal, no other
        // animation), that callback never arrives, so the surface can never
        // draw its next frame, which means it can never produce damage,
        // which means it never gets a callback - permanently frozen after
        // exactly one frame. Confirmed live: AGS and waybar both hung this
        // way, one frame in, with `wl_surface.frame` requests that were
        // never answered (see docs/PANEL_SUPPORT_TODO.md).
        //
        // Splitting the gate is safe rather than reintroducing the wezterm
        // bug: there are at most a handful of layer surfaces on a real
        // desktop (a bar, maybe a dock/launcher), their content is cheap to
        // redraw even when done needlessly, and periodic UI chrome (a
        // clock, a resource graph) is exactly the case frame callbacks
        // exist to pace - unlike a full toplevel window, whose redraw cost
        // is what made the unconditional case expensive in the first place.
        if !locked {
            for output in &ready_outputs {
                for layer in layer_map_for_output(output).layers() {
                    layer.send_frame(output, elapsed, None, |_, _| Some(output.clone()));
                }
            }
        }
        if locked {
            crate::screencopy::fail_pending(captures);
        } else if !captures.is_empty() {
            // Left over because their target head wasn't in `ready` this
            // pass (e.g. mid-page-flip). Put back rather than dropped: this
            // function runs again on the next poll tick (or the page-flip
            // completion that made the head ready), so the capture gets
            // another chance instead of silently vanishing - which is what
            // made `grim` hang waiting on a `ready`/`failed` that would
            // otherwise never come (see docs/PANEL_SUPPORT_TODO.md, P1).
            self.screencopy_pending.extend(captures);
        }
        const FRAME_BUDGET: Duration = Duration::from_millis(16);
        let frame_time = frame_start.elapsed();
        if frame_time > FRAME_BUDGET {
            let wm = self.wm.borrow();
            log::warn!(
                "PERF-RESIZE render_udev_frame took {frame_time:?} (budget {FRAME_BUDGET:?}) - resizing={} dragging={}",
                wm.resizing_window().is_some(),
                wm.is_dragging()
            );
        }
    }

    /// Sets a connector's DPMS mode via the generic KMS "DPMS" property --
    /// there is no dedicated legacy-API call for this in `drm-rs`, only the
    /// same `get_properties`/`set_property` pair every other connector
    /// property goes through, so the property has to be found by name each
    /// time rather than through some `Dpms` -specific method. `None` if the
    /// `wl_output` doesn't resolve to a live head, or the connector has no
    /// "DPMS" property at all (rare on real hardware, but virtual/headless
    /// outputs may not expose one) - either way maps to `zwlr_output_power_v1`'s
    /// `failed` event, matching what the protocol asks for when the mode
    /// can't be honoured.
    pub(crate) fn set_output_power(&self, wl_output: &smithay::reexports::wayland_server::protocol::wl_output::WlOutput, on: bool) -> Option<()> {
        // Raw KMS UAPI values for the "DPMS" connector property
        // (`DRM_MODE_DPMS_ON`/`_OFF` in `drm_sys`/the kernel's
        // `drm_mode.h`) - not worth a whole extra dependency on `drm-sys`
        // just for two constants that have been stable since DPMS was
        // added to the DRM UAPI.
        const DRM_MODE_DPMS_ON: u64 = 0;
        const DRM_MODE_DPMS_OFF: u64 = 3;

        let target = self.output_for_wl(wl_output)?.output.clone();
        let udev = self.udev.as_ref()?;
        let head = udev.heads.iter().find(|h| h.output == target)?;
        let props = udev.card.get_properties(head.connector).ok()?;
        let dpms_prop = props.as_props_and_values().0.iter().copied().find(|&handle| udev.card.get_property(handle).is_ok_and(|info| info.name().to_str() == Ok("DPMS")))?;
        let mode = if on { DRM_MODE_DPMS_ON } else { DRM_MODE_DPMS_OFF };
        udev.card.set_property(head.connector, dpms_prop, mode).ok()
    }

    /// The CRTC's gamma ramp length, in elements per channel - what
    /// `zwlr_gamma_control_v1.gamma_size` reports so a client knows how
    /// large a table `set_gamma` expects. `None` if the output doesn't
    /// resolve to a live head, or the CRTC reports a zero-length ramp
    /// (no gamma hardware, common on virtual/headless outputs).
    pub(crate) fn gamma_ramp_size(&self, wl_output: &smithay::reexports::wayland_server::protocol::wl_output::WlOutput) -> Option<u32> {
        let target = self.output_for_wl(wl_output)?.output.clone();
        let udev = self.udev.as_ref()?;
        let head = udev.heads.iter().find(|h| h.output == target)?;
        let len = udev.card.get_crtc(head.crtc).ok()?.gamma_length();
        (len > 0).then_some(len)
    }

    /// Reads a client-supplied gamma table (`zwlr_gamma_control_v1.
    /// set_gamma`'s `fd`: a memory-mapped blob of `gamma_size` `u16`s per
    /// channel, red then green then blue, per the protocol) and applies it
    /// to the CRTC. `None` on any failure - output/head not found, the
    /// blob is the wrong size, or the DRM `set_gamma` call itself fails --
    /// which the caller maps to `zwlr_gamma_control_v1.failed`, exactly
    /// what the protocol specifies for "setting the gamma tables failed".
    pub(crate) fn set_gamma_ramp(&self, wl_output: &smithay::reexports::wayland_server::protocol::wl_output::WlOutput, fd: std::os::fd::OwnedFd) -> Option<()> {
        let target = self.output_for_wl(wl_output)?.output.clone();
        let udev = self.udev.as_ref()?;
        let head = udev.heads.iter().find(|h| h.output == target)?;
        let size = udev.card.get_crtc(head.crtc).ok()?.gamma_length() as usize;
        if size == 0 {
            return None;
        }
        // Three channels, two bytes (one native-endian u16) per element --
        // client and compositor are always the same machine, so there is
        // no cross-endianness concern to handle here, unlike an over-the-
        // wire protocol value.
        let expected_bytes = size * 3 * 2;
        // SAFETY: the fd is a client-supplied shared-memory blob, mapped
        // read-only for the duration of this call and never touched again
        // afterwards - the same trust boundary `wl_shm` buffers already
        // cross for every window's actual pixel content elsewhere in this
        // codebase.
        let map = unsafe { memmap2::MmapOptions::new().map(&fd) }.ok()?;
        if map.len() < expected_bytes {
            return None;
        }
        let read_channel = |offset: usize| -> Vec<u16> { map[offset..offset + size * 2].chunks_exact(2).map(|b| u16::from_ne_bytes([b[0], b[1]])).collect() };
        let red = read_channel(0);
        let green = read_channel(size * 2);
        let blue = read_channel(size * 4);
        udev.card.set_gamma(head.crtc, &red, &green, &blue).ok()
    }
}

