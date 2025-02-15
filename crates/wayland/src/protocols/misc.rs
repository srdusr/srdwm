//! Small protocol handlers whose entire implementation is smithay's own
//! no-op default - the global still has to exist for clients that treat it
//! as mandatory, but there's nothing for this compositor to do in response.

use smithay::wayland::tablet_manager::TabletSeatHandler;

use crate::state::CompState;

impl smithay::wayland::output::OutputHandler for CompState {}

/// `wp_cursor_shape_v1`: lets a client ask for a *named* cursor (text,
/// grab, resize edges, ...) instead of rendering and attaching its own
/// surface. Its requests route straight into `SeatHandler::cursor_image`
/// (see `seat.rs`), same as a client-drawn cursor surface does - no extra
/// state on our side. Without this global at all, a client that only speaks
/// this (increasingly the norm - recent GTK4/Firefox use it for most
/// cursor changes) has no way to tell us the pointer should look like
/// anything but whatever it last was, which reads as the cursor going
/// stale, wrong, or simply disappearing depending on what was showing when
/// the client gave up trying.
///
/// `TabletSeatHandler` is a supertrait bound of this protocol's `Dispatch`
/// impl (cursor-shape covers tablet tools too); srdwm has no tablet
/// support to speak of, so every method is left at its no-op default.
impl TabletSeatHandler for CompState {}

/// Fractional scaling. srdwm runs every output at scale 1, so there is
/// nothing to compute - but the global has to exist, because clients that
/// use it (notably wallpaper daemons) treat it as mandatory.
impl smithay::wayland::fractional_scale::FractionalScaleHandler for CompState {}
