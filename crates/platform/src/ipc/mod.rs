//! A tiny local control socket, in the spirit of `hyprctl`/`swaymsg`, so
//! external scripts can query and drive window state without speaking
//! Wayland themselves. Bound at `$XDG_RUNTIME_DIR/srdwm-<display>.sock`.
//! `crates/ctl` (the `srd` binary) is the reference client.
//!
//! Deliberately synchronous and non-blocking-polled from each backend's
//! `poll_events()` tick (see `udev`/`winit`), the same way the
//! Wayland client socket itself is accepted - there is no calloop event
//! loop shared by both backends (`winit` has none at all), so this
//! avoids needing two different registration mechanisms for one feature.
//! An ordinary request is one request/one response/close; nothing here is
//! held open for those, so a stalled or hostile client can only ever leak
//! one never-completed connection object, not block the compositor.
//!
//! `{"cmd":"subscribe"}` is the one exception: instead of closing after its
//! reply, that connection is kept open and pushed a fresh `clients` event
//! every time the window list actually changes, so a dock/panel doesn't
//! have to re-poll `clients` on a timer and diff it itself to notice
//! anything - the single highest-leverage gap found comparing srdwm
//! against sway/i3/Hyprland/bspwm's own IPCs, all of which have an
//! event-subscribe side already. A peer session building an AGS dock hit
//! exactly this wall (see `docs/IMPLEMENTATION_STATUS.md`): with no way to
//! be told about changes, it had to poll `wlr-foreign-toplevel` from a
//! separate Python helper instead of using this socket at all.

//! Split into `types.rs` (response/event payload structs and the
//! snapshot functions that build them), `dispatch.rs` (`handle_request`/
//! `handle_set`, every real `cmd`), and `tests.rs` - this file now
//! keeps only `IpcServer` itself (the socket/connection lifecycle and the
//! subscribe-broadcast poll loop). Purely a by-concern split of what was
//! one ~1900-line file; no behavior changed by it.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use srdwm_core::WindowManager;

mod dispatch;
mod types;
#[cfg(test)]
mod tests;

pub(crate) use dispatch::handle_request;
pub(crate) use types::*;

pub struct IpcServer {
    listener: UnixListener,
    path: PathBuf,
    conns: Vec<(UnixStream, Vec<u8>)>,
    /// Long-lived connections from `{"cmd":"subscribe"}` - write-only after
    /// their initial snapshot, never read from again (a subscriber has no
    /// further requests to send; a client wanting both query and push needs
    /// two connections, matching Hyprland's separate event socket rather
    /// than sway's single multiplexed one, the simpler of the two to keep
    /// this connection loop's one-purpose-per-connection shape intact).
    subscribers: Vec<UnixStream>,
    /// What was last actually sent to subscribers, so a `poll()` tick with
    /// no real change (the common case, since this runs every ~16ms) skips
    /// serializing and writing anything at all.
    last_broadcast: Vec<ClientInfo>,
    /// `last_broadcast`'s workspace equivalent - diffed and pushed
    /// independently, see `WorkspacesEvent`'s doc comment for why this
    /// isn't folded into the field above.
    last_broadcast_workspaces: Vec<WorkspaceInfo>,
    /// `last_broadcast`'s keyboard-layout equivalent - see
    /// `KeyboardLayoutEvent`'s own doc comment.
    last_broadcast_keyboard_layout: String,
    /// `last_broadcast`'s monitor equivalent - see `MonitorsEvent`'s own
    /// doc comment.
    last_broadcast_monitors: Vec<MonitorInfo>,
}

impl IpcServer {
    /// `display_name` is the Wayland socket name (e.g. `wayland-1`) so
    /// concurrent nested/test instances - used throughout this project for
    /// self-testing - don't collide on one path.
    pub fn bind(display_name: &str) -> std::io::Result<Self> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
        Self::bind_in(&dir, display_name)
    }

    /// `bind`'s actual logic, parametrized over the runtime directory --
    /// split out so tests can point this at a `tempfile::tempdir()` instead
    /// of mutating the process-wide `XDG_RUNTIME_DIR` env var (racy under
    /// Rust's default parallel test execution, since every test in this
    /// crate shares one process).
    fn bind_in(dir: &std::path::Path, display_name: &str) -> std::io::Result<Self> {
        let path = dir.join(format!("srdwm-{display_name}.sock"));
        // A stale socket left behind by a crashed/killed previous instance
        // makes `bind` fail with `AddrInUse` even though nothing is
        // listening; a fresh instance always wins over a dead one.
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            path,
            conns: Vec::new(),
            subscribers: Vec::new(),
            last_broadcast: Vec::new(),
            last_broadcast_workspaces: Vec::new(),
            last_broadcast_keyboard_layout: String::new(),
            last_broadcast_monitors: Vec::new(),
        })
    }

    /// Accepts any waiting connections, advances in-progress reads, and
    /// pushes a fresh snapshot to every subscriber if the window list
    /// actually changed since the last one. Returns `true` if a request
    /// mutated window state, so the caller can fold that into its own
    /// dirty/`sync()` decision the same as any other event source.
    pub fn poll(&mut self, wm: &std::rc::Rc<std::cell::RefCell<WindowManager>>) -> bool {
        loop {
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    if stream.set_nonblocking(true).is_ok() {
                        self.conns.push((stream, Vec::new()));
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }

        let mut dirty = false;
        let mut new_subscribers = Vec::new();
        self.conns.retain_mut(|(stream, buf)| {
            let mut chunk = [0u8; 512];
            match stream.read(&mut chunk) {
                Ok(0) => return false,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(_) => return false,
            }
            let Some(nl) = buf.iter().position(|&b| b == b'\n') else {
                // Cap a request that never terminates - a hostile or
                // broken client shouldn't accumulate memory forever.
                return buf.len() < 4096;
            };
            let line = buf[..nl].to_vec();
            let cmd = serde_json::from_slice::<serde_json::Value>(&line).ok().and_then(|v| v.get("cmd").and_then(|c| c.as_str().map(str::to_string)));
            let (response, changed) = handle_request(&line, wm);
            dirty |= changed;
            let mut out = response;
            out.push(b'\n');
            if stream.write_all(&out).is_err() {
                return false;
            }
            if cmd.as_deref() == Some("subscribe") {
                // Handed off to `subscribers` below rather than kept here --
                // this connection is done being read from, only ever
                // written to from now on.
                if let Ok(cloned) = stream.try_clone() {
                    new_subscribers.push(cloned);
                }
                return false;
            }
            // Every other command is still one request/one response/close,
            // same as before subscribe existed.
            false
        });
        // `last_broadcast`/`last_broadcast_workspaces` are kept in sync
        // with reality unconditionally, whether or not anyone is actually
        // subscribed right now - only the socket write itself is gated on
        // `self.subscribers` being non-empty. This matters at the exact
        // moment a new subscriber joins: their `"subscribe"` reply (built
        // separately, inside `handle_request`, from its own fresh
        // `client_snapshot`/`workspace_snapshot` call) already sent them a
        // full current snapshot, but `new_subscribers` hasn't been merged
        // into `self.subscribers` yet at this point in `poll` - so
        // `self.subscribers` may still be empty here even though a reply
        // just went out. Skipping the *sync* as well as the write (an
        // earlier version of this gated both behind one `is_empty` check)
        // left `last_broadcast*` stale until the next real change, so the
        // very next tick's diff saw a mismatch against what the new
        // subscriber was already sent and pushed a redundant duplicate --
        // for clients this coincidentally never fired (an empty window
        // list at construction matches `last_broadcast`'s own empty
        // starting value), but workspaces are never empty (`WindowManager
        // ::new` always seeds one), so every first subscriber got a
        // spurious extra `workspaces` line one tick after connecting.
        // Always syncing, and only conditionally writing, keeps both
        // invariants true at once: a fresh subscriber's direct reply is
        // never redundantly repeated, and an *existing* subscriber still
        // gets notified of any real change that happens in the same tick
        // a new one joins, since the diff against the old subscriber list
        // runs before `new_subscribers` is merged in below regardless.
        let current: Vec<ClientInfo> = client_snapshot(wm);
        if current != self.last_broadcast {
            if !self.subscribers.is_empty() {
                if let Ok(mut out) = serde_json::to_vec(&ClientsEvent { event: "clients", clients: &current }) {
                    out.push(b'\n');
                    self.subscribers.retain_mut(|s| s.write_all(&out).is_ok());
                }
            }
            self.last_broadcast = current;
        }
        let current_workspaces: Vec<WorkspaceInfo> = workspace_snapshot(wm);
        if current_workspaces != self.last_broadcast_workspaces {
            if !self.subscribers.is_empty() {
                if let Ok(mut out) = serde_json::to_vec(&WorkspacesEvent { event: "workspaces", workspaces: &current_workspaces }) {
                    out.push(b'\n');
                    self.subscribers.retain_mut(|s| s.write_all(&out).is_ok());
                }
            }
            self.last_broadcast_workspaces = current_workspaces;
        }
        let current_layout = wm.borrow().keyboard_layout.clone();
        if current_layout != self.last_broadcast_keyboard_layout {
            if !self.subscribers.is_empty() {
                if let Ok(mut out) = serde_json::to_vec(&KeyboardLayoutEvent { event: "keyboard_layout", layout: &current_layout }) {
                    out.push(b'\n');
                    self.subscribers.retain_mut(|s| s.write_all(&out).is_ok());
                }
            }
            self.last_broadcast_keyboard_layout = current_layout;
        }
        let current_monitors = monitor_snapshot(wm);
        if current_monitors != self.last_broadcast_monitors {
            if !self.subscribers.is_empty() {
                if let Ok(mut out) = serde_json::to_vec(&MonitorsEvent { event: "monitors", monitors: &current_monitors }) {
                    out.push(b'\n');
                    self.subscribers.retain_mut(|s| s.write_all(&out).is_ok());
                }
            }
            self.last_broadcast_monitors = current_monitors;
        }
        self.subscribers.extend(new_subscribers);
        dirty
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

