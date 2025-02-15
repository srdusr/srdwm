use super::*;

impl WaylandPlatform {

    pub(super) fn render_frame(&mut self) -> PlatformResult<()> {
        self.state.tick_animations();
        self.state.tick_hover_glyph_animation();
        self.state.tick_dirty_broadcasts();
        let size = self.backend.window_size();
        let resized = self.output.current_mode().map(|m| m.size) != Some(size);
        if resized {
            // Only push a new output mode - and thus emit `wl_output.mode`/
            // `done` - when the size actually changed. This used to run
            // unconditionally every frame; harmless with no Wayland-native
            // client connected (the only way this was ever exercised before
            // real layer-shell clients existed), but a real client bound to
            // `wl_output` would otherwise be flooded with duplicate
            // mode/done events at the render loop's full frame rate.
            self.output.change_current_state(Some(OutputMode { size, refresh: 60_000 }), None, None, None);
            layer_map_for_output(&self.output).arrange();
        }

        let age = self.backend.buffer_age().unwrap_or(0);
        let (renderer, mut framebuffer) = self.backend.bind().map_err(err)?;

        // Locked: srdwm's own native lock UI, or an external locker's
        // surface, over an opaque black clear - nothing else, no windows,
        // no decorations, no layer surfaces, either way.
        if self.state.lock.locked && self.state.lock.native.is_some() {
            let name = self.output.name();
            let bg = self.state.native_lock_background(&name).cloned();
            let ui = self.state.native_lock_ui().map(|(buf, s)| (buf.clone(), s));
            let elements = crate::native_lock::native_lock_render_elements(bg.as_ref(), ui.as_ref().map(|(b, s)| (b, *s)), (size.w, size.h), renderer);
            self.damage_tracker
                .render_output(renderer, &mut framebuffer, age, &elements, [0.0, 0.0, 0.0, 1.0])
                .map_err(err)?;
            drop(framebuffer);
            self.backend.submit(None).map_err(err)?;
            screencopy::fail_pending(std::mem::take(&mut self.state.screencopy_pending));
            return Ok(());
        }
        if self.state.lock.locked {
            let lock_surface = self.state.lock_surface_for(&self.output).cloned();
            let elements = lock_render_elements(lock_surface.as_ref(), renderer);
            self.damage_tracker
                .render_output(renderer, &mut framebuffer, age, &elements, [0.0, 0.0, 0.0, 1.0])
                .map_err(err)?;
            drop(framebuffer);
            self.backend.submit(None).map_err(err)?;
            send_lock_frame(lock_surface.as_ref(), &self.output, self.state.start_time.elapsed());
            self.state.confirm_lock_if_presented(&self.output);
            screencopy::fail_pending(std::mem::take(&mut self.state.screencopy_pending));
            return Ok(());
        }

        // `WinitElement`, not `OverlayElement<GlesRenderer>` directly - see
        // `rounded_corners.rs`'s own doc comment on why content that gets
        // rounded needs a wider element type than everything else here,
        // and why that couldn't just be added as a new `OverlayElement`
        // variant instead. Every existing push below wraps its
        // `OverlayElement` in `WinitElement::Base`; only the new
        // rounded-content push (further down) uses `WinitElement::Rounded`
        // directly.
        let mut custom_elements: Vec<crate::rounded_corners::WinitElement> = Vec::new();
        // Night light/reading mode - pushed first (topmost) so it colours
        // everything else, including the context menu below: this backend
        // draws no cursor of its own to exempt (unlike udev/render.rs's
        // matching push), so there's nothing that needs to stay above it.
        // See `color_filter::render_element` for why this is a translucent
        // overlay rather than a true per-pixel shader.
        {
            let color_filter = self.wm.borrow().color_filter;
            let output_name = self.output.name();
            let buf = self.state.color_filter_buffers.entry(output_name).or_insert_with(SolidColorBuffer::default);
            if let Some(elem) = crate::color_filter::render_element(buf, color_filter, (size.w, size.h)) {
                custom_elements.push(crate::rounded_corners::WinitElement::Base(crate::elements::OverlayElement::Solid(elem)));
            }
        }
        // The right-click titlebar menu, if open - pushed first so it's
        // topmost over every window (this backend draws no cursor of its
        // own, see this module's doc comment, so there's no "stay under
        // the pointer" ordering concern like udev/render.rs's matching push has).
        if let (Some(menu), Some(buffer)) = (self.state.context_menu.as_ref(), self.state.context_menu_buffer.as_ref()) {
            let pos = (menu.pos.0 as f64, menu.pos.1 as f64);
            match MemoryRenderBufferRenderElement::from_buffer(renderer, pos, buffer, None, None, None, Kind::Unspecified) {
                Ok(elem) => custom_elements.push(crate::rounded_corners::WinitElement::Base(crate::elements::OverlayElement::Memory(elem))),
                Err(e) => log::warn!("failed to import context menu buffer: {e}"),
            }
        }
        // The Snap-Layouts flyout, if open - same topmost placement.
        if let (Some(flyout), Some(buffer)) = (self.state.snap_flyout.as_ref(), self.state.snap_flyout_buffer.as_ref()) {
            let pos = (flyout.pos.0 as f64, flyout.pos.1 as f64);
            match MemoryRenderBufferRenderElement::from_buffer(renderer, pos, buffer, None, None, None, Kind::Unspecified) {
                Ok(elem) => custom_elements.push(crate::rounded_corners::WinitElement::Base(crate::elements::OverlayElement::Memory(elem))),
                Err(e) => log::warn!("failed to import snap flyout buffer: {e}"),
            }
        }
        // Content now renders here too, one window at a time, not through
        // `render_output`'s own `spaces` argument - see this function's
        // own call to `damage_tracker.render_output` further down for why,
        // and `elements.rs`'s `surface_content_elements` doc comment for
        // what it's actually for (per-window opacity, impossible through
        // `spaces`, which takes one `alpha` for the whole frame).
        //
        // An earlier version of this exact change was reverted: with two or
        // more native Wayland toplevels on screen, whichever was created
        // *first* always painted in front of later ones regardless of real
        // focus/stacking order. That bug's real root cause (found by
        // instrumenting a locally vendored smithay copy directly) turned
        // out to be `sync_geometry`'s `Space::map_element` call silently
        // re-stacking windows to the top of `Space`'s *own* internal
        // order as a side effect of updating position - see `state/tick.rs`'s
        // `resync_stacking_order` doc comment for the full story and the
        // fix that landed for it (called after every `map_element` since).
        // This loop never reads `Space`'s order at all: `ids` below comes
        // from `WindowManager.order` (`visible_windows_front_to_back`),
        // srdwm's own stacking model, the same source `hit_test` already
        // trusts - so the specific bug that sank the earlier attempt
        // can't recur here regardless of whether `resync_stacking_order`
        // ever drifts again. `self.state.space` stays mapped and
        // `resync_stacking_order`-maintained exactly as before; only the
        // render step stopped reading from it.
        let ids: Vec<WindowId> = self.wm.borrow().visible_windows_front_to_back().map(|w| w.id).collect();
        let focused = self.wm.borrow().focused_id();
        // Popups next: always above every window's own content - see the
        // matching comment in `udev/render.rs`'s render loop for why this has to
        // be pushed ahead of both the bar/dock and every window now that
        // content shares this same list.
        let popup_targets = crate::elements::popup_targets(&self.state);
        custom_elements.extend(crate::elements::popup_render_elements(&popup_targets, renderer, (0, 0)).into_iter().map(crate::rounded_corners::WinitElement::Base));
        // The bar/dock/launcher, skipped entirely for a fullscreen window --
        // see `udev/render.rs`'s matching push for the full reasoning.
        let hide_top_layers = self.wm.borrow().visible_windows_front_to_back().any(|w| w.fullscreen);
        // `None` (the user's config never touched `general.rounded_corners`)
        // defaults to *on* here - this backend has an actual GPU shader
        // behind the feature (see `rounded_corners.rs`), no untested
        // per-frame CPU cost to weigh the way the udev backend's own
        // default has to.
        let rounded_corners_enabled = self.wm.borrow().rounded_corners_enabled.unwrap_or(true);
        if !hide_top_layers {
            custom_elements.extend(
                crate::elements::output_layer_elements(renderer, &self.output, |layer| matches!(layer, Layer::Top | Layer::Overlay))
                    .into_iter()
                    .map(crate::rounded_corners::WinitElement::Base),
            );
        }
        // Windows stacked in front of whichever one border/decoration is
        // being built right now - `ids` is already front-to-back, so this
        // only ever needs appending to, not recomputing. A window's own
        // *content*, pushed inside this same loop below, needs no separate
        // occlusion test - see the matching comment in `udev/render.rs`'s render
        // loop for why ordinary front-to-back push order already occludes
        // it correctly. The border strips and titlebar bitmap are
        // different: outside `geometry`, so they still need `occluders`'
        // explicit clip against whichever window is stacked in front.
        let mut occluders: Vec<srdwm_core::Rect> = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(w) = self.wm.borrow().window(id).cloned() else { continue };
            // `w.geometry` is the animation's target, not necessarily where
            // the window is actually drawn this frame - see the matching
            // comment in `udev/render.rs`'s render loop for the full story
            // (reported live as the border "not flush" with the window
            // during an animated maximize/fullscreen/open-slide transition).
            let geom = self.state.window_anims.get(&id).map(crate::state::WindowAnim::current_rect).unwrap_or(w.geometry);
            // `geom` is this compositor's own request/target; `frame`
            // corrects its far edge to match what the client's surface
            // really committed - see `effective_frame`'s own doc comment
            // and the matching comment in `udev/render.rs`'s render loop.
            let frame = self.state.effective_frame(id, geom);
            if let Some(deco) = self.state.decorations.get(&id) {
                // Fragment-clipped, same as udev/render.rs's matching titlebar
                // push - see that comment for why all-or-nothing (skip
                // only once *fully* covered) wasn't enough: a titlebar
                // only partially covered, the common case for cascaded
                // windows, still bled through the covered part.
                let titlebar_rect = srdwm_core::Rect::new(frame.x, frame.y, frame.width, srdwm_core::TITLEBAR_HEIGHT);
                for fragment in crate::elements::visible_border_fragments(titlebar_rect, &occluders) {
                    let pos = (fragment.x as f64, fragment.y as f64);
                    let src = Rectangle::new(
                        Point::from(((fragment.x - titlebar_rect.x) as f64, (fragment.y - titlebar_rect.y) as f64)),
                        Size::from((fragment.width as f64, fragment.height as f64)),
                    );
                    match MemoryRenderBufferRenderElement::from_buffer(renderer, pos, deco, None, Some(src), None, Kind::Unspecified) {
                        Ok(elem) => custom_elements.push(crate::rounded_corners::WinitElement::Base(crate::elements::OverlayElement::Memory(elem))),
                        Err(e) => log::warn!("failed to import titlebar buffer for window {id}: {e}"),
                    }
                }
            }
            // Border strips sit entirely outside `geometry` (see
            // `decoration::border_strips`), so they never overlap this same
            // window's own decoration/content pixels - draw order relative
            // to those doesn't matter, only relative to other windows'.
            // (The left/right strips *do* still need cropping against the
            // top/bottom strip's own extended curve - see that crop's own
            // doc comment further down; that's an overlap between two
            // pieces of this window's own decoration, not with its content,
            // so it doesn't contradict this paragraph.)
            if w.border_width > 0 {
                let color = crate::state::effective_border_color(w.border_color, focused == Some(id), self.wm.borrow().theme.border_inactive_dim);
                let strips = decoration::border_strips(frame, w.border_width);
                // Strips 0/1 (top/bottom) are rounded on their own two
                // corners - see `render_border_top`/`render_border_bottom`'s
                // doc comments - so both are cached bitmaps (rebuilt only in
                // `redraw_decoration_buffer`, same as the titlebar itself),
                // not rasterized fresh here every frame; the remaining two
                // (left/right) never touch a corner and stay persistent
                // solid-colour buffers instead - see `elements::
                // border_side_render_element`'s doc comment for why a
                // per-frame rebuild of either was a real, continuous cost,
                // not a cosmetic one. Not fragment-clipped like the left/
                // right strips below - see the matching comment in
                // `udev/render.rs` for why top/bottom only get the cheaper
                // all-or-nothing occlusion check.
                if strips[0].width > 0 && strips[0].height > 0 && !strips[0].subtract_all(&occluders).is_empty() {
                    if let Some(buffer) = self.state.border_top_decorations.get(&id) {
                        // See `decoration::border_top_visible_rows`'s own
                        // doc comment: an undecorated window has no
                        // titlebar band to safely absorb this buffer's own
                        // corner-curve-only extra rows, so they're cropped
                        // away instead of landing on real content.
                        let (row0, rows, shift) = decoration::border_top_visible_rows(w.decorated, w.border_width, w.corner_radius);
                        let pos = (strips[0].x as f64, (strips[0].y + shift as i32) as f64);
                        let src = Some(Rectangle::new(Point::from((0.0, row0 as f64)), Size::from((strips[0].width as f64, rows as f64))));
                        match MemoryRenderBufferRenderElement::from_buffer(renderer, pos, buffer, None, src, None, Kind::Unspecified) {
                            Ok(elem) => custom_elements.push(crate::rounded_corners::WinitElement::Base(crate::elements::OverlayElement::Memory(elem))),
                            Err(e) => log::warn!("failed to import top border buffer for window {id}: {e}"),
                        }
                    }
                }
                // Same all-or-nothing bitmap treatment as the top strip,
                // for its own two corners - see `decoration::
                // render_border_bottom`'s doc comment.
                if strips[1].width > 0 && strips[1].height > 0 && !strips[1].subtract_all(&occluders).is_empty() {
                    if let Some(buffer) = self.state.border_bottom_decorations.get(&id) {
                        // See `decoration::border_bottom_visible_rows`'s
                        // own doc comment.
                        let (row0, rows, shift) = decoration::border_bottom_visible_rows(w.decorated, w.border_width, w.corner_radius);
                        let pos = (strips[1].x as f64, (strips[1].y - shift as i32) as f64);
                        let src = Some(Rectangle::new(Point::from((0.0, row0 as f64)), Size::from((strips[1].width as f64, rows as f64))));
                        match MemoryRenderBufferRenderElement::from_buffer(renderer, pos, buffer, None, src, None, Kind::Unspecified) {
                            Ok(elem) => custom_elements.push(crate::rounded_corners::WinitElement::Base(crate::elements::OverlayElement::Memory(elem))),
                            Err(e) => log::warn!("failed to import bottom border buffer for window {id}: {e}"),
                        }
                    }
                }
                // Cropped top and bottom by `extra` - see the matching fix
                // (and its own doc comment) in `udev/render.rs`'s identical
                // side-strip loop: the top/bottom strip's own curve extends
                // `corner_radius - border_width` rows into what would
                // otherwise be these flat, curve-unaware side strips' own
                // nominal top/bottom rows, and without this crop their
                // solid fill bled through the curve's own transparent
                // cutout as a straight vertical line poking out of an
                // otherwise correctly-rounded corner - reported live,
                // confirmed via raw pixel sampling on the udev backend;
                // this backend shares the identical strip geometry and was
                // never actually confirmed clean, just never specifically
                // screenshotted the same way.
                let extra = if w.decorated { w.border_width.max(w.corner_radius).saturating_sub(w.border_width) } else { 0 };
                let mut side_strips = [strips[2], strips[3]];
                for s in &mut side_strips {
                    s.y += extra as i32;
                    s.height = s.height.saturating_sub(2 * extra);
                }
                let pool = self.state.border_side_buffers.entry(id).or_default();
                let mut buf_index = 0;
                for strip in &side_strips {
                    if strip.width == 0 || strip.height == 0 {
                        continue;
                    }
                    for fragment in crate::elements::visible_border_fragments(*strip, &occluders) {
                        let buf = crate::elements::border_fragment_buffer(pool, buf_index);
                        buf_index += 1;
                        custom_elements.push(crate::rounded_corners::WinitElement::Base(crate::elements::OverlayElement::Solid(crate::elements::border_side_render_element(buf, fragment, color, (0, 0)))));
                    }
                }
            }
            // Shadow - pushed *after* the titlebar/border above, not
            // before. See the matching fix (and its full explanation) in
            // `udev/render.rs`'s render loop: `custom_elements` treats
            // earlier-pushed as topmost, so a shadow pushed before this
            // window's own border rendered on top of it, alpha-blending
            // black over the configured border colour and muting it into a
            // hazy smear instead of a crisp line - reported live as
            // "spacing before the border". Positioned from `geom`, not
            // `w.geometry`, same reasoning as the border above (a stale-
            // position shadow during an animated tween looks as detached
            // as the border did before that fix). Not fragment-clipped
            // against `occluders` like the titlebar/border above: at
            // `SHADOW_MAX_ALPHA`'s low opacity, a shadow bleeding slightly
            // onto a window stacked in front of this one reads as a soft
            // edge, not the hard-line bleed-through that made the
            // titlebar/border need it.
            if let Some(shadow) = self.state.shadow_buffers.get(&id) {
                let rect = decoration::shadow_rect(frame);
                let pos = (rect.x as f64, rect.y as f64);
                match MemoryRenderBufferRenderElement::from_buffer(renderer, pos, shadow, None, None, None, Kind::Unspecified) {
                    Ok(elem) => custom_elements.push(crate::rounded_corners::WinitElement::Base(crate::elements::OverlayElement::Memory(elem))),
                    Err(e) => log::warn!("failed to import shadow buffer for window {id}: {e}"),
                }
            }
            // The window's own content, at its own `opacity` - see the
            // matching push in `udev/render.rs`'s render loop for why. Single
            // output at the global origin, so no offset to subtract (see
            // `elements.rs`'s doc comment on why `udev/render.rs`'s per-head call
            // does). Rounded via `rounded_corners::rounded_content_element`
            // when the feature's on and the shader compiled - a decorated
            // window only rounds its bottom two corners (the top two are
            // already rounded, on the titlebar's own bitmap, by
            // `decoration.rs`), an undecorated/CSD one rounds all four,
            // since its content *is* the window's whole visible extent.
            // Falls back to the plain, unrounded `surface_content_elements`
            // on any failure - no committed buffer yet, a single-pixel
            // solid-colour buffer, or the feature simply being off - same
            // "always show something over a prettier maybe-nothing"
            // reasoning `cursor.rs`'s built-in-arrow fallback already uses.
            if let Some(dwindow) = self.state.id_to_window.get(&id) {
                if let Some(surface) = crate::elements::window_wl_surface(dwindow) {
                    let band = if w.decorated { srdwm_core::TITLEBAR_HEIGHT as i32 } else { 0 };
                    // See the matching fix in `udev/render.rs`'s render loop
                    // for the full explanation: a CSD client's own
                    // `set_window_geometry` offset (its declared visible
                    // content within a larger buffer that also reserves an
                    // invisible shadow margin) was never subtracted, so
                    // that margin's worth of gap showed through at this
                    // window's top-left corner.
                    let content_offset = dwindow.geometry().loc;
                    let pos = (geom.x - content_offset.x, geom.y + band - content_offset.y);
                    let rounded = rounded_corners_enabled.then_some(self.state.rounded_corners_program.as_ref()).flatten().and_then(|program| {
                        let corners = if w.decorated { crate::rounded_corners::RoundedCorners::BOTTOM_ONLY } else { crate::rounded_corners::RoundedCorners::ALL };
                        crate::rounded_corners::rounded_content_element(renderer, program, &surface, pos, w.opacity, w.corner_radius as f32, corners)
                    });
                    match rounded {
                        Some(elem) => custom_elements.push(crate::rounded_corners::WinitElement::Rounded(elem)),
                        None => custom_elements.extend(crate::elements::surface_content_elements(renderer, &surface, pos, w.opacity).into_iter().map(crate::rounded_corners::WinitElement::Base)),
                    }
                }
            }
            occluders.push(frame);
        }
        // Background/bottom layer-shell (wallpaper engines) last --
        // bottommost, matching smithay's own `space_render_elements`
        // ordering, which this whole custom loop now replaces.
        custom_elements.extend(
            crate::elements::output_layer_elements(renderer, &self.output, |layer| matches!(layer, Layer::Background | Layer::Bottom))
                .into_iter()
                .map(crate::rounded_corners::WinitElement::Base),
        );

        // Not `smithay::desktop::space::render_output`: see `udev/render.rs`'s
        // matching call site for why (per-window opacity, fullscreen-aware
        // layer-shell inclusion - `custom_elements` above already carries
        // everything that wrapper would have built).
        let result = self
            .damage_tracker
            .render_output(renderer, &mut framebuffer, age, &custom_elements, [0.05, 0.05, 0.08, 1.0])
            .map_err(err)?;
        let damage_rects: Vec<Rectangle<i32, Physical>> = result.damage.cloned().unwrap_or_default();
        let has_damage = !damage_rects.is_empty();
        // A native lock is waiting on this output's background - same
        // capture hook as `udev/render.rs`'s matching point, exercised
        // here too so a native lock can be tested against this nested dev
        // session without ever touching the live tty1 one. `self.state.
        // lock.locked` is still `false` at this point (`begin_native_lock`
        // doesn't flip it until every output has a background - see its
        // own doc comment), so the frame just rendered into `framebuffer`
        // is the ordinary desktop scene, not a lock scene.
        if self.state.native_lock_needs_capture(&self.output.name()) {
            let name = self.output.name();
            let blur_radius = self.state.wm.borrow().lock.blur_radius;
            match crate::native_lock::capture_and_blur(renderer, &framebuffer, size.into(), blur_radius) {
                Ok(blurred) => self.state.capture_output(&name, blurred),
                Err(e) => log::warn!("native lock: capture failed for output {name}: {e}"),
            }
        }
        drop(framebuffer);
        // Both the buffer swap and the frame-callback notification are
        // conditional on real damage now - this used to run
        // unconditionally on every call to this function (every ~16ms
        // regardless of activity), which told every window it could render
        // its next frame whether or not the screen had actually changed.
        // Any client using the standard wait-for-frame-callback render
        // pattern (most of them) had no reason not to redraw at whatever
        // rate this loop cycled, forever - confirmed live on the udev
        // backend: wezterm-gui pinned at 140%+ CPU sitting on a fully idle,
        // unchanged terminal, from the identical bug there.
        //
        // That output-wide gate wasn't enough on its own: cursor motion
        // alone damages the small region around the pointer, which still
        // marked the *whole output* damaged and sent every mapped window a
        // callback regardless of whether the cursor was anywhere near it.
        // `windows_touched_by_damage` narrows this to windows the actual
        // damage rectangles overlap - see its doc comment in elements.rs.
        if has_damage {
            self.backend.submit(None).map_err(err)?;
            let scale = Scale::from(self.output.current_scale().fractional_scale());
            let now = self.state.start_time.elapsed();
            for w in crate::elements::windows_touched_by_damage(&self.state.space, &damage_rects, (0, 0).into(), scale) {
                w.send_frame(&self.output, now, None, |_, _| Some(self.output.clone()));
            }
        }
        // Deliberately outside `if has_damage`: the whole point of
        // `always_notify` is covering the case where the output has *no*
        // damage at all (a fully idle desktop, cursor not moving) but the
        // focused/hovered window still has a pending callback it needs
        // answered to unblock an input-driven redraw. Nesting this inside
        // `if has_damage` (the first version of this fix) meant it only
        // ever ran on a tick that already had damage from something else
        // happening - i.e. never in the exact scenario it exists for.
        // Reported live as clicks in Firefox still doing nothing at all,
        // not just intermittently, after the first version of this fix.
        {
            let pointer_pos = last_pointer_pos(&self.state);
            let now = self.state.start_time.elapsed();
            let wm = self.wm.borrow();
            let always_notify = [wm.focused_id(), wm.window_at(pointer_pos.x as i32, pointer_pos.y as i32)];
            drop(wm);
            for w in always_notify.into_iter().flatten().filter_map(|id| self.state.id_to_window.get(&id)) {
                w.send_frame(&self.output, now, None, |_, _| Some(self.output.clone()));
            }
        }
        // Layer-shell surfaces get their callback every pass, unconditionally
        // - NOT folded into the `has_damage` gate above. See the matching
        // (much longer) comment in udev/render.rs's `render_udev_frame`: many
        // layer-shell clients (GTK4/AGS among them) drive their entire
        // repaint loop off frame callbacks with no independent timer
        // fallback, so withholding the callback until *something* on the
        // desktop happens to produce damage deadlocks them permanently
        // after their first frame - confirmed live, AGS and waybar both
        // froze exactly this way. Toplevel windows keep the damage gate
        // (that's what fixed the wezterm-gui CPU-burn bug); layer surfaces
        // are few, cheap to redraw, and are exactly the periodic-UI-chrome
        // case frame callbacks exist to pace.
        for layer in layer_map_for_output(&self.output).layers() {
            layer.send_frame(&self.output, self.state.start_time.elapsed(), None, |_, _| Some(self.output.clone()));
        }

        // Screencopy is serviced *after* the on-screen frame is submitted,
        // into its own offscreen buffer - never by reading back the window
        // surface. Reading the winit backend's EGL window surface (what an
        // earlier version did) reliably killed the GL context: the first
        // `grim` capture produced `eglSwapBuffers: BAD_SURFACE` followed by
        // `BAD_ALLOC` and "context has been lost", confirmed by A/B-ing the
        // same build with only the readback call removed. The offscreen
        // detour costs a second scene render, but only on frames where a
        // capture was actually requested.
        //
        // Deliberately placed after the locked-session early return above,
        // so a capture requested while the screen is locked can never see
        // client content.
        let captures = std::mem::take(&mut self.state.screencopy_pending);
        if !captures.is_empty() {
            if let Err(e) = self.capture_offscreen(captures) {
                log::warn!("screencopy: offscreen capture pass failed: {e}");
            }
        }
        Ok(())
    }
}
