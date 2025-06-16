// Minimal zwlr_foreign_toplevel_manager_v1 client: lists every open
// toplevel, optionally activates one by index, then exits. Built to
// reproduce srdwm's own aegis-reported bug (srd clients' `focused` field
// going stale after an activate-driven focus change) with a real protocol
// client instead of guessing from source reading alone.
//
// Usage:
//   activate            -- list toplevels with their index
//   activate <index>    -- activate the toplevel at that index, then list again

use std::collections::HashMap;

use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

#[derive(Default, Debug, Clone)]
struct ToplevelInfo {
    title: String,
    app_id: String,
    activated: bool,
}

struct State {
    seat: Option<WlSeat>,
    manager: Option<ZwlrForeignToplevelManagerV1>,
    toplevels: HashMap<u32, (ZwlrForeignToplevelHandleV1, ToplevelInfo)>,
    next_index: u32,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            if interface == "wl_seat" {
                state.seat = Some(registry.bind::<WlSeat, _, _>(name, version.min(9), qh, ()));
            } else if interface == "zwlr_foreign_toplevel_manager_v1" {
                state.manager = Some(registry.bind::<ZwlrForeignToplevelManagerV1, _, _>(name, version.min(3), qh, ()));
            }
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(_: &mut Self, _: &WlSeat, _: wayland_client::protocol::wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn event_created_child(opcode: u16, qh: &QueueHandle<Self>) -> std::sync::Arc<dyn wayland_client::backend::ObjectData> {
        // Opcode 0 is the `toplevel` event, whose one argument is a new
        // `zwlr_foreign_toplevel_handle_v1` object - wayland-client needs
        // to know the object-data for that new id *before* the event
        // itself is dispatched, which is what this override provides.
        match opcode {
            0 => qh.make_data::<ZwlrForeignToplevelHandleV1, ()>(()),
            _ => panic!("unexpected new-object event opcode {opcode} on zwlr_foreign_toplevel_manager_v1"),
        }
    }

    fn event(state: &mut Self, _: &ZwlrForeignToplevelManagerV1, event: zwlr_foreign_toplevel_manager_v1::Event, _: &(), _: &Connection, _qh: &QueueHandle<Self>) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            let idx = state.next_index;
            state.next_index += 1;
            // `toplevel` (the new handle) already has its object data set
            // by `event_created_child` above by the time this event fires
            // - events for it just get matched back to our own map by
            // proxy equality in the handle `Dispatch` impl below, rather
            // than needing the object data itself to carry the index.
            state.toplevels.insert(idx, (toplevel, ToplevelInfo::default()));
        }
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(state: &mut Self, proxy: &ZwlrForeignToplevelHandleV1, event: zwlr_foreign_toplevel_handle_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let Some((_, info)) = state.toplevels.values_mut().find(|(h, _)| *h == *proxy) else { return };
        match event {
            zwlr_foreign_toplevel_handle_v1::Event::Title { title } => info.title = title,
            zwlr_foreign_toplevel_handle_v1::Event::AppId { app_id } => info.app_id = app_id,
            zwlr_foreign_toplevel_handle_v1::Event::State { state: bytes } => {
                // Each state entry is a native-endian u32; 2 == Activated per
                // the protocol's own enum (Maximized=0, Minimized=1,
                // Activated=2, Fullscreen=3).
                info.activated = bytes.chunks_exact(4).any(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]) == 2);
            }
            _ => {}
        }
    }
}

fn main() {
    let conn = Connection::connect_to_env().expect("failed to connect to the Wayland compositor - is WAYLAND_DISPLAY set?");
    let display = conn.display();
    let mut event_queue: EventQueue<State> = conn.new_event_queue();
    let qh = event_queue.handle();
    let _registry = display.get_registry(&qh, ());

    let mut state = State { seat: None, manager: None, toplevels: HashMap::new(), next_index: 0 };
    event_queue.roundtrip(&mut state).expect("initial roundtrip failed");
    // A second roundtrip: title/app_id/state/done events for each toplevel
    // arrive in a batch right after the manager announces it, not
    // necessarily inside the same roundtrip that discovered the manager.
    event_queue.roundtrip(&mut state).expect("second roundtrip failed");

    if state.manager.is_none() {
        eprintln!("compositor does not advertise zwlr_foreign_toplevel_manager_v1");
        std::process::exit(1);
    }
    let Some(seat) = state.seat.clone() else {
        eprintln!("compositor does not advertise wl_seat");
        std::process::exit(1);
    };

    let arg = std::env::args().nth(1);
    if let Some(idx_str) = arg {
        let idx: u32 = idx_str.parse().expect("argument must be a toplevel index (see the no-argument listing)");
        let Some((handle, info)) = state.toplevels.get(&idx) else {
            eprintln!("no toplevel with index {idx}");
            std::process::exit(1);
        };
        println!("activating [{idx}] app_id={:?} title={:?}", info.app_id, info.title);
        handle.activate(&seat);
        event_queue.roundtrip(&mut state).expect("activate roundtrip failed");
        println!("activate() call sent and flushed.");
    }

    println!("-- toplevels --");
    let mut entries: Vec<_> = state.toplevels.iter().collect();
    entries.sort_by_key(|(idx, _)| **idx);
    for (idx, (_, info)) in entries {
        println!("[{idx}] app_id={:?} title={:?} activated={}", info.app_id, info.title, info.activated);
    }
}
