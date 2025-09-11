// Minimal zwlr_virtual_pointer_unstable_v1 client: prints its own pid (so
// an external script can `srd dispatch pin input <pid> <window-id>` while
// it waits), then on a line from stdin sends motion_absolute/button/frame
// requests to drag from one point to another - built to live-verify
// srdwm's Phase 2 pinned-virtual-pointer delivery (see
// crates/wayland/src/virtual_pointer.rs's own module doc comment) against
// a real client, the same "purpose-built protocol test client, not a
// guess from source reading" precedent ../toplevel-activate already set
// for zwlr_foreign_toplevel_handle_v1.
//
// Usage:
//   vptest <x1> <y1> <x2> <y2>
//     Prints "pid <n>", then blocks on one line of stdin ("go\n").
//     Sends motion_absolute(x1,y1) [0..10000 extent], button press,
//     motion_absolute(x2,y2), button release, frame, flush, exits.

use std::io::BufRead;

use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::{self, ZwlrVirtualPointerV1};

const EXTENT: u32 = 10000;
const BTN_LEFT: u32 = 0x110;

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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 4 {
        eprintln!("usage: vptest <x1> <y1> <x2> <y2>  (fractions of EXTENT={EXTENT}, e.g. 5000 5000 is dead center)");
        std::process::exit(1);
    }
    let x1: u32 = args[0].parse().expect("x1 must be a number");
    let y1: u32 = args[1].parse().expect("y1 must be a number");
    let x2: u32 = args[2].parse().expect("x2 must be a number");
    let y2: u32 = args[3].parse().expect("y2 must be a number");

    let conn = Connection::connect_to_env().expect("failed to connect to the Wayland compositor - is WAYLAND_DISPLAY set?");
    let display = conn.display();
    let mut event_queue: EventQueue<State> = conn.new_event_queue();
    let qh = event_queue.handle();
    let _registry = display.get_registry(&qh, ());

    let mut state = State { seat: None, manager: None };
    event_queue.roundtrip(&mut state).expect("initial roundtrip failed");

    let Some(manager) = state.manager.clone() else {
        eprintln!("compositor does not advertise zwlr_virtual_pointer_manager_v1");
        std::process::exit(1);
    };
    let Some(seat) = state.seat.clone() else {
        eprintln!("compositor does not advertise wl_seat");
        std::process::exit(1);
    };

    let pointer = manager.create_virtual_pointer(Some(&seat), &qh, ());
    event_queue.roundtrip(&mut state).expect("create_virtual_pointer roundtrip failed");

    println!("pid {}", std::process::id());
    println!("waiting for a line on stdin before sending any motion...");
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).expect("failed to read stdin");

    let time = 0;
    pointer.motion_absolute(time, x1, y1, EXTENT, EXTENT);
    pointer.frame();
    event_queue.roundtrip(&mut state).expect("motion_absolute(1) roundtrip failed");

    pointer.button(time, BTN_LEFT, wayland_client::protocol::wl_pointer::ButtonState::Pressed);
    pointer.frame();
    event_queue.roundtrip(&mut state).expect("button press roundtrip failed");

    pointer.motion_absolute(time, x2, y2, EXTENT, EXTENT);
    pointer.frame();
    event_queue.roundtrip(&mut state).expect("motion_absolute(2) roundtrip failed");

    pointer.button(time, BTN_LEFT, wayland_client::protocol::wl_pointer::ButtonState::Released);
    pointer.frame();
    event_queue.roundtrip(&mut state).expect("button release roundtrip failed");

    println!("done.");
}
