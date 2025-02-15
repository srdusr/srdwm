use super::*;

impl CompState {

    /// Re-broadcasts any dock/panel-facing protocol state that changed by a
    /// path with no direct hook back into this module - specifically a
    /// compositor keybinding via `crates/config`'s `WindowAction`/
    /// `srd.workspace.*` API, which only ever touches `WindowManager` and
    /// has no way to reach `CompState`. See `foreign_toplevel::
    /// broadcast_dirty_state` and `workspace::broadcast_dirty_active`'s own
    /// doc comments for the full story; called once per frame alongside
    /// `tick_animations`, from both backends' poll loops.
    pub(crate) fn tick_dirty_broadcasts(&mut self) {
        foreign_toplevel::broadcast_dirty_state(self);
        workspace::broadcast_dirty_active(self);
        output_management::broadcast_dirty_outputs(self);
    }

    /// Advances every in-flight `WindowAnim` by one frame; called once per
    /// redraw from both backends' poll loops. A finished tween is dropped
    /// *before* its final `sync_geometry` call, so that call lands exactly
    /// on `Window.geometry` (the authoritative target) rather than on
    /// whatever the eased curve's last sub-pixel step happened to be.
    pub(crate) fn tick_animations(&mut self) {
        if self.window_anims.is_empty() {
            return;
        }
        let ids: Vec<WindowId> = self.window_anims.keys().copied().collect();
        for id in ids {
            if self.window_anims.get(&id).is_some_and(WindowAnim::is_done) {
                self.window_anims.remove(&id);
            }
            self.sync_geometry(id);
        }
    }

    /// Forces a fresh `redraw_decoration_buffer` call every frame while the
    /// titlebar-button glyph-reveal-on-hover animation is still in
    /// progress; called once per redraw from both backends' poll loops,
    /// alongside `tick_animations`. Needed for the same reason that one
    /// is: `redraw_decoration_buffer`'s own signature-based cache only
    /// rebuilds when *called*, and nothing else calls it once a pointer
    /// stops moving over an already-hovered button - without this, the
    /// glyph would jump straight from invisible to full opacity on the
    /// one motion event that started the hover, then never update again
    /// for the rest of the animation's own duration, since no further
    /// motion event arrives to drive it. Does nothing in `theme.
    /// button_glyph_always` mode or once the animation has actually
    /// finished (`HOVER_GLYPH_DURATION` elapsed) - both are already a
    /// stable, cached final state with nothing left to advance.
    pub(crate) fn tick_hover_glyph_animation(&mut self) {
        let Some((id, _, start)) = self.hovered_titlebar_button else { return };
        if self.wm.borrow().theme.button_glyph_always || start.elapsed() >= decoration::HOVER_GLYPH_DURATION {
            return;
        }
        self.redraw_decoration_buffer(id);
    }

    /// Re-applies `WindowManager`'s own stacking order to `Space`, bottom
    /// to top.
    ///
    /// `Space::map_element` (smithay 0.7.0) always re-stacks its target to
    /// the top of `Space`'s internal order as an unconditional side effect
    /// of updating its tracked position - true regardless of the
    /// `activate` argument, and there is no "move without restacking" in
    /// this smithay version. `sync_geometry` calls `map_element` for
    /// reasons that have nothing to do with raising a window at all (a
    /// title/app_id changing, an ordinary resize frame, a workspace
    /// switch), so every one of those silently raised the window it
    /// touched to the top of `Space`'s order regardless of which window
    /// `WindowManager`/the user actually considered focused or on top.
    /// Real-world effect, confirmed live: two windows created moments
    /// apart, each independently going through their own startup
    /// title/app_id negotiation, would each trigger a handful of
    /// `sync_geometry` calls purely from that startup sequence - so
    /// whichever one happened to settle *last* silently won `Space`'s
    /// notion of "on top", a race with no relationship to which window was
    /// actually focused. Reported live as a background window's content
    /// and decoration randomly painting in front of the actually-focused
    /// window on top of it. Calling this right after any `map_element`
    /// restores `Space`'s order to exactly match `WindowManager.order`
    /// (which `restack_pinned` already keeps pinned windows at the tail
    /// of), so the two can never drift apart again.
    pub(crate) fn resync_stacking_order(&mut self) {
        let order: Vec<WindowId> = self.wm.borrow().stacking_order().map(|w| w.id).collect();
        for id in order {
            if let Some(w) = self.id_to_window.get(&id).cloned() {
                self.space.raise_element(&w, false);
            }
        }
    }
}
