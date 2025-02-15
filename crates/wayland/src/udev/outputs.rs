use super::*;
use super::drm::{bring_up_head, pick_crtc, probe_connected};

impl CompState {
    /// Applies whatever monitor layout `monitor_layout::load()` remembers
    /// from a previous run, on top of the default left-to-right layout
    /// every head was just brought up with. Call once, right after every
    /// head exists but before the Wayland socket is bound - see the call
    /// site in `platform.rs`'s `connect()` for why that ordering is the
    /// entire point (no client, panel or otherwise, gets a chance to see
    /// the un-restored arrangement, not even for one frame).
    ///
    /// A connector with no remembered entry (a monitor plugged in for the
    /// first time, or a fresh install with no state file yet) is left
    /// exactly where the default layout put it - this only ever narrows
    /// toward a remembered position, never invents one.
    pub(crate) fn restore_monitor_layout(&mut self) {
        let remembered = crate::monitor_layout::load();
        if remembered.is_empty() {
            return;
        }
        // Disables first, deliberately: `disable_connector_by_name` ends
        // with its own `relayout_outputs()` call, which recomputes every
        // *remaining* head's position from the default left-to-right
        // layout - doing that after a position restore below would just
        // overwrite it again. Processing every disable up front means
        // that default re-layout has already happened, once, before any
        // remembered position gets applied on top of it.
        for (name, entry) in &remembered {
            if !entry.enabled {
                self.disable_connector_by_name(name);
            }
        }
        for (name, entry) in &remembered {
            if !entry.enabled {
                continue;
            }
            let Some(output) = self.udev.as_ref().and_then(|u| u.heads.iter().find(|h| &h.output.name() == name)).map(|h| h.output.clone()) else {
                continue;
            };
            crate::output_management::apply_output_position(self, &output, (entry.x, entry.y).into());
        }
    }

    /// Re-probes connectors after a hotplug and reconciles the head list.
    ///
    /// Connectors that vanished have their head torn down (global removed,
    /// output unmapped, DRM buffers freed); newly connected ones are brought
    /// up exactly as they would have been at startup. Every head is then
    /// repositioned left-to-right, because removing a monitor shifts the
    /// ones after it.
    pub(crate) fn reprobe_outputs(&mut self) {
        let Some(udev) = self.udev.as_ref() else { return };
        let card = udev.card.clone();

        let probes = match probe_connected(&card) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("udev: hotplug re-probe failed: {e}");
                return;
            }
        };
        let present: Vec<connector::Handle> = probes.iter().map(|p| p.connector).collect();
        let existing: Vec<connector::Handle> = udev.heads.iter().map(|h| h.connector).collect();

        // A disabled connector that's genuinely gone from this fresh probe
        // was actually unplugged, not just left administratively off --
        // checked (and cleaned up) *before* the `gone.is_empty() &&
        // added.is_empty()` early-out just below, since a disabled
        // connector was never in `existing`/`heads` to begin with and so
        // never affects either of those on its own; without this check
        // running first, that early-out would fire and this cleanup would
        // simply never happen for a hotplug event this narrow. See
        // `MonitorInfo::enabled`'s own doc comment for why "off" and "not
        // connected" have to be reported differently - this is what
        // actually makes that transition happen.
        let present_names: Vec<&str> = probes.iter().map(|p| p.name.as_str()).collect();
        let unplugged_while_disabled: Vec<String> = udev.disabled_connectors.iter().filter(|name| !present_names.contains(&name.as_str())).cloned().collect();
        if !unplugged_while_disabled.is_empty() {
            let mut wm = self.wm.borrow_mut();
            for name in &unplugged_while_disabled {
                log::info!("udev: administratively-disabled output {name} was physically unplugged");
                wm.clear_disabled_monitor(name);
            }
        }

        let gone: Vec<connector::Handle> = existing.iter().copied().filter(|c| !present.contains(c)).collect();
        let added: Vec<usize> = probes
            .iter()
            .enumerate()
            // `!udev.disabled_connectors.contains(&p.name)`: without this,
            // an administratively-disabled-but-still-connected output
            // (`disable_connector_by_name`) looks identical to a genuinely
            // new one here - present in a fresh probe, absent from
            // `heads` - and this *unrelated* hotplug event (any
            // connector, not just the disabled one) would bring it
            // straight back up.
            .filter(|(_, p)| !existing.contains(&p.connector) && !udev.disabled_connectors.contains(&p.name))
            .map(|(i, _)| i)
            .collect();
        // `udev` (the outer immutable borrow) is done being read after
        // this point, so `disabled_connectors` can be mutated now to drop
        // whatever `unplugged_while_disabled` found - deferred this far
        // specifically because the `added` filter just above still needed
        // to read it first.
        if !unplugged_while_disabled.is_empty() {
            if let Some(udev) = self.udev.as_mut() {
                udev.disabled_connectors.retain(|name| !unplugged_while_disabled.contains(name));
            }
        }
        if gone.is_empty() && added.is_empty() && unplugged_while_disabled.is_empty() {
            return; // a "changed" event that didn't change the connector set
        }
        log::info!(
            "udev: hotplug - {} output(s) removed, {} added, {} disabled-and-unplugged",
            gone.len(),
            added.len(),
            unplugged_while_disabled.len()
        );

        // ---- removals ----
        for connector in &gone {
            let Some(udev) = self.udev.as_mut() else { return };
            let Some(index) = udev.heads.iter().position(|h| h.connector == *connector) else { continue };
            let head = udev.heads.remove(index);
            log::info!("udev: output {} disconnected", head.output.name());
            self.dh.remove_global::<CompState>(head.global.clone());
            self.space.unmap_output(&head.output);
            self.outputs.retain(|e| e.output != head.output);
            // A lock surface for a monitor that no longer exists would keep
            // `confirm_lock_if_presented` waiting forever otherwise.
            self.lock.surfaces.remove(&head.output.name());
            self.lock.presented.remove(&head.output.name());
            head.release(&card);
            self.pending.borrow_mut().push(CoreEvent::MonitorRemoved(index as u32));
        }

        // ---- additions ----
        for i in added {
            let probe = &probes[i];
            let used: Vec<crtc::Handle> =
                self.udev.as_ref().map(|u| u.heads.iter().map(|h| h.crtc).collect()).unwrap_or_default();
            let Some(crtc) = pick_crtc(&card, probe, &used) else {
                log::warn!("udev: no free CRTC for newly connected {}; not driving it", probe.name);
                continue;
            };
            // Placed at 0 for now; the re-layout below assigns real offsets.
            let scale = self.wm.borrow().monitor_scale(&probe.name);
            match bring_up_head(&card, &self.dh.clone(), probe, crtc, 0, 0, scale) {
                Ok((head, entry)) => {
                    log::info!("udev: output {} connected ({}x{})", probe.name, head.size.0, head.size.1);
                    let monitor_id = self.outputs.len() as u32;
                    let geometry = srdwm_core::Rect::new(0, 0, head.size.0 as u32, head.size.1 as u32);
                    if let Some(udev) = self.udev.as_mut() {
                        udev.heads.push(head);
                    }
                    self.outputs.push(entry);
                    self.pending
                        .borrow_mut()
                        .push(CoreEvent::MonitorAdded(srdwm_core::Monitor::new(monitor_id, probe.name.clone(), geometry)));
                }
                Err(e) => log::warn!("udev: failed to bring up {}: {e}", probe.name),
            }
        }

        // Safety net: never leave the session with zero live outputs.
        // Real scenario, flagged live before it could actually happen:
        // administratively disable the internal/laptop panel (`srd
        // dispatch set output enabled ... false`), then physically unplug
        // the one remaining external monitor - this same hotplug path
        // handles the unplug correctly (the external head is removed
        // above, same as any other disconnect), but without this, the
        // internal panel stays administratively disabled forever after,
        // leaving genuinely nothing to drive at all: no picture, and (a
        // laptop having no other input device to fix it from) no way back
        // in short of a restart. Re-enabling the most recently disabled
        // connector that's still physically present - exactly
        // `enable_connector_by_name`'s own normal path, just triggered by
        // "we're about to have nothing" instead of an explicit request --
        // trades the administrative disable for actually having a screen,
        // which is the only reasonable choice once the alternative is a
        // fully dark machine.
        let no_live_heads = self.udev.as_ref().is_some_and(|u| u.heads.is_empty());
        if no_live_heads {
            let candidates: Vec<&drm::ConnectorProbe> =
                self.udev.as_ref().map(|u| probes.iter().filter(|p| u.disabled_connectors.contains(&p.name)).collect()).unwrap_or_default();
            // The internal/laptop panel specifically, if it's one of the
            // candidates - `eDP`/`LVDS`/`DSI` are the real DRM connector-
            // type prefixes an embedded display reports as, matching the
            // exact scenario this exists for (disable the internal panel,
            // then lose the external one it was standing in for). Falls
            // back to whatever else is available rather than doing
            // nothing, on the same "a screen is better than no screen"
            // reasoning - an external monitor left administratively
            // disabled is still a better fallback than a fully dark
            // machine, even if it wasn't the specific one this was
            // written for.
            let fallback = candidates
                .iter()
                .find(|p| p.name.starts_with("eDP") || p.name.starts_with("LVDS") || p.name.starts_with("DSI"))
                .or_else(|| candidates.first())
                .map(|p| p.name.clone());
            if let Some(name) = fallback {
                log::warn!("udev: every output would otherwise be off - re-enabling {name} rather than leaving nothing to drive");
                self.enable_connector_by_name(&name);
                return;
            }
        }

        self.relayout_outputs();
    }

    /// Administratively disables the output named `name` - the backend
    /// half of `srd dispatch set output enabled <name> false`. Reuses
    /// exactly the same removal steps `reprobe_outputs` already takes for
    /// a real unplug just above (destroy the `wl_output` global, unmap
    /// from `Space`, drop lock-surface tracking, free the DRM buffers via
    /// `head.release`, rehome its windows via a `MonitorRemoved` event) --
    /// the only difference is remembering the connector's *name*
    /// afterward, in `UdevState::disabled_connectors`, so `reprobe_
    /// outputs` won't bring it straight back on the next unrelated
    /// hotplug, and so `enable_connector_by_name` can find it again later
    /// without a real replug.
    pub(crate) fn disable_connector_by_name(&mut self, name: &str) {
        let Some(udev) = self.udev.as_mut() else { return };
        let card = udev.card.clone();
        let Some(index) = udev.heads.iter().position(|h| h.output.name() == name) else {
            log::warn!("udev: set output enabled false: no connected output named {name}");
            return;
        };
        // Snapshotted before removal, same computation `Platform::
        // monitors()` itself uses - see `WindowManager::
        // set_disabled_monitor`'s own doc comment for why `srd monitors`
        // still wants this after the head is gone (a last-known rect to
        // show, not a live one).
        let head_ref = &udev.heads[index];
        let zone = layer_map_for_output(&head_ref.output).non_exclusive_zone();
        // `zone` is logical (scale-divided), `head_ref.location`/`size` are
        // raw physical pixels - same unit mismatch `Platform::monitors()`
        // itself had to be fixed for, and the same fix: scale `zone` back
        // into physical pixels before combining. See that function's own
        // doc comment for the live symptom this caused when left
        // unconverted (a scaled output's reported geometry overlapping its
        // neighbor's).
        let scale = head_ref.output.current_scale().fractional_scale();
        let zone_physical = |v: i32| (v as f64 * scale).round() as i32;
        let usable_geometry = srdwm_core::Rect::new(
            head_ref.location.x + zone_physical(zone.loc.x),
            head_ref.location.y + zone_physical(zone.loc.y),
            zone_physical(zone.size.w).max(0) as u32,
            zone_physical(zone.size.h).max(0) as u32,
        );
        let full_geometry = srdwm_core::Rect::new(head_ref.location.x, head_ref.location.y, head_ref.size.0 as u32, head_ref.size.1 as u32);
        let was_primary = index == 0;
        let head = udev.heads.remove(index);
        log::info!("udev: output {name} administratively disabled");
        self.dh.remove_global::<CompState>(head.global.clone());
        self.space.unmap_output(&head.output);
        self.outputs.retain(|e| e.output != head.output);
        self.lock.surfaces.remove(&head.output.name());
        self.lock.presented.remove(&head.output.name());
        head.release(&card);
        self.pending.borrow_mut().push(CoreEvent::MonitorRemoved(index as u32));
        if let Some(udev) = self.udev.as_mut() {
            udev.disabled_connectors.insert(name.to_string());
        }
        self.wm.borrow_mut().set_disabled_monitor(name.to_string(), usable_geometry, full_geometry, was_primary);
        self.relayout_outputs();
        // Last-known physical position kept alongside `enabled: false` --
        // re-enabling this same connector later (`enable_connector_by_name`
        // below) restores it, rather than a disable silently discarding
        // where it used to be.
        crate::monitor_layout::save_output(name, crate::monitor_layout::PersistedOutput { x: full_geometry.x, y: full_geometry.y, enabled: false });
    }

    /// The other half of `disable_connector_by_name` - brings a
    /// previously-disabled-but-still-connected output back up exactly the
    /// way `reprobe_outputs` brings up a genuinely new one, since nothing
    /// about the underlying hardware actually changed in between (the
    /// connector was never really unplugged, just not driven).
    pub(crate) fn enable_connector_by_name(&mut self, name: &str) {
        let Some(udev) = self.udev.as_ref() else { return };
        let card = udev.card.clone();
        if !udev.disabled_connectors.contains(name) {
            log::warn!("udev: set output enabled true: {name} isn't administratively disabled (already on, or never connected)");
            return;
        }
        let probes = match probe_connected(&card) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("udev: re-enable probe for {name} failed: {e}");
                return;
            }
        };
        let Some(probe) = probes.iter().find(|p| p.name == name) else {
            log::warn!("udev: set output enabled true: {name} is no longer physically connected");
            if let Some(udev) = self.udev.as_mut() {
                udev.disabled_connectors.remove(name);
            }
            // "Off" and "not connected" have to read differently to a
            // listener (see `MonitorInfo::enabled`'s own doc comment) --
            // this output is now the latter, so it stops being listed at
            // all, same as a genuine unplug always has.
            self.wm.borrow_mut().clear_disabled_monitor(name);
            return;
        };
        let used: Vec<crtc::Handle> = udev.heads.iter().map(|h| h.crtc).collect();
        let Some(crtc) = pick_crtc(&card, probe, &used) else {
            log::warn!("udev: no free CRTC to re-enable {name}");
            return;
        };
        // Placed at 0 for now; `relayout_outputs` below assigns real
        // offsets, same as a genuine hotplug addition.
        let scale = self.wm.borrow().monitor_scale(name);
        match bring_up_head(&card, &self.dh.clone(), probe, crtc, 0, 0, scale) {
            Ok((head, entry)) => {
                log::info!("udev: output {name} re-enabled ({}x{})", head.size.0, head.size.1);
                let monitor_id = self.outputs.len() as u32;
                let geometry = srdwm_core::Rect::new(0, 0, head.size.0 as u32, head.size.1 as u32);
                if let Some(udev) = self.udev.as_mut() {
                    udev.heads.push(head);
                    udev.disabled_connectors.remove(name);
                }
                self.outputs.push(entry);
                self.pending.borrow_mut().push(CoreEvent::MonitorAdded(srdwm_core::Monitor::new(monitor_id, name.to_string(), geometry)));
                // It's live again - `monitors()` reports it directly now,
                // so it has no business also showing up in the separate
                // disabled-outputs listing.
                self.wm.borrow_mut().clear_disabled_monitor(name);
            }
            Err(e) => log::warn!("udev: failed to re-enable {name}: {e}"),
        }
        self.relayout_outputs();
        // Read back after `relayout_outputs` has assigned this head its
        // real position, not the `(0, 0)` placeholder it was brought up
        // at above.
        if let Some(location) = self.udev.as_ref().and_then(|u| u.heads.iter().find(|h| h.output.name() == name)).map(|h| h.location) {
            crate::monitor_layout::save_output(name, crate::monitor_layout::PersistedOutput { x: location.x, y: location.y, enabled: true });
        }
    }

    /// Repositions every head left-to-right and republishes the new
    /// positions to the output globals, the `Space`, and the layer maps.
    fn relayout_outputs(&mut self) {
        let Some(udev) = self.udev.as_mut() else { return };
        // Two separate accumulators, not one - `x_physical` is this
        // compositor's own internal placement convention (`head.location`,
        // `Space`, everything else), `x_logical` is what actually goes out
        // over the wire via `change_current_state`, which the Wayland
        // protocol always specifies in logical points. At `scale == 1.0`
        // for every output these are numerically identical, which is why
        // this was invisible until a non-1.0 scale existed: passing the
        // *physical* offset straight into `change_current_state` here
        // (this used to do exactly that, unconditionally) put a second
        // output's *logical* position short of where the first output's
        // own *logical* width actually ends whenever a scale below 1.0 was
        // involved - e.g. a first output that's 1920 physical but 2276
        // logical (0.843 scale) left the second output advertised at
        // logical x=1920, deep inside the first one's own logical extent,
        // not past it. Reported live (measured from inside GTK, not
        // inferred) as the two outputs' logical rectangles overlapping by
        // a few hundred pixels - ambiguous "which monitor is this point
        // on" answers, and hit-testing/screenshots landing on the wrong
        // output entirely in the overlap band.
        let mut x_physical = 0;
        let mut x_logical = 0;
        let mut placed: Vec<(Output, Point<i32, Logical>)> = Vec::new();
        for head in &mut udev.heads {
            let scale = head.output.current_scale().fractional_scale();
            head.location = (x_physical, 0).into();
            head.output.change_current_state(None, None, None, Some((x_logical, 0).into()));
            placed.push((head.output.clone(), head.location));
            x_physical += head.size.0;
            x_logical += (head.size.0 as f64 / scale).round() as i32;
        }
        for (output, location) in placed {
            if let Some(entry) = self.outputs.iter_mut().find(|e| e.output == output) {
                entry.location = location;
            }
            self.space.map_output(&output, (location.x, location.y));
            // Bars are anchored to their output, so their geometry has to be
            // recomputed against the moved output rectangle.
            layer_map_for_output(&output).arrange();
        }
    }
}

