use super::*;

impl CompState {
    /// Renders and (if there was damage) page-flips a new frame on every
    /// head that is ready for one. A head with a flip still in flight is
    /// skipped this pass and picked up when its page-flip event arrives, so
    /// monitors on different refresh rates each run at their own pace
    /// instead of the slowest one gating the rest.
    pub(crate) fn render_udev_frame(&mut self) {
        self.tick_animations();
        self.tick_dirty_broadcasts();
        let locked = self.lock.locked;
        let elapsed = self.start_time.elapsed();
        // Drained before the `&mut self.udev` borrow below, so screencopy can
        // be serviced with the renderer that borrow owns.
        let mut captures = std::mem::take(&mut self.screencopy_pending);
        // Same reason: the cursor needs the renderer that borrow owns.
        let cursor_status = self.cursor_status.clone();
        let cursor_buffers = self.cursor_buffers.clone();

        // Border geometry is in global space, independent of which head
        // renders it, so it's gathered once here rather than per head.
        // Buffers are pre-built for the same reason as `cursor_buffers`:
        // Rendered per window, front-to-back (topmost first), each window's
        // content immediately followed by its decoration and border --
        // fixes the same cross-window ordering bug documented in
        // `winit.rs`'s render loop: a background window's titlebar could
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
        let ids: Vec<srdwm_core::WindowId> = if locked { Vec::new() } else { self.wm.borrow().visible_windows_front_to_back().map(|w| w.id).collect() };
        let focused = self.wm.borrow().focused_id();
        // Default `false` here, unlike winit's `unwrap_or(true)` - see
        // `rounded_corners_pixman`'s module doc comment for the CPU cost
        // that makes this backend opt-in rather than on by default.
        let rounded_corners_enabled = self.wm.borrow().rounded_corners_enabled.unwrap_or(false);
        let popup_targets = if locked { Vec::new() } else { crate::elements::popup_targets(self) };

        // Which heads are eligible, and what each needs, gathered before the
        // mutable borrow of `self.udev`. Both early-outs below give the
        // `captures` taken above nowhere to go this pass - put them back
        // rather than silently dropping a client's pending screenshot
        // because a VT switch happened to be in progress at that instant.
        let Some(udev) = self.udev.as_ref() else {
            self.screencopy_pending.extend(captures);
            return;
        };
        if !udev.active {
            self.screencopy_pending.extend(captures);
            return;
        }
        let ready: Vec<(usize, Output)> = udev
            .heads
            .iter()
            .enumerate()
            .filter(|(_, h)| !h.flip_pending)
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
        let mut presented: Vec<(Output, Vec<Rectangle<i32, Physical>>)> = Vec::new();
        for (index, output) in ready {
            let lock_surface = self.lock_surface_for(&output).cloned();

            // Content/decoration elements are built per head: both need the
            // renderer, and geometry is translated into head-local space.
            let origin = self.udev.as_ref().map(|u| u.heads[index].location).unwrap_or_default();

            let Some(udev) = self.udev.as_mut() else { return };
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
                        (origin.x, origin.y),
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
                    // Drawn first among this window's own decoration, and
                    // positioned from the same animated `geom` as everything
                    // else here - not `w.geometry` - for the identical
                    // reason: a shadow that stayed at the pre-tween rect
                    // while the window slid past it would look exactly as
                    // detached as the border did before that fix. Not
                    // fragment-clipped against `occluders` like the titlebar/
                    // border below: at `SHADOW_MAX_ALPHA`'s low opacity, a
                    // shadow bleeding slightly onto a window stacked in front
                    // of this one reads as a soft edge, not the hard-line
                    // bleed-through that made the titlebar/border need it.
                    if let Some(shadow) = self.shadow_buffers.get(&id) {
                        let rect = decoration::shadow_rect(geom);
                        let pos = ((rect.x - origin.x) as f64, (rect.y - origin.y) as f64);
                        match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, pos, shadow, None, None, None, Kind::Unspecified) {
                            Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                            Err(e) => log::warn!("udev: failed to import shadow buffer: {e}"),
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
                        let titlebar_rect = srdwm_core::Rect::new(geom.x, geom.y, geom.width, srdwm_core::TITLEBAR_HEIGHT);
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
                    // Border strips sit entirely outside this window's own
                    // `geometry` (see `decoration::border_strips`), so they
                    // never overlap its own decoration/content - draw
                    // order against those doesn't matter here, only against
                    // other windows', which iterating `ids` in stacking
                    // order already gets right *for windows also drawn via
                    // this same custom_elements loop* - but not against
                    // any window's own *content*, which is why `occluders`
                    // below is still needed even with that ordering.
                    if w.border_width > 0 {
                        let color = crate::state::effective_border_color(w.border_color, focused == Some(id));
                        let strips = decoration::border_strips(geom, w.border_width);
                        // Strip 0 (top) rounded to match the titlebar under
                        // it - see `render_border_top`'s doc comment - so
                        // it's a cached bitmap (rebuilt only in
                        // `redraw_decoration_buffer`, same as the titlebar
                        // itself), not rasterized fresh here every frame.
                        // Not fragment-clipped like the other three strips
                        // below - cropping a bitmap's source rect per
                        // fragment is real extra work for a strip that's
                        // only `border_width` pixels tall to begin with, so
                        // this only handles the all-or-nothing case: skip
                        // entirely once *fully* covered, accept a small
                        // residual bleed while only partially covered.
                        if strips[0].width > 0 && strips[0].height > 0 && !strips[0].subtract_all(&occluders).is_empty() {
                            if let Some(buffer) = self.border_top_decorations.get(&id) {
                                let pos = ((strips[0].x - origin.x) as f64, (strips[0].y - origin.y) as f64);
                                match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, pos, buffer, None, None, None, Kind::Unspecified) {
                                    Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                                    Err(e) => log::warn!("udev: failed to import top border buffer: {e}"),
                                }
                            }
                        }
                        // The other three strips are persistent
                        // `SolidColorBuffer`s updated in place, not rebuilt
                        // with a fresh `Id` every frame - see
                        // `elements::border_side_render_element`'s doc
                        // comment for why that distinction is load-bearing
                        // for damage tracking, not cosmetic. Each strip is
                        // further split into whatever fragments remain
                        // visible after subtracting `occluders`, since a
                        // whole unclipped strip is exactly the bug fixed
                        // here.
                        let pool = self.border_side_buffers.entry(id).or_default();
                        let mut buf_index = 0;
                        for strip in &strips[1..] {
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
                            let pos = (geom.x - origin.x, geom.y + band - origin.y);
                            let mut rounded_elem = None;
                            if rounded_corners_enabled {
                                let epoch = self.content_epoch.get(&id).copied().unwrap_or(0);
                                // Bottom-only for a decorated window, same
                                // reasoning as `winit.rs`'s identical split:
                                // the top two corners are already hidden
                                // under the titlebar band's own rounded
                                // bitmap.
                                let corners = if w.decorated { crate::rounded_corners::RoundedCorners::BOTTOM_ONLY } else { crate::rounded_corners::RoundedCorners::ALL };
                                if let Some(buffer) =
                                    crate::elements::rounded_content_buffer(&mut self.rounded_content_buffers, epoch, id, &surface, decoration::CORNER_RADIUS as f32, corners)
                                {
                                    match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, (pos.0 as f64, pos.1 as f64), buffer, Some(w.opacity), None, None, Kind::Unspecified)
                                    {
                                        Ok(elem) => rounded_elem = Some(elem),
                                        Err(e) => log::warn!("udev: failed to import rounded content buffer: {e}"),
                                    }
                                }
                            }
                            match rounded_elem {
                                Some(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                                None => custom_elements.extend(crate::elements::surface_content_elements(&mut udev.renderer, &surface, pos, w.opacity)),
                            }
                        }
                    }
                    occluders.push(geom);
                }
                // Background/bottom layer-shell (wallpaper engines) last --
                // bottommost, matching smithay's own `space_render_elements`
                // ordering, which this whole custom loop now replaces.
                custom_elements.extend(crate::elements::output_layer_elements(
                    &mut udev.renderer,
                    &output,
                    (origin.x, origin.y),
                    |layer| matches!(layer, Layer::Background | Layer::Bottom),
                ));
            }
            let lock_elements = if locked {
                crate::lock::lock_render_elements(lock_surface.as_ref(), &mut udev.renderer)
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

            // Locked heads draw the lock surface over opaque black and
            // nothing else; unlocked heads draw the normal scene.
            let result = if locked {
                head.damage_tracker
                    .render_output(&mut udev.renderer, &mut framebuffer, 0, &lock_elements, [0.0, 0.0, 0.0, 1.0])
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
                if let Err(e) = head.copy_and_flip(&udev.card, back) {
                    log::error!("udev: page flip failed: {e}");
                    continue;
                }
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
                presented.push((output, damage_rects));
            }
        }

        // Frame callbacks + lock confirmation, once the `udev` borrow is done.
        for (output, damage_rects) in presented {
            if locked {
                let surface = self.lock_surface_for(&output).cloned();
                crate::lock::send_lock_frame(surface.as_ref(), &output, elapsed);
                self.confirm_lock_if_presented(&output);
            } else {
                let out = output.clone();
                let scale = Scale::from(out.current_scale().fractional_scale());
                for w in crate::elements::windows_touched_by_damage(&self.space, &damage_rects, scale) {
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

