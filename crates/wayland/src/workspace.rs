//! `ext-workspace-v1`: enumerates srdwm's workspaces (name, active/urgent/
//! hidden state) to any client that binds it, and lets that client request
//! activation - the workspace pill/switcher half of a dock, requested
//! alongside `foreign_toplevel.rs` (see `docs/PANEL_SUPPORT_TODO.md`'s P1
//! list). smithay 0.7 has no built-in helper for this protocol either;
//! hand-written against the raw `wayland-protocols` server bindings, same
//! pattern as `screencopy.rs`/`foreign_toplevel.rs`.
//!
//! srdwm has exactly one flat, global list of workspaces shared by every
//! output (`WindowManager::current_workspace`/`workspaces()`), not a
//! separate set per monitor - so this always advertises exactly one
//! `ext_workspace_group_handle_v1`, entered by every output, containing
//! every workspace. A compositor with per-output workspaces would need one
//! group per output instead; that's not this one.
//!
//! Only `activate` is implemented as a request: srdwm's workspaces are a
//! fixed, config-defined set (created once at startup, not created/removed/
//! reassigned at runtime), so `create_workspace`/`remove`/`assign` have
//! nothing meaningful to do and their capability bits are simply not
//! advertised - a client is expected to hide the UI for a request whose
//! capability bit is unset, per the protocol's own `capabilities` event
//! doc comment, rather than send it and be ignored.
//!
//! `deactivate` is also not advertised: srdwm always has exactly one
//! current workspace, there is no "no workspace active" state to request.

use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};
use wayland_protocols::ext::workspace::v1::server::ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1, GroupCapabilities};
use wayland_protocols::ext::workspace::v1::server::ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1, State, WorkspaceCapabilities};
use wayland_protocols::ext::workspace::v1::server::ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1};

use srdwm_core::WorkspaceId;

use crate::state::CompState;

const PROTOCOL_VERSION: u32 = 1;

pub struct WorkspaceManagerState {
    _global: smithay::reexports::wayland_server::backend::GlobalId,
}

impl WorkspaceManagerState {
    pub fn new<D>(dh: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ExtWorkspaceManagerV1, ()> + 'static,
    {
        Self { _global: dh.create_global::<D, ExtWorkspaceManagerV1, _>(PROTOCOL_VERSION, ()) }
    }
}

pub struct WorkspaceHandleData {
    workspace: WorkspaceId,
}

impl GlobalDispatch<ExtWorkspaceManagerV1, ()> for CompState {
    fn bind(state: &mut Self, dh: &DisplayHandle, client: &Client, manager: New<ExtWorkspaceManagerV1>, _data: &(), data_init: &mut DataInit<'_, Self>) {
        let manager = data_init.init(manager, ());

        let Ok(group) = client.create_resource::<ExtWorkspaceGroupHandleV1, (), CompState>(dh, manager.version(), ()) else {
            return;
        };
        manager.workspace_group(&group);
        // Nothing here is dynamically created/removed/reassigned - see the
        // module doc comment on why this is always empty.
        group.capabilities(GroupCapabilities::empty());
        // Every output enters the one group, since workspaces span all of
        // them. Only reaches outputs this client has *already* bound
        // `wl_output` for - a client binding `ext_workspace_manager_v1`
        // before any `wl_output` global would see no `output_enter` here,
        // and none later either, since nothing currently re-checks this on
        // a subsequent `wl_output` bind. Real clients bind their globals up
        // front, so not fixing that ordering-dependency is an accepted gap
        // rather than a deliberate design choice.
        for output in state.outputs() {
            for wl_output in output.client_outputs(client) {
                group.output_enter(&wl_output);
            }
        }

        let ids: Vec<WorkspaceId> = state.wm.borrow().workspaces().iter().map(|w| w.id).collect();
        for id in ids {
            announce_workspace(state, &manager, &group, id, client, dh);
        }
        manager.done();

        state.workspace_managers.push(manager);
        state.workspace_groups.push(group);
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for CompState {
    fn request(state: &mut Self, _client: &Client, manager: &ExtWorkspaceManagerV1, request: ext_workspace_manager_v1::Request, _data: &(), _dh: &DisplayHandle, _data_init: &mut DataInit<'_, Self>) {
        match request {
            // Every request below is handled the moment it arrives rather
            // than batched, so there is nothing left for `commit` itself to
            // flush - a plain acknowledgement.
            ext_workspace_manager_v1::Request::Commit => {}
            ext_workspace_manager_v1::Request::Stop => {
                manager.finished();
                state.workspace_managers.retain(|m| m != manager);
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _client: smithay::reexports::wayland_server::backend::ClientId, manager: &ExtWorkspaceManagerV1, _data: &()) {
        state.workspace_managers.retain(|m| m != manager);
    }
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for CompState {
    fn request(state: &mut Self, _client: &Client, group: &ExtWorkspaceGroupHandleV1, request: ext_workspace_group_handle_v1::Request, _data: &(), _dh: &DisplayHandle, _data_init: &mut DataInit<'_, Self>) {
        // `create_workspace` is deliberately unimplemented - its
        // capability bit is never advertised (see the module doc comment),
        // so a well-behaved client never sends it; ignored either way
        // rather than erroring a client that sends it anyway.
        if let ext_workspace_group_handle_v1::Request::Destroy = request {
            state.workspace_groups.retain(|g| g != group);
        }
    }

    fn destroyed(state: &mut Self, _client: smithay::reexports::wayland_server::backend::ClientId, group: &ExtWorkspaceGroupHandleV1, _data: &()) {
        state.workspace_groups.retain(|g| g != group);
    }
}

impl Dispatch<ExtWorkspaceHandleV1, WorkspaceHandleData> for CompState {
    fn request(state: &mut Self, _client: &Client, _handle: &ExtWorkspaceHandleV1, request: ext_workspace_handle_v1::Request, data: &WorkspaceHandleData, _dh: &DisplayHandle, _data_init: &mut DataInit<'_, Self>) {
        if let ext_workspace_handle_v1::Request::Activate = request {
            let mut wm = state.wm.borrow_mut();
            if wm.current_workspace() != data.workspace {
                wm.switch_workspace(data.workspace);
                drop(wm);
                // `switch_workspace` alone only updates core's own state --
                // nothing re-renders or shows/hides windows for the new
                // workspace without `main.rs`'s `sync()` running, which
                // only happens when a polled event sets `dirty`. Pushing
                // `WorkspaceChanged` is what makes that happen; see its
                // definition in `srdwm_core::event` for the fuller story
                // (this request path has the exact same problem the
                // pre-existing `SUPER+scroll` workspace gesture already
                // had, found while wiring this up).
                state.pending.borrow_mut().push(srdwm_core::Event::WorkspaceChanged);
                broadcast_active_workspace(state);
            }
        }
        // Deactivate/Assign/Remove: not advertised as available (see the
        // module doc comment), so real clients don't send them. Destroy
        // ends this protocol object, handled by `destroyed` below.
    }

    fn destroyed(state: &mut Self, _client: smithay::reexports::wayland_server::backend::ClientId, handle: &ExtWorkspaceHandleV1, data: &WorkspaceHandleData) {
        if let Some(handles) = state.workspace_handles.get_mut(&data.workspace) {
            handles.retain(|h| h != handle);
        }
    }
}

/// Packs a workspace's current flags into `ext_workspace_handle_v1.state`'s
/// wire format. srdwm's workspaces are never `Urgent` (nothing in this
/// codebase has an "urgent"/attention-request concept for a workspace) or
/// `Hidden` (all of them are always real, switchable workspaces, never an
/// internal/scratch one that should stay out of a switcher UI) - only
/// `Active` ever varies.
fn workspace_state(active: bool) -> State {
    if active {
        State::Active
    } else {
        State::empty()
    }
}

fn announce_workspace(state: &mut CompState, manager: &ExtWorkspaceManagerV1, group: &ExtWorkspaceGroupHandleV1, id: WorkspaceId, client: &Client, dh: &DisplayHandle) {
    let Ok(handle) = client.create_resource::<ExtWorkspaceHandleV1, WorkspaceHandleData, CompState>(dh, manager.version(), WorkspaceHandleData { workspace: id }) else {
        return;
    };
    manager.workspace(&handle);
    group.workspace_enter(&handle);

    let Some(w) = state.wm.borrow().workspaces().iter().find(|w| w.id == id).cloned() else { return };
    let active = state.wm.borrow().current_workspace() == id;
    handle.name(w.name);
    // 1D: srdwm numbers workspaces without any geometric/grid arrangement
    // - see this event's own doc comment on why a flat index is the
    // correct thing to send here, not a guess at one.
    handle.coordinates((id as u32).to_ne_bytes().to_vec());
    handle.state(workspace_state(active));
    handle.capabilities(WorkspaceCapabilities::Activate);

    state.workspace_handles.entry(id).or_default().push(handle);
}

/// Called after `switch_workspace` actually changes which workspace is
/// current - re-sends `state` (and `done`) for every workspace handle
/// across every bound client, so a switcher's active-workspace highlight
/// follows real changes instead of only ever reflecting whatever was
/// active when each handle was created. Broadcasts to *all* workspaces
/// (not just the old/new pair) since, unlike `foreign_toplevel`'s
/// per-window handle lookup, resolving "which workspace lost `Active`" has
/// no direct handle to key off here - this only fires on a real
/// workspace switch, not on every input event, so the cost is one small
/// loop per switch, not per frame.
pub(crate) fn broadcast_active_workspace(state: &mut CompState) {
    let current = state.wm.borrow().current_workspace();
    let ids: Vec<WorkspaceId> = state.workspace_handles.keys().copied().collect();
    for id in ids {
        let Some(handles) = state.workspace_handles.get(&id).cloned() else { continue };
        for handle in &handles {
            handle.state(workspace_state(id == current));
        }
    }
    for manager in state.workspace_managers.clone() {
        manager.done();
    }
}

/// Calls `broadcast_active_workspace` only when the current workspace has
/// actually changed since the last broadcast (by *any* means), called once
/// per frame from `CompState::tick_dirty_broadcasts`.
///
/// `input.rs`'s `SUPER+scroll` cycle gesture and this protocol's own
/// `activate` request both already call `broadcast_active_workspace`
/// directly. The gap, same shape and same root cause as `foreign_toplevel::
/// broadcast_dirty_state`'s (see its doc comment): the Lua `srd.workspace.
/// next()`/`.prev()`/`.switch()` API changes `WindowManager` state through
/// `crates/config`, which has no reachable path to this Wayland-specific
/// module, so a switch driven from a keybinding went stale in a dock's
/// workspace pill until some unrelated scroll or `activate` request
/// happened to resync it. `broadcast_active_workspace` itself is not safe
/// to call unconditionally every frame - it always re-sends `state` to
/// every handle and `done` to every manager, real protocol traffic, not
/// just a cheap comparison - so this gates it on an actual change first.
pub(crate) fn broadcast_dirty_active(state: &mut CompState) {
    let current = state.wm.borrow().current_workspace();
    if state.last_broadcast_workspace == Some(current) {
        return;
    }
    state.last_broadcast_workspace = Some(current);
    broadcast_active_workspace(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_active_workspace_gets_the_active_bit() {
        assert_eq!(workspace_state(true), State::Active);
        assert_eq!(workspace_state(false), State::empty());
    }
}
