use super::*;
use super::drm::{bring_up_head, pick_crtc, probe_connected};

impl CompState {
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

        let gone: Vec<connector::Handle> = existing.iter().copied().filter(|c| !present.contains(c)).collect();
        let added: Vec<usize> = probes
            .iter()
            .enumerate()
            .filter(|(_, p)| !existing.contains(&p.connector))
            .map(|(i, _)| i)
            .collect();
        if gone.is_empty() && added.is_empty() {
            return; // a "changed" event that didn't change the connector set
        }
        log::info!("udev: hotplug - {} output(s) removed, {} added", gone.len(), added.len());

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
            match bring_up_head(&card, &self.dh.clone(), probe, crtc, 0) {
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

        self.relayout_outputs();
    }

    /// Repositions every head left-to-right and republishes the new
    /// positions to the output globals, the `Space`, and the layer maps.
    fn relayout_outputs(&mut self) {
        let Some(udev) = self.udev.as_mut() else { return };
        let mut x = 0;
        let mut placed: Vec<(Output, Point<i32, Logical>)> = Vec::new();
        for head in &mut udev.heads {
            head.location = (x, 0).into();
            head.output.change_current_state(None, None, None, Some((x, 0).into()));
            placed.push((head.output.clone(), head.location));
            x += head.size.0;
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

