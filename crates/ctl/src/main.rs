//! Control CLI for srdwm's control socket (`crates/platform/src/ipc.rs`,
//! shared by every backend that has one). Deliberately dumb: builds one
//! request line from argv, writes it, reads the response(s), prints them.
//! All real logic - and all JSON encoding - lives server-side; this
//! stays a thin pipe so it never needs a JSON dependency of its own.
//!
//! Usage:
//!   srd clients                        list windows, one JSON object
//!   srd subscribe                      like `clients`, then one JSON
//!                                       object per line forever, each time
//!                                       the window list actually changes
//!   srd dispatch toggle_visibility ID  hide/show a window
//!   srd dispatch focus ID
//!   srd dispatch close ID
//!
//! The socket is Unix-domain; on platforms without one this always fails
//! cleanly rather than not building at all, since it's one binary in a
//! workspace that otherwise targets Windows and macOS too.

#[cfg(unix)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let request = match build_request(&args) {
        Ok(r) => r,
        Err(msg) => {
            eprintln!("srd: {msg}");
            print_usage();
            std::process::exit(2);
        }
    };

    let result = if args.first().map(String::as_str) == Some("subscribe") { unix::stream(&request) } else { unix::send(&request).map(|r| println!("{r}")) };
    if let Err(e) = result {
        eprintln!("srd: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("srd: srdwm's control socket is Unix-only; nothing to connect to on this platform");
    std::process::exit(1);
}

fn build_request(args: &[String]) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("clients") => Ok(r#"{"cmd":"clients"}"#.to_string()),
        Some("monitors") => Ok(r#"{"cmd":"monitors"}"#.to_string()),
        Some("subscribe") => Ok(r#"{"cmd":"subscribe"}"#.to_string()),
        Some("dispatch") => {
            let action = args.get(1).ok_or("dispatch needs an action (toggle_visibility/focus/close)")?;
            if !matches!(action.as_str(), "toggle_visibility" | "focus" | "close") {
                return Err(format!("unknown dispatch action '{action}'"));
            }
            let id: u64 = args.get(2).ok_or("dispatch needs a window id")?.parse().map_err(|_| "window id must be a number".to_string())?;
            Ok(format!(r#"{{"cmd":"{action}","id":{id}}}"#))
        }
        _ => Err("expected 'clients', 'monitors', 'subscribe', or 'dispatch <action> <id>'".to_string()),
    }
}

fn print_usage() {
    eprintln!("usage:");
    eprintln!("  srd clients");
    eprintln!("  srd monitors");
    eprintln!("  srd subscribe");
    eprintln!("  srd dispatch toggle_visibility <id>");
    eprintln!("  srd dispatch focus <id>");
    eprintln!("  srd dispatch close <id>");
}

#[cfg(unix)]
mod unix {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;

    /// `<display>` in `srdwm-<display>.sock` is whichever env var the
    /// compositor itself used to name it: `WAYLAND_DISPLAY` (Wayland
    /// backend) or `DISPLAY` (X11 backend) - see `srdwm_platform::
    /// IpcServer::bind`'s callers in `crates/wayland`/`crates/x11` for the
    /// matching naming choice on the server side. Reuses `srdwm_platform::
    /// detect()` rather than re-deriving the same Wayland-vs-X11 decision
    /// a second, potentially-drifting way here.
    fn socket_path() -> Result<PathBuf, String> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).ok_or("XDG_RUNTIME_DIR is not set")?;
        let display = match srdwm_platform::detect() {
            srdwm_platform::PlatformKind::X11 => std::env::var("DISPLAY").map_err(|_| "DISPLAY is not set".to_string())?,
            _ => std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_string()),
        };
        Ok(dir.join(format!("srdwm-{display}.sock")))
    }

    fn connect() -> Result<UnixStream, String> {
        let path = socket_path()?;
        UnixStream::connect(&path)
            .map_err(|e| format!("can't reach srdwm's control socket at {} ({e}) - is srdwm running, and are WAYLAND_DISPLAY/DISPLAY set correctly?", path.display()))
    }

    pub fn send(request: &str) -> Result<String, String> {
        let mut stream = connect()?;
        stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
        stream.write_all(b"\n").map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).map_err(|e| e.to_string())?;
        Ok(line.trim_end().to_string())
    }

    /// `subscribe`'s connection: unlike `send`, this never closes on its
    /// own - the server keeps it open and writes one more line every time
    /// the window list changes, so this prints each one as it arrives
    /// until the process is killed or the compositor closes the socket.
    pub fn stream(request: &str) -> Result<(), String> {
        let mut conn = connect()?;
        conn.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
        conn.write_all(b"\n").map_err(|e| e.to_string())?;
        let reader = BufReader::new(conn);
        for line in reader.lines() {
            match line {
                Ok(line) => println!("{line}"),
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
}
