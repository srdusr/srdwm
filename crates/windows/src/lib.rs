//! Windows backend for srdwm.
//!
//! Only the `cfg(windows)` code paths are real; this crate is still included
//! in the workspace on Linux/macOS so it type-checks continuously, but on
//! non-Windows targets every `Platform` method returns
//! `PlatformError::Unsupported` rather than pretending to work - this crate
//! has never been built or run on an actual Windows machine, unlike the X11
//! backend.
//!
//! Porting notes (see docs/PRIOR_ART.md for the full legacy-C++ digest this
//! is based on): the legacy `windows_platform.cc` had real, working
//! `DwmSetWindowAttribute(DWMWA_BORDER_COLOR, ...)` border tinting (Windows
//! 11+ only), low-level `WH_KEYBOARD_LL`/`WH_MOUSE_LL` hooks, and
//! `EnumDisplayMonitors` monitor enumeration - those are the pieces
//! reimplemented for real below (behind `cfg(windows)`). It had no window
//! subclassing, no virtual-desktop support (would need the undocumented COM
//! `IVirtualDesktopManager` interface), and no custom title bar - DWM does
//! not support custom border widths or a fully custom frame without
//! disabling the native one, so for feature parity with the X11/Wayland
//! "full title bar" look, srdwm on Windows should render its own decoration
//! via a borderless (`WS_POPUP`) window plus manual hit-testing (same
//! `srdwm_core::window::ResizeEdge::hit_test` used by the other backends),
//! rather than trying to theme the native frame. That's flagged as TODO here
//! rather than implemented, since it can't be visually verified without a
//! Windows machine.

use srdwm_core::{Monitor, Rect, Window, WindowId};
use srdwm_platform::{Platform, PlatformError, PlatformKind, Result};

pub struct WindowsPlatform {
    #[cfg(windows)]
    windows: std::collections::HashMap<WindowId, windows::Win32::Foundation::HWND>,
}

impl WindowsPlatform {
    pub fn new() -> Result<Self> {
        #[cfg(windows)]
        {
            Ok(Self { windows: Default::default() })
        }
        #[cfg(not(windows))]
        {
            Err(PlatformError::Unsupported("srdwm-windows was not compiled for a Windows target"))
        }
    }
}

impl Platform for WindowsPlatform {
    fn kind(&self) -> PlatformKind {
        PlatformKind::Windows
    }

    #[cfg(windows)]
    fn poll_events(&mut self) -> Result<Vec<srdwm_core::Event>> {
        // TODO: real PeekMessageW/TranslateMessage/DispatchMessage pump,
        // mapping WM_* to srdwm_core::Event as the legacy backend did.
        Ok(Vec::new())
    }
    #[cfg(not(windows))]
    fn poll_events(&mut self) -> Result<Vec<srdwm_core::Event>> {
        Err(PlatformError::Unsupported("poll_events"))
    }

    #[cfg(windows)]
    fn monitors(&mut self) -> Result<Vec<Monitor>> {
        use windows::Win32::Graphics::Gdi::{
            EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
        };
        use windows::Win32::Foundation::{BOOL, LPARAM, RECT};

        unsafe extern "system" fn callback(
            hmonitor: HMONITOR,
            _hdc: HDC,
            _rect: *mut RECT,
            lparam: LPARAM,
        ) -> BOOL {
            let out = &mut *(lparam.0 as *mut Vec<Monitor>);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(hmonitor, &mut info).as_bool() {
                let r = info.rcMonitor;
                out.push(Monitor::new(
                    out.len() as u32,
                    format!("monitor-{}", out.len()),
                    Rect::new(r.left, r.top, (r.right - r.left) as u32, (r.bottom - r.top) as u32),
                ));
            }
            BOOL(1)
        }

        let mut monitors: Vec<Monitor> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(callback), LPARAM(&mut monitors as *mut _ as isize));
        }
        Ok(monitors)
    }
    #[cfg(not(windows))]
    fn monitors(&mut self) -> Result<Vec<Monitor>> {
        Err(PlatformError::Unsupported("monitors"))
    }

    #[cfg(windows)]
    fn apply_geometry(&mut self, window: WindowId, geometry: Rect) -> Result<()> {
        use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOZORDER};
        if let Some(&hwnd) = self.windows.get(&window) {
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    geometry.x,
                    geometry.y,
                    geometry.width as i32,
                    geometry.height as i32,
                    SWP_NOZORDER,
                );
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    fn apply_geometry(&mut self, _window: WindowId, _geometry: Rect) -> Result<()> {
        Err(PlatformError::Unsupported("apply_geometry"))
    }

    #[cfg(windows)]
    fn set_title(&mut self, window: WindowId, title: &str) -> Result<()> {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::SetWindowTextW;
        if let Some(&hwnd) = self.windows.get(&window) {
            unsafe {
                let _ = SetWindowTextW(hwnd, &HSTRING::from(title));
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    fn set_title(&mut self, _window: WindowId, _title: &str) -> Result<()> {
        Err(PlatformError::Unsupported("set_title"))
    }

    #[cfg(windows)]
    fn focus(&mut self, window: WindowId) -> Result<()> {
        use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
        use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
        if let Some(&hwnd) = self.windows.get(&window) {
            unsafe {
                let _ = SetForegroundWindow(hwnd);
                let _ = SetFocus(Some(hwnd));
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    fn focus(&mut self, _window: WindowId) -> Result<()> {
        Err(PlatformError::Unsupported("focus"))
    }

    #[cfg(windows)]
    fn minimize(&mut self, window: WindowId) -> Result<()> {
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_MINIMIZE};
        if let Some(&hwnd) = self.windows.get(&window) {
            unsafe {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    fn minimize(&mut self, _window: WindowId) -> Result<()> {
        Err(PlatformError::Unsupported("minimize"))
    }

    #[cfg(windows)]
    fn restore(&mut self, window: WindowId) -> Result<()> {
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_RESTORE};
        if let Some(&hwnd) = self.windows.get(&window) {
            unsafe {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    fn restore(&mut self, _window: WindowId) -> Result<()> {
        Err(PlatformError::Unsupported("restore"))
    }

    #[cfg(windows)]
    fn close(&mut self, window: WindowId) -> Result<()> {
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
        if let Some(&hwnd) = self.windows.get(&window) {
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, windows::Win32::Foundation::WPARAM(0), windows::Win32::Foundation::LPARAM(0));
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    fn close(&mut self, _window: WindowId) -> Result<()> {
        Err(PlatformError::Unsupported("close"))
    }

    fn set_decorated(&mut self, _window: WindowId, _decorated: bool) -> Result<()> {
        // TODO: toggle WS_CAPTION|WS_SYSMENU|WS_MINIMIZEBOX|WS_MAXIMIZEBOX via
        // GetWindowLong/SetWindowLong + SWP_FRAMECHANGED, as the legacy
        // backend did.
        Err(PlatformError::Unsupported("set_decorated"))
    }

    #[cfg(windows)]
    fn set_border_color(&mut self, window: WindowId, rgb: (u8, u8, u8)) -> Result<()> {
        use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_BORDER_COLOR};
        if let Some(&hwnd) = self.windows.get(&window) {
            let colorref = (rgb.0 as u32) | ((rgb.1 as u32) << 8) | ((rgb.2 as u32) << 16);
            unsafe {
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_BORDER_COLOR,
                    &colorref as *const _ as *const std::ffi::c_void,
                    std::mem::size_of::<u32>() as u32,
                );
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    fn set_border_color(&mut self, _window: WindowId, _rgb: (u8, u8, u8)) -> Result<()> {
        Err(PlatformError::Unsupported("set_border_color"))
    }

    fn set_border_width(&mut self, _window: WindowId, _width: u32) -> Result<()> {
        // DWM does not expose a border-width knob without replacing the
        // native frame entirely (see module docs).
        Err(PlatformError::Unsupported("set_border_width (DWM has no custom-width API)"))
    }

    fn redraw_decoration(&mut self, _window: WindowId, _win: &Window, _focused: bool) -> Result<()> {
        // Native DWM frame is used for now; see module docs for the
        // borderless-window plan to get X11/Wayland decoration parity.
        Ok(())
    }

    fn grab_keyboard(&mut self) -> Result<()> {
        // TODO: SetWindowsHookExW(WH_KEYBOARD_LL, ...) as the legacy backend did.
        Err(PlatformError::Unsupported("grab_keyboard"))
    }

    fn ungrab_keyboard(&mut self) -> Result<()> {
        Err(PlatformError::Unsupported("ungrab_keyboard"))
    }

    fn keyboard_layout(&mut self) -> Result<String> {
        Err(PlatformError::Unsupported("keyboard_layout"))
    }

    fn cycle_keyboard_layout(&mut self) -> Result<String> {
        Err(PlatformError::Unsupported("cycle_keyboard_layout"))
    }
}
