//! Compile-time-generated wire bindings for `gtk_shell1`/`gtk_surface1`
//! (GTK's own private protocol - `protocols/gtk-shell.xml`, vendored from
//! `gdk/wayland/protocol/gtk-shell.xml` in GTK's own source tree). Unlike
//! the wlr/staging protocols this codebase already uses, there is no
//! published `wayland-protocols-*` crate for this one - it isn't part of
//! any standardised protocol umbrella, just GTK's own extension - so it's
//! generated here the same way `wayland-protocols-wlr` generates its own
//! bindings internally (see that crate's `protocol_macro.rs`), just
//! server-side only, since this compositor never needs the client half.
//!
//! `gtk_shell.rs` is the actual `GlobalDispatch`/`Dispatch` handler logic;
//! this module is only the raw generated types it's built against.

#![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
#![allow(non_upper_case_globals, non_snake_case, unused_imports)]
#![allow(missing_docs, clippy::all)]

pub mod server {
    use smithay::reexports::wayland_server;
    use smithay::reexports::wayland_server::protocol::*;

    pub mod __interfaces {
        use smithay::reexports::wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/gtk-shell.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("protocols/gtk-shell.xml");
}
