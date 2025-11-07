use super::*;
use std::cell::RefCell;
use std::io::BufRead;
use std::rc::Rc;

/// Takes a reader the caller already owns, rather than building a fresh
/// `BufReader` around a cloned handle every call (what this used to do):
/// `BufRead::read_line` is free to read further ahead than one line in
/// a single syscall whenever more is already sitting in the kernel
/// socket buffer - true the moment a caller (like `"subscribe"`'s
/// two-line initial reply, added alongside the workspace event) writes
/// more than one line in one `write_all`. A fresh `BufReader` built
/// per call has nowhere to keep whatever it over-read once the call
/// returns and it's dropped - that data is already gone from the
/// kernel's queue, so the *next* fresh `BufReader`'s read blocks
/// forever waiting for bytes that already arrived and were silently
/// discarded. Hung an entire test run silently, with no compiler error
/// and no panic to point at it, until the underlying `cargo test`
/// process was found sitting at 0% CPU with no explanation. One
/// `BufReader`, reused for every `read_line` call in a test, keeps
/// whatever it over-reads available for the next call instead.
fn read_line(reader: &mut std::io::BufReader<UnixStream>) -> String {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    line
}

#[test]
fn subscribe_gets_an_immediate_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
    server.poll(&wm);

    // Four lines, not one: a client snapshot, a workspace snapshot, a
    // keyboard-layout snapshot, and a monitor snapshot, same as every
    // later push - see the `"subscribe"` match arm's own doc comment
    // for why all four are sent immediately rather than waiting for
    // the first real change of each kind.
    let clients_line = read_line(&mut reader);
    assert!(clients_line.contains(r#""event":"clients""#));
    assert!(clients_line.contains(r#""clients":[]"#));

    let workspaces_line = read_line(&mut reader);
    assert!(workspaces_line.contains(r#""event":"workspaces""#));
    assert!(workspaces_line.contains(r#""id":1"#));
    assert!(workspaces_line.contains(r#""active":true"#));

    let layout_line = read_line(&mut reader);
    assert!(layout_line.contains(r#""event":"keyboard_layout""#));
    assert!(layout_line.contains(r#""layout":""#));

    let monitors_line = read_line(&mut reader);
    assert!(monitors_line.contains(r#""event":"monitors""#));
    assert!(monitors_line.contains(r#""monitors":[]"#));
}

#[test]
fn subscribe_then_a_monitor_change_pushes_a_fresh_monitors_event() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
    server.poll(&wm);
    let _ = read_line(&mut reader); // clients
    let _ = read_line(&mut reader); // workspaces
    let _ = read_line(&mut reader); // keyboard_layout
    let _ = read_line(&mut reader); // monitors (empty)

    wm.borrow_mut().set_monitors(vec![{
        let mut m = srdwm_core::Monitor::new(0, "HDMI-A-1", srdwm_core::Rect::new(0, 0, 1920, 1080));
        m.primary = true;
        m
    }]);
    server.poll(&wm);

    // A real monitor existing for the first time also changes which
    // monitor (if any) `WorkspaceInfo::monitor` reports for the
    // now-visible workspace - `None` (no monitor existed to show it)
    // to `Some(0)` - so a "workspaces" event fires too, ahead of
    // "monitors" in `poll`'s own emission order. Drained here, not
    // asserted on: this test is about the monitors event specifically.
    let workspaces_line = read_line(&mut reader);
    assert!(workspaces_line.contains(r#""event":"workspaces""#));

    let line = read_line(&mut reader);
    assert!(line.contains(r#""event":"monitors""#));
    assert!(line.contains(r#""name":"HDMI-A-1""#));
}

#[test]
fn subscribe_then_a_window_change_pushes_a_fresh_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
    server.poll(&wm);
    let _initial_clients = read_line(&mut reader);
    let _initial_workspaces = read_line(&mut reader);
    let _initial_layout = read_line(&mut reader);
    let _initial_monitors = read_line(&mut reader);

    {
        let mut wm = wm.borrow_mut();
        let id = wm.alloc_window_id();
        wm.add_window(srdwm_core::Window::new(id, "hello"));
    }
    server.poll(&wm);

    let pushed = read_line(&mut reader);
    assert!(pushed.contains(r#""event":"clients""#));
    assert!(pushed.contains(r#""title":"hello""#));
}

#[test]
fn subscribe_then_a_workspace_switch_pushes_a_fresh_workspaces_event() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().add_workspace("2", "dynamic");

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
    server.poll(&wm);
    let _initial_clients = read_line(&mut reader);
    let _initial_workspaces = read_line(&mut reader);
    let _initial_layout = read_line(&mut reader);
    let _initial_monitors = read_line(&mut reader);

    wm.borrow_mut().switch_workspace(2);
    server.poll(&wm);

    // Switching touches no window, so the clients list is unchanged --
    // the next line waiting must be the workspaces push, not a
    // clients one that never comes.
    let pushed = read_line(&mut reader);
    assert!(pushed.contains(r#""event":"workspaces""#));
    assert!(pushed.contains(r#""id":2,"name":"2","layout":"dynamic","active":true"#));
}

#[test]
fn a_poll_with_no_real_change_pushes_nothing_new() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
    server.poll(&wm);
    let _initial_clients = read_line(&mut reader);
    let _initial_workspaces = read_line(&mut reader);
    let _initial_layout = read_line(&mut reader);
    let _initial_monitors = read_line(&mut reader);

    // Nothing changed between these two polls - a second push would
    // show up as a second readable line the client isn't expecting.
    server.poll(&wm);
    server.poll(&wm);
    client.set_nonblocking(true).unwrap();
    let mut buf = [0u8; 16];
    match client.read(&mut buf) {
        Err(e) if e.kind() == ErrorKind::WouldBlock => {}
        other => panic!("expected no further data, got {other:?}"),
    }
}

#[test]
fn workspaces_command_reports_the_default_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"workspaces\"}\n").unwrap();
    server.poll(&wm);

    let line = read_line(&mut reader);
    assert!(!line.contains(r#""event""#), "one-shot command, same plain shape as \"clients\"");
    assert!(line.contains(r#""id":1,"name":"1","layout":"dynamic","active":true"#));
}

#[test]
fn workspaces_and_monitors_agree_on_which_monitor_shows_which_workspace() {
    // `WorkspaceInfo::monitor` and `MonitorInfo::active_workspace` are
    // the same fact from either direction - requested by an AGS peer
    // session so a workspace pill or a per-monitor picker can each
    // read it from whichever side it already has in hand. Default
    // `WindowManager::new()` starts on workspace `1` with no real
    // monitor set up yet; adding one real monitor (id `0`) must make
    // workspace `1`'s own `monitor` read back `0`, and that monitor's
    // `active_workspace` read back `1`.
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(0, "eDP-1", srdwm_core::Rect::new(0, 0, 1920, 1080))]);

    // Two separate connections, not one reused - a one-shot command's
    // connection closes after its reply (see `a_oneshot_clients_
    // request_still_closes_the_connection_as_before`), so a second
    // command on the same `client` would just hit a broken pipe.
    let mut workspaces_client = UnixStream::connect(&server.path).unwrap();
    let mut workspaces_reader = std::io::BufReader::new(workspaces_client.try_clone().unwrap());
    workspaces_client.write_all(b"{\"cmd\":\"workspaces\"}\n").unwrap();
    server.poll(&wm);
    let workspaces_line = read_line(&mut workspaces_reader);
    assert!(workspaces_line.contains(r#""id":1,"name":"1","layout":"dynamic","active":true,"monitor":0"#));

    let mut monitors_client = UnixStream::connect(&server.path).unwrap();
    let mut monitors_reader = std::io::BufReader::new(monitors_client.try_clone().unwrap());
    monitors_client.write_all(b"{\"cmd\":\"monitors\"}\n").unwrap();
    server.poll(&wm);
    let monitors_line = read_line(&mut monitors_reader);
    assert!(monitors_line.contains(r#""name":"eDP-1""#));
    assert!(monitors_line.contains(r#""active_workspace":1"#));
}

#[test]
fn settings_command_reflects_a_prior_set() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));

    // A fresh connection per command - one-shot commands close the
    // connection after replying (see `a_oneshot_clients_request_
    // still_closes_the_connection_as_before`), so a second write on
    // the same client after its first reply hits a closed socket.
    let mut set_client = UnixStream::connect(&server.path).unwrap();
    set_client.write_all(b"{\"cmd\":\"set\",\"key\":\"night_light\",\"value\":true}\n").unwrap();
    server.poll(&wm);
    let _set_reply = read_line(&mut std::io::BufReader::new(set_client.try_clone().unwrap()));

    let mut settings_client = UnixStream::connect(&server.path).unwrap();
    settings_client.write_all(b"{\"cmd\":\"settings\"}\n").unwrap();
    server.poll(&wm);
    let line = read_line(&mut std::io::BufReader::new(settings_client));
    assert!(!line.contains(r#""event""#), "one-shot command, same plain shape as \"clients\"");
    assert!(line.contains(r#""night_light":true"#));
    assert!(line.contains(r#""reading_mode":false"#), "night_light and reading_mode share one slot, so setting one leaves the other off");
}

#[test]
fn setting_night_light_then_reading_mode_clears_night_light() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));

    let mut client = UnixStream::connect(&server.path).unwrap();
    client.write_all(b"{\"cmd\":\"set\",\"key\":\"night_light\",\"value\":true}\n").unwrap();
    server.poll(&wm);
    let _ = read_line(&mut std::io::BufReader::new(client));

    let mut client = UnixStream::connect(&server.path).unwrap();
    client.write_all(b"{\"cmd\":\"set\",\"key\":\"reading_mode\",\"value\":true}\n").unwrap();
    server.poll(&wm);
    let _ = read_line(&mut std::io::BufReader::new(client));

    let mut client = UnixStream::connect(&server.path).unwrap();
    client.write_all(b"{\"cmd\":\"settings\"}\n").unwrap();
    server.poll(&wm);
    let line = read_line(&mut std::io::BufReader::new(client));
    assert!(line.contains(r#""night_light":false"#));
    assert!(line.contains(r#""reading_mode":true"#));
}

#[test]
fn keyboard_layout_command_reports_the_current_layout() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().set_keyboard_layout("English (US)");

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"keyboard_layout\"}\n").unwrap();
    server.poll(&wm);

    let line = read_line(&mut reader);
    assert!(!line.contains(r#""event""#), "one-shot command, same plain shape as \"clients\"");
    assert!(line.contains(r#""layout":"English (US)""#));
}

#[test]
fn cycle_keyboard_layout_dispatch_queues_a_request_for_main_rs_to_act_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"cycle_keyboard_layout\"}\n").unwrap();
    server.poll(&wm);
    let _ = read_line(&mut reader);

    // `IpcServer` has no seat/keyboard of its own to actually cycle --
    // it can only queue the intent for `main.rs`'s `sync()` to act on,
    // same as `close_requests`/`activate_workspace`. This is as far as
    // this crate can verify the request landed.
    assert_eq!(wm.borrow_mut().take_keyboard_layout_cycle_requests(), 1);
}

#[test]
fn activate_workspace_dispatch_switches_the_current_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().add_workspace("2", "dynamic");

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    // `id: 2`, not `1` - `WindowManager::new`'s default workspace is
    // id 1 (already current), and `switch_workspace` no-ops when asked
    // to "switch" to the already-current workspace, so activating `1`
    // would silently test nothing: the assertion below would pass
    // whether or not this dispatch actually worked at all.
    client.write_all(b"{\"cmd\":\"activate_workspace\",\"id\":2}\n").unwrap();
    server.poll(&wm);
    let _ = read_line(&mut reader);

    assert_eq!(wm.borrow().current_workspace(), 2);
}

#[test]
fn set_output_position_resolves_a_monitor_name_to_its_id() {
    // The CLI (`srd dispatch set output position <name> <x> <y>`)
    // sends a `name`, not an `id`, whenever the caller didn't already
    // have a numeric id handy - `srd monitors` reports names, not an
    // arbitrary index, so a display-arrangement UI reasonably works
    // in those terms.
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().set_monitors(vec![
        srdwm_core::Monitor::new(0, "EmbeddedDisplayPort-1", srdwm_core::Rect::new(0, 0, 1920, 1080)),
        srdwm_core::Monitor::new(1, "HDMI-A-1", srdwm_core::Rect::new(1920, 0, 1920, 1080)),
    ]);

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"set_output_position\",\"name\":\"HDMI-A-1\",\"x\":1920,\"y\":0}\n").unwrap();
    server.poll(&wm);
    let _ = read_line(&mut reader);

    let queued = wm.borrow_mut().drain_output_position_requests();
    assert_eq!(queued, vec![(1, 1920, 0)], "must resolve to monitor id 1, not treat the name as missing");
}

#[test]
fn set_output_position_with_an_unknown_name_errors_instead_of_silently_no_opping() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(0, "EmbeddedDisplayPort-1", srdwm_core::Rect::new(0, 0, 1920, 1080))]);

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"set_output_position\",\"name\":\"nonexistent-output\",\"x\":0,\"y\":0}\n").unwrap();
    server.poll(&wm);
    let line = read_line(&mut reader);

    assert!(line.contains(r#""error""#), "an unresolvable name must error, not silently queue nothing");
    assert!(wm.borrow_mut().drain_output_position_requests().is_empty());
}

#[test]
fn set_output_enabled_accepts_a_name_directly() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(0, "HDMI-A-1", srdwm_core::Rect::new(0, 0, 1920, 1080))]);

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"set_output_enabled\",\"name\":\"HDMI-A-1\",\"enabled\":false}\n").unwrap();
    server.poll(&wm);
    let _ = read_line(&mut reader);

    assert_eq!(wm.borrow_mut().drain_output_enable_requests(), vec![("HDMI-A-1".to_string(), false)]);
}

#[test]
fn set_output_enabled_resolves_an_id_to_its_name_for_the_disable_direction() {
    // `id` only ever resolves against the *live* monitor list - fine
    // for disabling something currently connected, which is the only
    // case where a stale-index concern doesn't apply yet.
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(3, "HDMI-A-1", srdwm_core::Rect::new(0, 0, 1920, 1080))]);

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"set_output_enabled\",\"id\":3,\"enabled\":false}\n").unwrap();
    server.poll(&wm);
    let _ = read_line(&mut reader);

    assert_eq!(wm.borrow_mut().drain_output_enable_requests(), vec![("HDMI-A-1".to_string(), false)]);
}

#[test]
fn set_output_enabled_with_neither_name_nor_a_resolvable_id_errors() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"set_output_enabled\",\"enabled\":true}\n").unwrap();
    server.poll(&wm);
    let line = read_line(&mut reader);

    assert!(line.contains(r#""error""#));
    assert!(wm.borrow_mut().drain_output_enable_requests().is_empty());
}

#[test]
fn set_monitor_split_accepts_a_name_directly_and_queues_a_request() {
    // Queued, not applied directly - see `WindowManager::monitor_split_
    // requests`'s own doc comment for why a direct mutation here left
    // `srd monitors` reporting the stale, unsplit layout indefinitely
    // (nothing re-triggers `monitors`' own passive cache). Only the
    // backend that owns real output hardware can call `set_monitor_split`
    // and push the matching recompute event, so this test - run from the
    // platform crate alone, with no such backend - can only observe the
    // queued request, not the applied state.
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(0, "eDP-1", srdwm_core::Rect::new(0, 0, 1920, 1080))]);

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"set_monitor_split\",\"name\":\"eDP-1\",\"parts\":2,\"rows\":false}\n").unwrap();
    server.poll(&wm);
    let _ = read_line(&mut reader);

    assert_eq!(wm.borrow_mut().drain_monitor_split_requests(), vec![("eDP-1".to_string(), 2, false)]);
}

#[test]
fn set_monitor_split_resolves_an_id_to_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(3, "HDMI-A-1", srdwm_core::Rect::new(0, 0, 1920, 1080))]);

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"set_monitor_split\",\"id\":3,\"parts\":3,\"rows\":true}\n").unwrap();
    server.poll(&wm);
    let _ = read_line(&mut reader);

    assert_eq!(wm.borrow_mut().drain_monitor_split_requests(), vec![("HDMI-A-1".to_string(), 3, true)]);
}

#[test]
fn set_monitor_split_replaces_a_still_pending_request_for_the_same_name() {
    // Same "last write wins per name" semantics as `request_output_
    // position` - only the latest requested split for a given output
    // matters if several arrive before the backend's next drain.
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(0, "eDP-1", srdwm_core::Rect::new(0, 0, 1920, 1080))]);

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"set_monitor_split\",\"name\":\"eDP-1\",\"parts\":2,\"rows\":false}\n").unwrap();
    server.poll(&wm);
    let _ = read_line(&mut reader);

    // A fresh connection - each `srd dispatch` is its own one-shot socket
    // connection in practice, not a second write on an already-answered
    // one (which the server closes after replying).
    let mut client2 = UnixStream::connect(&server.path).unwrap();
    let mut reader2 = std::io::BufReader::new(client2.try_clone().unwrap());
    client2.write_all(b"{\"cmd\":\"set_monitor_split\",\"name\":\"eDP-1\",\"parts\":1}\n").unwrap();
    server.poll(&wm);
    let _ = read_line(&mut reader2);

    assert_eq!(wm.borrow_mut().drain_monitor_split_requests(), vec![("eDP-1".to_string(), 1, false)]);
}

#[test]
fn set_monitor_split_with_neither_name_nor_a_resolvable_id_errors() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"set_monitor_split\",\"parts\":2}\n").unwrap();
    server.poll(&wm);
    let line = read_line(&mut reader);

    assert!(line.contains(r#""error""#));
}

#[test]
fn monitors_query_marks_a_virtual_output_so_a_client_does_not_treat_it_as_a_real_hotplug() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    let real = srdwm_core::Monitor::new(0, "eDP-1", srdwm_core::Rect::new(0, 0, 1920, 1080));
    let mut fake = srdwm_core::Monitor::new(1, "FAKE-1", srdwm_core::Rect::new(1920, 0, 1920, 1080));
    fake.is_virtual = true;
    wm.borrow_mut().set_monitors(vec![real, fake]);

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"monitors\"}\n").unwrap();
    server.poll(&wm);
    let line = read_line(&mut reader);

    let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
    let monitors = parsed["monitors"].as_array().unwrap();
    let real = monitors.iter().find(|m| m["name"] == "eDP-1").unwrap();
    let fake = monitors.iter().find(|m| m["name"] == "FAKE-1").unwrap();
    assert_eq!(real["virtual"], false, "a real output must not be marked virtual");
    assert_eq!(fake["virtual"], true, "a fake monitor must be marked virtual so a client can tell it apart from a real hotplug");
}

#[test]
fn monitors_query_lists_a_disabled_output_alongside_live_ones() {
    // What the AGS peer session asked for directly: a disabled output
    // must not just vanish from `srd monitors` - it needs a row
    // (name + `enabled: false`) to offer turning back on.
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(0, "EmbeddedDisplayPort-1", srdwm_core::Rect::new(0, 0, 1920, 1080))]);
    wm.borrow_mut().set_disabled_monitor("HDMI-A-1".to_string(), srdwm_core::Rect::new(1920, 0, 1920, 1080), srdwm_core::Rect::new(1920, 0, 1920, 1080), false);

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"monitors\"}\n").unwrap();
    server.poll(&wm);
    let line = read_line(&mut reader);

    assert!(line.contains(r#""name":"EmbeddedDisplayPort-1""#));
    assert!(line.contains(r#""enabled":true"#), "live outputs must report enabled:true");
    assert!(line.contains(r#""name":"HDMI-A-1""#), "disabled output must still be listed");
    assert!(line.contains(r#""enabled":false"#), "the disabled output's own row must say so");
}

#[test]
fn monitors_query_marks_a_split_part_so_a_client_does_not_treat_it_as_a_real_output() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    let whole = srdwm_core::Monitor::new(0, "eDP-1", srdwm_core::Rect::new(0, 0, 1920, 1080));
    let mut half = srdwm_core::Monitor::new(1, "eDP-1-1", srdwm_core::Rect::new(0, 0, 960, 1080));
    half.split = true;
    wm.borrow_mut().set_monitors(vec![whole, half]);

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"monitors\"}\n").unwrap();
    server.poll(&wm);
    let line = read_line(&mut reader);

    let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
    let monitors = parsed["monitors"].as_array().unwrap();
    let whole = monitors.iter().find(|m| m["name"] == "eDP-1").unwrap();
    let half = monitors.iter().find(|m| m["name"] == "eDP-1-1").unwrap();
    assert_eq!(whole["split"], false, "an ordinary output must not be marked as a split part");
    assert_eq!(half["split"], true, "a split part must be marked so a client can tell it apart from a real output");
}

#[test]
fn lock_dispatch_queues_a_lock_request_with_no_id_needed() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    assert!(!wm.borrow_mut().drain_lock_request(), "nothing queued before the dispatch");

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"lock\"}\n").unwrap();
    server.poll(&wm);
    let response = read_line(&mut reader);

    assert!(!response.contains(r#""ok":false"#), "lock must not require an id: {response}");
    assert!(wm.borrow_mut().drain_lock_request(), "the dispatch must have queued a lock request");
}

#[test]
fn toggle_floating_dispatch_flips_the_windows_floating_flag() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    let id = {
        let mut wm = wm.borrow_mut();
        let id = wm.alloc_window_id();
        wm.add_window(srdwm_core::Window::new(id, "a"));
        id
    };
    assert!(!wm.borrow().is_floating(id));

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(format!("{{\"cmd\":\"toggle_floating\",\"id\":{id}}}\n").as_bytes()).unwrap();
    server.poll(&wm);
    let _ = read_line(&mut reader);

    assert!(wm.borrow().is_floating(id));
}

#[test]
fn toggle_pinned_dispatch_flips_always_on_top() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    let id = {
        let mut wm = wm.borrow_mut();
        let id = wm.alloc_window_id();
        wm.add_window(srdwm_core::Window::new(id, "a"));
        id
    };

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(format!("{{\"cmd\":\"toggle_pinned\",\"id\":{id}}}\n").as_bytes()).unwrap();
    server.poll(&wm);
    let _ = read_line(&mut reader);

    assert!(wm.borrow().window(id).unwrap().always_on_top);
}

#[test]
fn move_window_dispatch_swaps_with_the_neighbour_and_focuses_the_target_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(0, "primary", srdwm_core::Rect::new(0, 0, 1920, 1080))]);
    let (a, b) = {
        let mut wm = wm.borrow_mut();
        let a = wm.alloc_window_id();
        wm.add_window(srdwm_core::Window::new(a, "a"));
        let b = wm.alloc_window_id();
        wm.add_window(srdwm_core::Window::new(b, "b"));
        // Set geometry *after* `add_window`, not before - a dynamic-
        // layout workspace's own `SmartPlacement` grid overrides
        // whatever geometry a freshly constructed `Window` already
        // carried in, the same lesson `crates/core/src/manager/
        // tests.rs`'s decoration-mode tests already ran into.
        wm.window_mut(a).unwrap().geometry = srdwm_core::Rect::new(0, 0, 400, 300);
        wm.window_mut(b).unwrap().geometry = srdwm_core::Rect::new(500, 0, 400, 300);
        (a, b)
    };
    // Focus `a` first, then ask to move `b` - the dispatch must focus
    // `b` itself before swapping, not silently move whatever was
    // already focused.
    wm.borrow_mut().focus_window(a);

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(format!("{{\"cmd\":\"move_window\",\"id\":{b},\"direction\":\"left\"}}\n").as_bytes()).unwrap();
    server.poll(&wm);
    let _ = read_line(&mut reader);

    let wm = wm.borrow();
    assert_eq!(wm.window(b).unwrap().geometry.x, 0, "b must have swapped into a's old position");
    assert_eq!(wm.window(a).unwrap().geometry.x, 500, "a must have swapped into b's old position");
}

#[test]
fn move_to_workspace_dispatch_moves_the_window_without_switching_the_current_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().add_workspace("2", "dynamic");
    let id = {
        let mut wm = wm.borrow_mut();
        let id = wm.alloc_window_id();
        wm.add_window(srdwm_core::Window::new(id, "a"));
        id
    };

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(format!("{{\"cmd\":\"move_to_workspace\",\"id\":{id},\"workspace\":2}}\n").as_bytes()).unwrap();
    server.poll(&wm);
    let _ = read_line(&mut reader);

    let wm = wm.borrow();
    assert_eq!(wm.window(id).unwrap().workspace, 2);
    assert_eq!(wm.current_workspace(), 1, "moving a window to a workspace must not also switch to it");
}

#[test]
fn a_oneshot_clients_request_still_closes_the_connection_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));

    let mut client = UnixStream::connect(&server.path).unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    client.write_all(b"{\"cmd\":\"clients\"}\n").unwrap();
    server.poll(&wm);
    let line = read_line(&mut reader);
    // The plain, pre-existing shape - no `"event"` field - so
    // existing one-shot polling consumers (`crates/ctl`) see no change.
    assert!(!line.contains(r#""event""#));
    assert!(line.contains(r#""clients""#));

    {
        let mut wm = wm.borrow_mut();
        let id = wm.alloc_window_id();
        wm.add_window(srdwm_core::Window::new(id, "later"));
    }
    server.poll(&wm);
    client.set_nonblocking(true).unwrap();
    let mut buf = [0u8; 16];
    let result = client.read(&mut buf);
    // A one-shot connection was never registered as a subscriber, so a
    // later change must not be pushed to it - and the server already
    // closed its end after the single reply, so a read either sees EOF
    // (0 bytes) or, depending on how quickly the close propagates,
    // WouldBlock; either is correct, actual new data would not be.
    match result {
        Ok(n) => assert_eq!(n, 0, "expected EOF, got {n} bytes of unexpected data"),
        Err(e) => assert_eq!(e.kind(), ErrorKind::WouldBlock),
    }
}

#[test]
fn settings_reports_the_readback_fields_flagged_as_missing() {
    // Confirms the whole batch at once rather than one test per field --
    // these were all added together for the same reason (a settings
    // panel could set any of them blind but never read the current value
    // back).
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));

    let mut client = UnixStream::connect(&server.path).unwrap();
    client.write_all(b"{\"cmd\":\"settings\"}\n").unwrap();
    server.poll(&wm);
    let line = read_line(&mut std::io::BufReader::new(client));
    let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
    for field in ["border_width", "border_color", "corner_radius", "decoration_mode_server", "gap_inner", "gap_outer", "master_ratio", "master_count"] {
        assert!(parsed.get(field).is_some(), "settings response is missing '{field}'");
    }
    assert_eq!(parsed["border_color"].as_str().unwrap().chars().next(), Some('#'), "border_color must be a hex string, matching what srd set border_color itself accepts");
}

#[test]
fn set_master_ratio_is_reflected_immediately_by_settings() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(0, "eDP-1", srdwm_core::Rect::new(0, 0, 1920, 1080))]);

    let mut set_client = UnixStream::connect(&server.path).unwrap();
    set_client.write_all(b"{\"cmd\":\"set\",\"key\":\"master_ratio\",\"value\":0.7}\n").unwrap();
    server.poll(&wm);
    let _ = read_line(&mut std::io::BufReader::new(set_client));

    let mut settings_client = UnixStream::connect(&server.path).unwrap();
    settings_client.write_all(b"{\"cmd\":\"settings\"}\n").unwrap();
    server.poll(&wm);
    let line = read_line(&mut std::io::BufReader::new(settings_client));
    assert!(line.contains(r#""master_ratio":0.7"#));
}

#[test]
fn set_master_ratio_clamps_to_a_sane_range() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));

    let mut client = UnixStream::connect(&server.path).unwrap();
    client.write_all(b"{\"cmd\":\"set\",\"key\":\"master_ratio\",\"value\":1.5}\n").unwrap();
    server.poll(&wm);
    let _ = read_line(&mut std::io::BufReader::new(client));

    assert_eq!(wm.borrow().tiling.master_ratio, 0.9, "a value past the sane range must clamp, not be accepted verbatim");
}

#[test]
fn pinned_inputs_lists_nothing_before_any_pin_is_applied() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));

    let mut client = UnixStream::connect(&server.path).unwrap();
    client.write_all(b"{\"cmd\":\"pinned_inputs\"}\n").unwrap();
    server.poll(&wm);
    let line = read_line(&mut std::io::BufReader::new(client));
    assert_eq!(line.trim_end(), r#"{"pinned":[]}"#);
}

#[test]
fn pinned_inputs_reports_a_pin_once_the_backend_has_applied_it() {
    // `pin_input` only ever queues a *request* - `WindowManager::
    // set_pinned_window` is the backend's own confirmation that it was
    // genuinely applied, called directly here to simulate that (the real
    // caller is `CompState::set_virtual_pointer_pin` in the wayland
    // crate, unreachable from a platform-crate test).
    let dir = tempfile::tempdir().unwrap();
    let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
    let wm = Rc::new(RefCell::new(WindowManager::new()));
    wm.borrow_mut().set_pinned_window(12345, Some(7));

    let mut client = UnixStream::connect(&server.path).unwrap();
    client.write_all(b"{\"cmd\":\"pinned_inputs\"}\n").unwrap();
    server.poll(&wm);
    let line = read_line(&mut std::io::BufReader::new(client));
    assert_eq!(line.trim_end(), r#"{"pinned":[{"pid":12345,"id":7}]}"#);
}
