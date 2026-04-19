// A scriptable zwlr_virtual_pointer_unstable_v1 driver: reads one command
// per line from stdin and turns it into a virtual-pointer request against
// whatever compositor WAYLAND_DISPLAY names.
//
// WHY THIS EXISTS RATHER THAN ydotool
//
// ydotool writes to /dev/uinput. That is a kernel-level device shared by
// every session on the machine, so a synthetic click from it lands wherever
// the real seat's focus happens to be - which, when the target is a nested
// throwaway compositor, is very often the user's real desktop instead. This
// tool is an ordinary Wayland client of one specific compositor, so its
// input physically cannot reach any other one. That makes it the safe way
// to drive a nested test instance, which is the only reason it exists.
//
// ../virtual-pointer-pin-test is not this: it is a fixed left-button drag
// used to verify pinned delivery, and it blocks on stdin exactly once. This
// one stays alive and takes a stream of commands, so a shell script can
// interleave a `grim` screenshot between a move and the click that follows
// it - which is what "never click at a position you have not verified
// first" actually requires.
//
// Usage:
//   vpclick            (commands on stdin, one per line)
//
//   move <x> <y>       absolute, as a fraction of EXTENT (5000 5000 = centre)
//   press <button>     left | right | middle
//   release <button>
//   click <button>     press then release
//   sync               round-trip and acknowledge, nothing else
//   quit               exit
//
// Every command prints "ok <command>" once it has round-tripped, so a
// driving script can wait for the compositor to have actually seen it
// rather than sleeping and hoping.

use std::io::{BufRead, Write};

use wayland_client::protocol::wl_pointer::ButtonState;
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::{self, ZwlrVirtualPointerV1};

const EXTENT: u32 = 10000;

/// Linux `input-event-codes.h` button codes - what the protocol asks for
/// verbatim, not a wl_pointer enum.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

struct State {
    seat: Option<WlSeat>,
    manager: Option<ZwlrVirtualPointerManagerV1>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            if interface == "wl_seat" {
                state.seat = Some(registry.bind::<WlSeat, _, _>(name, version.min(9), qh, ()));
            } else if interface == "zwlr_virtual_pointer_manager_v1" {
                state.manager = Some(registry.bind::<ZwlrVirtualPointerManagerV1, _, _>(name, version.min(2), qh, ()));
            }
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(_: &mut Self, _: &WlSeat, _: wayland_client::protocol::wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn event(_: &mut Self, _: &ZwlrVirtualPointerManagerV1, _: wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
    }
}

impl Dispatch<ZwlrVirtualPointerV1, ()> for State {
    fn event(_: &mut Self, _: &ZwlrVirtualPointerV1, _: zwlr_virtual_pointer_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

fn button_code(name: &str) -> Option<u32> {
    match name {
        "left" => Some(BTN_LEFT),
        "right" => Some(BTN_RIGHT),
        "middle" => Some(BTN_MIDDLE),
        _ => None,
    }
}

fn main() {
    let conn = Connection::connect_to_env().expect("failed to connect to the Wayland compositor - is WAYLAND_DISPLAY set?");
    let display = conn.display();
    let mut queue: EventQueue<State> = conn.new_event_queue();
    let qh = queue.handle();
    let _registry = display.get_registry(&qh, ());

    let mut state = State { seat: None, manager: None };
    queue.roundtrip(&mut state).expect("initial roundtrip failed");

    let Some(manager) = state.manager.clone() else {
        eprintln!("compositor does not advertise zwlr_virtual_pointer_manager_v1");
        std::process::exit(1);
    };
    let Some(seat) = state.seat.clone() else {
        eprintln!("compositor does not advertise wl_seat");
        std::process::exit(1);
    };

    let pointer = manager.create_virtual_pointer(Some(&seat), &qh, ());
    queue.roundtrip(&mut state).expect("create_virtual_pointer roundtrip failed");
    println!("ready pid {}", std::process::id());
    let _ = std::io::stdout().flush();

    // A monotonically rising millisecond stamp. Some compositors ignore
    // this entirely, but a click whose press and release carry the same
    // timestamp is indistinguishable from a double-click to anything that
    // does look, so it is worth stepping.
    let mut time: u32 = 1;
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.expect("failed to read stdin");
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some(&cmd) = parts.first() else { continue };
        match cmd {
            "move" if parts.len() == 3 => {
                let x: u32 = parts[1].parse().expect("x must be a number");
                let y: u32 = parts[2].parse().expect("y must be a number");
                pointer.motion_absolute(time, x, y, EXTENT, EXTENT);
                pointer.frame();
            }
            "press" | "release" | "click" if parts.len() == 2 => {
                let Some(code) = button_code(parts[1]) else {
                    println!("err unknown button {}", parts[1]);
                    let _ = std::io::stdout().flush();
                    continue;
                };
                if cmd != "release" {
                    pointer.button(time, code, ButtonState::Pressed);
                    pointer.frame();
                    time += 10;
                }
                if cmd != "press" {
                    pointer.button(time, code, ButtonState::Released);
                    pointer.frame();
                }
            }
            "sync" => {}
            "quit" => break,
            _ => {
                println!("err bad command: {line}");
                let _ = std::io::stdout().flush();
                continue;
            }
        }
        time += 10;
        queue.roundtrip(&mut state).expect("roundtrip failed");
        println!("ok {line}");
        let _ = std::io::stdout().flush();
    }
}
