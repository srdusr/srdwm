use super::*;

fn mode_refresh_mhz(mode: &DrmMode) -> i32 {
    let vrefresh = mode.vrefresh();
    if vrefresh > 0 {
        vrefresh as i32 * 1000
    } else {
        60_000
    }
}

/// Brings one connector up: allocates its scanout buffers, sets the mode,
/// and creates the `wl_output` global. Shared by startup and hotplug so a
/// monitor plugged in later is set up exactly like one present at boot.
pub(crate) fn bring_up_head(
    card: &Card,
    dh: &DisplayHandle,
    probe: &ConnectorProbe,
    crtc: crtc::Handle,
    x_offset: i32,
) -> PlatformResult<(UdevHead, crate::state::OutputEntry)> {
    let (width, height) = probe.mode.size();
    let (width, height) = (width as i32, height as i32);

    let buffers = [make_drm_buffer(card, width, height)?, make_drm_buffer(card, width, height)?];
    card.set_crtc(crtc, Some(buffers[0].fb), (0, 0), &[probe.connector], Some(probe.mode)).map_err(err)?;

    // Named after the real connector (eDP-1, HDMI-A-1, ...) so clients and
    // the user can tell monitors apart; `wl_output.name` is what a bar's
    // per-monitor config keys off.
    //
    // Physical size in millimeters comes straight from EDID via the
    // connector, not the hardcoded (0, 0) this used to be - some clients
    // compute their own effective DPI from it (independently of the
    // compositor's own scale factor, which srdwm always reports as 1), so
    // reporting "no physical size at all" was live, wrong data reaching
    // every client, not just an unfilled-in placeholder.
    let (phys_w, phys_h) = probe.info.size().unwrap_or((0, 0));
    let physical_mm = (phys_w as i32, phys_h as i32);
    let output = Output::new(
        probe.name.clone(),
        PhysicalProperties { size: physical_mm.into(), subpixel: Subpixel::Unknown, make: "srdwm".into(), model: "drm".into() },
    );
    let mode = OutputMode { size: (width, height).into(), refresh: mode_refresh_mhz(&probe.mode) };
    output.change_current_state(Some(mode), Some(Transform::Normal), None, Some((x_offset, 0).into()));
    output.set_preferred(mode);
    let global = output.create_global::<CompState>(dh);

    let location: Point<i32, Logical> = (x_offset, 0).into();
    let head = UdevHead {
        crtc,
        connector: probe.connector,
        output: output.clone(),
        global,
        damage_tracker: OutputDamageTracker::from_output(&output),
        buffers,
        front: 0,
        flip_pending: false,
        ages: [0, 0],
        location,
        size: (width, height),
        flip_retry_after: None,
    };
    Ok((head, crate::state::OutputEntry { output, location }))
}

/// A connected connector and the mode we intend to drive it at. CRTC
/// assignment is deliberately separate ([`pick_crtc`]) so a hotplug re-probe
/// can leave surviving heads on the CRTCs they already hold.
pub(crate) struct ConnectorProbe {
    pub(crate) connector: connector::Handle,
    pub(crate) info: connector::Info,
    pub(crate) mode: DrmMode,
    /// Connector name as the kernel reports it (`eDP-1`, `HDMI-A-1`, ...).
    pub(crate) name: String,
}

/// Every connector currently reporting `Connected`, with its preferred mode.
///
/// Forces a fresh probe (`get_connector(.., true)`) rather than trusting
/// cached state - on a hotplug the cached status is exactly what has gone
/// stale.
pub(crate) fn probe_connected(card: &Card) -> PlatformResult<Vec<ConnectorProbe>> {
    let res = card.resource_handles().map_err(err)?;
    let mut probes = Vec::new();
    for handle in res.connectors() {
        let Ok(info) = card.get_connector(*handle, true) else { continue };
        if info.state() != connector::State::Connected {
            continue;
        }
        let name = format!("{:?}-{}", info.interface(), info.interface_id());
        // Prefer the mode the display advertises as PREFERRED (its native
        // resolution) rather than whatever happens to be listed first --
        // the list order is not guaranteed, and picking wrong means running
        // a monitor at the wrong resolution.
        let Some(&mode) = info
            .modes()
            .iter()
            .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| info.modes().first())
        else {
            log::warn!("udev: connector {name} is connected but reports no modes; skipping");
            continue;
        };
        probes.push(ConnectorProbe { connector: *handle, info, mode, name });
    }
    Ok(probes)
}

/// Picks a CRTC for `probe` that is not in `used`.
///
/// CRTCs are a finite hardware resource and cannot be shared, so a machine
/// with more connected monitors than CRTCs drives as many as the hardware
/// allows and logs the rest rather than failing outright.
pub(crate) fn pick_crtc(card: &Card, probe: &ConnectorProbe, used: &[crtc::Handle]) -> Option<crtc::Handle> {
    let res = card.resource_handles().ok()?;
    // Prefer the CRTC already driving this connector, else any free one the
    // encoder can reach, else anything free at all.
    probe
        .info
        .current_encoder()
        .and_then(|enc| card.get_encoder(enc).ok())
        .map(|enc| res.filter_crtcs(enc.possible_crtcs()))
        .unwrap_or_default()
        .into_iter()
        .chain(res.crtcs().iter().copied())
        .find(|c| !used.contains(c))
}

fn make_drm_buffer(card: &Card, width: i32, height: i32) -> PlatformResult<DrmBuffer> {
    let dumb = card.create_dumb_buffer((width as u32, height as u32), DrmFourcc::Xrgb8888, 32).map_err(err)?;
    let fb = card.add_framebuffer(&dumb, 24, 32).map_err(err)?;
    let format = FormatCode::try_from(DrmFourcc::Xrgb8888).map_err(|_| PlatformError::Other("udev: unsupported pixel format".into()))?;
    let image = Image::new(format, width as usize, height as usize, true).map_err(|_| PlatformError::Other("udev: failed to allocate render buffer".into()))?;
    Ok(DrmBuffer { dumb, fb, image })
}

