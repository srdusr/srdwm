//! Input routing: keyboard, pointer, and what "focus" means.
//!
//! Shared by both backends - smithay delivers keyboard/pointer events
//! through generic `InputBackend` traits, so the precise keybinding matching
//! and titlebar hit-testing exist once here and are called from the winit
//! backend ([`crate::winit`]) and the libinput/udev one ([`crate::udev`])
//! alike.
//!
//! Every function that routes an event checks the session lock first: while
//! locked, input goes to the lock surface and nowhere else. See
//! [`crate::lock`].

use smithay::backend::input::{ButtonState as BackendButtonState, KeyState as BackendKeyState, KeyboardKeyEvent};
use smithay::desktop::{layer_map_for_output, Window as DWindow, WindowSurfaceType};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{ButtonEvent, MotionEvent};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, Layer};

use srdwm_core::{Event as CoreEvent, Modifiers, TitlebarHit, WindowId};

use crate::state::CompState;

pub(crate) fn last_pointer_pos(state: &CompState) -> Point<f64, Logical> {
    state.seat.get_pointer().map(|p| p.current_location()).unwrap_or_default()
}

/// Topmost layer-shell surface (if any) under `pos`, checked in the same
/// above-everything-else stacking order `space_render_elements` renders
/// `Overlay`/`Top` layers in (bars, launchers, notifications, lock UIs).
/// `Background`/`Bottom` layers (wallpapers) deliberately aren't checked
/// here: nothing in scope for the daily-driver gate needs pointer input
/// routed to them, and space windows should stay clickable over a
/// wallpaper.
/// `pos` is in the global space; layer geometry is relative to its own
/// output, so the pointer is translated into output-local coordinates
/// before hit-testing and the result translated back out.
pub(crate) fn layer_surface_under(state: &CompState, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<i32, Logical>)> {
    let entry = state.output_at(pos)?;
    let origin = entry.location;
    let local = pos - origin.to_f64();
    let map = layer_map_for_output(&entry.output);
    for layer_kind in [Layer::Overlay, Layer::Top] {
        let Some(layer) = map.layer_under(layer_kind, local) else { continue };
        let Some(geo) = map.layer_geometry(layer) else { continue };
        if let Some((surface, surface_loc)) = layer.surface_under(local - geo.loc.to_f64(), WindowSurfaceType::ALL) {
            return Some((surface, origin + geo.loc + surface_loc));
        }
    }
    None
}

pub(crate) fn handle_pointer_position(state: &mut CompState, pos: Point<f64, Logical>, time: u32) {
    // Locked: pointer motion goes to the lock surface only. No hit-testing
    // against windows/decorations, so no hover, no drag, no resize.
    if state.lock.locked {
        let surface = state.any_lock_surface().cloned();
        if let Some(pointer) = state.seat.get_pointer() {
            let focus = surface.map(|s| (s, Point::from((0, 0)).to_f64()));
            pointer.motion(state, focus, &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
        }
        return;
    }

    let layer_hit = layer_surface_under(state, pos);
    let hit = state.wm.borrow().hit_test(pos.x as i32, pos.y as i32);
    let under = state.space.element_under(pos).map(|(w, loc)| (w.clone(), loc));

    let Some(pointer) = state.seat.get_pointer() else { return };
    if let Some((surface, loc)) = layer_hit {
        let surface_loc = pos - loc.to_f64();
        pointer.motion(state, Some((surface, loc.to_f64())), &MotionEvent { location: surface_loc, serial: SERIAL_COUNTER.next_serial(), time });
    } else if hit.is_some() {
        // Over our own decoration - no client focus.
        pointer.motion(state, None, &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
    } else if let Some((window, loc)) = under {
        if let Some(surface) = window.toplevel().map(|t| t.wl_surface().clone()) {
            let surface_loc = pos - loc.to_f64();
            pointer.motion(state, Some((surface, loc.to_f64())), &MotionEvent { location: surface_loc, serial: SERIAL_COUNTER.next_serial(), time });
        }
    } else {
        pointer.motion(state, None, &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
    }

    let mut wm = state.wm.borrow_mut();
    let dragging_or_resizing = wm.is_dragging() || wm.is_resizing();
    if wm.is_dragging() {
        wm.update_drag(pos.x as i32, pos.y as i32);
    } else if wm.is_resizing() {
        wm.update_resize(pos.x as i32, pos.y as i32);
    }
    let focused = wm.focused_id();
    drop(wm);
    if dragging_or_resizing {
        if let Some(id) = focused {
            state.sync_geometry(id);
        }
    }
}

/// The underlying `wl_surface` for a mapped window, regardless of whether
/// it's a native `xdg-shell` toplevel or an XWayland `X11Surface` --
/// `desktop::Window` exposes these as two separate accessors with no
/// shared one.
pub(crate) fn dwindow_wl_surface(w: &DWindow) -> Option<WlSurface> {
    if let Some(top) = w.toplevel() {
        return Some(top.wl_surface().clone());
    }
    w.x11_surface().and_then(|x| x.wl_surface())
}

/// Requests a client close its window, whichever kind it is.
pub(crate) fn close_dwindow(w: &DWindow) {
    if let Some(top) = w.toplevel() {
        top.send_close();
    } else if let Some(x11) = w.x11_surface() {
        let _ = x11.close();
    }
}

/// Focuses `id` in our own `WindowManager` *and* gives its surface real
/// Wayland/X11 keyboard focus - without this, a window can be raised and
/// tiled correctly yet never receive a single keystroke.
pub(crate) fn focus_window(state: &mut CompState, id: WindowId) {
    state.wm.borrow_mut().focus_window(id);
    state.pending.borrow_mut().push(CoreEvent::WindowFocused(id));
    let surface = state.id_to_window.get(&id).and_then(dwindow_wl_surface);
    // Routed through `set_keyboard_focus` (rather than calling
    // `KeyboardHandle::set_focus` directly) so clipboard/primary-selection
    // focus follows window focus too - see that method's doc comment.
    state.set_keyboard_focus(surface);
}

pub(crate) fn handle_pointer_button(state: &mut CompState, pos: Point<f64, Logical>, button: u32, pressed: bool, time: u32) {
    const BTN_LEFT: u32 = 0x110;
    let serial = SERIAL_COUNTER.next_serial();

    // Locked: forward the click to the lock surface (it may have a button or
    // a text field) but never let it focus, raise, drag, or close a window.
    if state.lock.locked {
        if let Some(pointer) = state.seat.get_pointer() {
            let button_state = if pressed { BackendButtonState::Pressed } else { BackendButtonState::Released };
            pointer.button(state, &ButtonEvent { serial, time, button, state: button_state });
        }
        return;
    }

    if pressed && button == BTN_LEFT {
        let layer_hit = layer_surface_under(state, pos);
        if let Some((surface, _)) = &layer_hit {
            // Look the surface up on whichever output actually holds it.
            let on_demand = state
                .outputs()
                .find_map(|output| {
                    layer_map_for_output(output)
                        .layer_for_surface(surface, WindowSurfaceType::ALL)
                        .map(|l| {
                            l.can_receive_keyboard_focus()
                                && l.cached_state().keyboard_interactivity != KeyboardInteractivity::Exclusive
                        })
                })
                .unwrap_or(false);
            // `Exclusive` layers (lock screens, exclusive launchers) already
            // hold focus from `ensure_layer_initial_configure` and keep it
            // regardless of where else is clicked; only `OnDemand` layers
            // (e.g. a bar's search field) claim it on click.
            if on_demand {
                state.set_keyboard_focus(Some(surface.clone()));
            }
        }
        let hit = if layer_hit.is_some() { None } else { state.wm.borrow().hit_test(pos.x as i32, pos.y as i32) };
        if let Some((id, hit)) = hit {
            focus_window(state, id);
            match hit {
                TitlebarHit::Drag => state.wm.borrow_mut().start_drag(id, pos.x as i32, pos.y as i32),
                TitlebarHit::Close => {
                    if let Some(w) = state.id_to_window.get(&id) {
                        close_dwindow(w);
                    }
                }
                TitlebarHit::Maximize => {
                    state.wm.borrow_mut().toggle_maximize(id);
                    state.sync_geometry(id);
                }
                TitlebarHit::Minimize => state.wm.borrow_mut().minimize_window(id),
                TitlebarHit::Resize(edge) => state.wm.borrow_mut().start_resize(id, edge, pos.x as i32, pos.y as i32),
            }
        } else if layer_hit.is_none() {
            if let Some((window, _loc)) = state.space.element_under(pos) {
                let window = window.clone();
                state.space.raise_element(&window, true);
                if let Some(&id) = dwindow_wl_surface(&window).and_then(|s| state.surface_to_id.get(&s)) {
                    focus_window(state, id);
                }
            }
        }
    } else if !pressed {
        let mut wm = state.wm.borrow_mut();
        if wm.is_dragging() {
            wm.end_drag();
        } else if wm.is_resizing() {
            wm.end_resize();
        }
    }

    if let Some(pointer) = state.seat.get_pointer() {
        let button_state = if pressed { BackendButtonState::Pressed } else { BackendButtonState::Released };
        pointer.button(state, &ButtonEvent { serial, time, button, state: button_state });
    }
}

/// Shared between the winit (nested) and udev (bare-TTY) backends: both
/// deliver keyboard events through smithay's generic `KeyboardKeyEvent`
/// trait, so the precise-keybinding-matching logic (see the module docs)
/// only needs to exist once.
pub(crate) fn handle_keyboard_key_event<B: smithay::backend::input::InputBackend, E: KeyboardKeyEvent<B>>(state: &mut CompState, event: &E) {
    let keycode = event.key_code();
    let key_state = event.state();
    let time = event.time_msec();
    let serial = SERIAL_COUNTER.next_serial();
    let Some(keyboard) = state.seat.get_keyboard() else { return };

    // While the session is locked, every key goes to the lock surface and
    // *nothing* is treated as a WM keybinding. Skipping this would leave the
    // lock trivially bypassable - the config binds spawn commands
    // (`Mod4+Return` opens a terminal), so honouring bindings here would let
    // anyone at a locked screen run arbitrary programs.
    if state.lock.locked {
        keyboard.input::<(), _>(state, keycode, key_state, serial, time, |_, _, _| FilterResult::Forward);
        return;
    }

    let bound_keys = state.bound_keys.clone();
    let matched: Option<(String, Modifiers)> =
        keyboard.input(state, keycode, key_state, serial, time, move |_, mods, handle| {
            let modifiers = core_modifiers_from_xkb(mods);
            match keysym_name_for(handle) {
                Some(name) if bound_keys.contains(&srdwm_core::key_combo_string(modifiers, &name)) => {
                    FilterResult::Intercept((name, modifiers))
                }
                _ => FilterResult::Forward,
            }
        });

    if key_state == BackendKeyState::Pressed {
        if let Some((key_name, modifiers)) = matched {
            state.pending.borrow_mut().push(CoreEvent::KeyPress { key_name, modifiers });
        }
    }
    // Unmatched keys were already forwarded to the focused client by
    // `FilterResult::Forward` inside the closure above.
}

/// Translates the effective xkb keysym for this keypress into the same
/// `"Return"`/`"a"`/`"F5"`-style name `srdwm_core::keysyms` uses, so a
/// binding written once in Lua resolves identically on X11 and Wayland.
pub(crate) fn keysym_name_for(handle: smithay::input::keyboard::KeysymHandle<'_>) -> Option<String> {
    srdwm_core::keysyms::keysym_to_name(handle.modified_sym().raw())
}

pub(crate) fn core_modifiers_from_xkb(mods: &smithay::input::keyboard::ModifiersState) -> Modifiers {
    let mut m = Modifiers::empty();
    if mods.shift {
        m |= Modifiers::SHIFT;
    }
    if mods.ctrl {
        m |= Modifiers::CTRL;
    }
    if mods.alt {
        m |= Modifiers::ALT;
    }
    if mods.logo {
        m |= Modifiers::SUPER;
    }
    m
}
