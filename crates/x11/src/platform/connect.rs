use super::*;
impl X11Platform {
    pub fn connect(wm: Rc<RefCell<WindowManager>>) -> PlatformResult<Self> {
        let (conn, screen_num) = RustConnection::connect(None).map_err(|e| PlatformError::ConnectionFailed(e.to_string()))?;
        let root = conn.setup().roots[screen_num].root;

        // Registering for SUBSTRUCTURE_REDIRECT is how X tells us "no other
        // WM may do this" - if one already has it, this request comes back
        // as a checked BadAccess. This replaces the legacy check, which
        // always returned true because its error handler discarded errors.
        let aux = ChangeWindowAttributesAux::new()
            .event_mask(EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE);
        conn.change_window_attributes(root, &aux).map_err(err)?.check().map_err(|_| PlatformError::AnotherWmRunning)?;

        let atoms = Atoms::new(&conn).map_err(err)?.reply().map_err(err)?;
        conn.change_property32(x11rb::protocol::xproto::PropMode::REPLACE, root, atoms._NET_SUPPORTED, x11rb::protocol::xproto::AtomEnum::ATOM, &[
            atoms._NET_WM_STATE,
            atoms._NET_WM_STATE_MAXIMIZED_VERT,
            atoms._NET_WM_STATE_MAXIMIZED_HORZ,
            atoms._NET_CLIENT_LIST,
            atoms._NET_ACTIVE_WINDOW,
            atoms._NET_WM_STRUT,
            atoms._NET_WM_STRUT_PARTIAL,
        ]).map_err(err)?;

        let font = conn.generate_id().map_err(err)?;
        conn.open_font(font, b"fixed").map_err(err)?;

        let gc = conn.generate_id().map_err(err)?;
        let gc_aux = CreateGCAux::new().font(font).graphics_exposures(0);
        conn.create_gc(gc, root, &gc_aux).map_err(err)?;

        let setup = conn.setup();
        let (min_keycode, max_keycode) = (setup.min_keycode, setup.max_keycode);
        let mapping = conn
            .get_keyboard_mapping(min_keycode, max_keycode - min_keycode + 1)
            .map_err(err)?
            .reply()
            .map_err(err)?;
        let keysyms_per_keycode = mapping.keysyms_per_keycode;
        let keyboard_mapping = mapping.keysyms;

        // Num Lock's modifier bit is not fixed by the X11 spec (unlike Caps
        // Lock, which is always `ModMask::LOCK`) - it's whichever of
        // Mod1..Mod5 the server happens to have bound it to, keyboard- and
        // OS-dependent. Found the same way every other X11 WM does: look up
        // Num Lock's keycode (keysym `0xff7f`, XK_Num_Lock) in the keyboard
        // mapping just queried above, then find which modifier slot's
        // keycode list contains it. See `grab_keybindings`'s doc comment
        // for why this is needed at all.
        let numlock_mask = {
            const XK_NUM_LOCK: u32 = 0xff7f;
            let numlock_keycode = (min_keycode..=max_keycode).find(|&kc| {
                let idx = (kc - min_keycode) as usize * keysyms_per_keycode as usize;
                keyboard_mapping.get(idx).copied() == Some(XK_NUM_LOCK)
            });
            match numlock_keycode {
                Some(kc) => {
                    let modmap = conn.get_modifier_mapping().map_err(err)?.reply().map_err(err)?;
                    let per = modmap.keycodes_per_modifier() as usize;
                    modmask_for_keycode_in_mod_slots(kc, per, &modmap.keycodes)
                }
                None => ModMask::from(0u16),
            }
        };

        conn.flush().map_err(err)?;

        // Same socket name convention as the Wayland backends
        // (`srdwm-<display>.sock`) - there, `<display>` is the Wayland
        // socket's own name; here, the only display identity X11 has is
        // `$DISPLAY` itself (e.g. `:0`), which is exactly what every X
        // client - including a nested Xephyr/Xnest session used for
        // testing - already keys off to tell one server from another.
        let display_name = std::env::var("DISPLAY").unwrap_or_else(|_| "x11".to_string());
        let ipc = match srdwm_platform::IpcServer::bind(&display_name) {
            Ok(ipc) => Some(ipc),
            Err(e) => {
                log::warn!("failed to bind srd IPC socket for display '{display_name}': {e}");
                None
            }
        };

        Ok(Self {
            conn,
            root,
            atoms,
            gc,
            font,
            wm,
            xid_to_core: HashMap::new(),
            frames: HashMap::new(),
            min_keycode,
            max_keycode,
            keysyms_per_keycode,
            keyboard_mapping,
            numlock_mask,
            ipc,
            appmenu_registrar: Some(srdwm_platform::AppmenuRegistrarState::new()),
            struts: HashMap::new(),
        })
    }

    pub(super) fn keycode_to_keysym(&self, keycode: u8) -> u32 {
        if keycode < self.min_keycode || keycode > self.max_keycode || self.keysyms_per_keycode == 0 {
            return 0;
        }
        let idx = (keycode - self.min_keycode) as usize * self.keysyms_per_keycode as usize;
        self.keyboard_mapping.get(idx).copied().unwrap_or(0)
    }

    fn keysym_to_keycode(&self, keysym: u32) -> Option<u8> {
        for kc in self.min_keycode..=self.max_keycode {
            let idx = (kc - self.min_keycode) as usize * self.keysyms_per_keycode as usize;
            if self.keyboard_mapping.get(idx).copied() == Some(keysym) {
                return Some(kc);
            }
        }
        None
    }

    pub(super) fn modifiers_from_state(state: u16) -> Modifiers {
        let mut m = Modifiers::empty();
        if state & ModMask::SHIFT.bits() != 0 {
            m |= Modifiers::SHIFT;
        }
        if state & ModMask::CONTROL.bits() != 0 {
            m |= Modifiers::CTRL;
        }
        if state & ModMask::M1.bits() != 0 {
            m |= Modifiers::ALT;
        }
        if state & ModMask::M4.bits() != 0 {
            m |= Modifiers::SUPER;
        }
        m
    }

    fn modmask_for(modifiers: Modifiers) -> ModMask {
        let mut mask = ModMask::from(0u16);
        if modifiers.contains(Modifiers::SHIFT) {
            mask |= ModMask::SHIFT;
        }
        if modifiers.contains(Modifiers::CTRL) {
            mask |= ModMask::CONTROL;
        }
        if modifiers.contains(Modifiers::ALT) {
            mask |= ModMask::M1;
        }
        if modifiers.contains(Modifiers::SUPER) {
            mask |= ModMask::M4;
        }
        mask
    }

    /// Grabs the given `"Mod4+Shift+Return"`-style key combos on the root
    /// window so their KeyPress events reach us even when a client has
    /// input focus. Call after loading config (once bindings are known).
    ///
    /// A `KeyPress`'s modifier state includes whichever lock modifiers
    /// happen to be toggled on (Num Lock, Caps Lock) in addition to
    /// whatever the binding actually asked for - `XGrabKey` matches state
    /// *exactly*, not as a subset, so a grab registered only for e.g.
    /// `Mod4` never fires the moment Num Lock is on, since the real event's
    /// state is `Mod4 | numlock_mask` instead. Every real X11 WM (i3,
    /// bspwm, dwm) grabs each binding once per combination of the lock
    /// modifiers for exactly this reason; this one previously didn't,
    /// which meant every keybinding silently stopped firing the instant
    /// Num Lock was toggled on - not a missing feature, a basic X11
    /// correctness requirement that was simply never implemented.
    pub fn grab_keybindings(&mut self, combos: &[String]) -> PlatformResult<()> {
        // The four combinations of "Num Lock toggled or not" x "Caps Lock
        // toggled or not" - Scroll Lock is deliberately not covered here,
        // matching the convention every WM referenced above also follows
        // (rarely present on modern keyboards, rarely toggled when it is).
        let lock_variants = [ModMask::from(0u16), self.numlock_mask, ModMask::LOCK, self.numlock_mask | ModMask::LOCK];
        for combo in combos {
            let Some((modifiers, key_name)) = srdwm_core::parse_key_combo(combo) else { continue };
            let Some(keysym) = keysyms::name_to_keysym(key_name) else {
                log::warn!("cannot grab '{combo}': unknown key name '{key_name}'");
                continue;
            };
            let Some(keycode) = self.keysym_to_keycode(keysym) else {
                log::warn!("cannot grab '{combo}': no keycode for keysym {keysym:#x}");
                continue;
            };
            let mask = Self::modmask_for(modifiers);
            for lock in lock_variants {
                self.conn
                    .grab_key(true, self.root, mask | lock, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
                    .map_err(err)?;
            }
        }
        self.conn.flush().map_err(err)?;
        Ok(())
    }
}
