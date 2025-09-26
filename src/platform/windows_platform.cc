#include "windows_platform.h"
#include <iostream>
#include <windows.h>
#include <dwmapi.h>

// Static member initialization
SRDWindowsPlatform* SRDWindowsPlatform::instance_ = nullptr;

SRDWindowsPlatform::SRDWindowsPlatform()
    : h_instance_(nullptr)
    , keyboard_hook_(nullptr)
    , mouse_hook_(nullptr) {
    
    instance_ = this;
}

SRDWindowsPlatform::~SRDWindowsPlatform() {
    shutdown();
    if (instance_ == this) {
        instance_ = nullptr;
    }
}

bool SRDWindowsPlatform::initialize() {
    std::cout << "Initializing SRDWindows platform..." << std::endl;
    
    // Get module handle
    h_instance_ = GetModuleHandle(nullptr);
    if (!h_instance_) {
        std::cerr << "Failed to get module handle" << std::endl;
        return false;
    }
    
    // Register window class
    if (!register_window_class()) {
        std::cerr << "Failed to register window class" << std::endl;
        return false;
    }
    
    // Setup global hooks
    setup_global_hooks();
    
    std::cout << "SRDWindows platform initialized successfully" << std::endl;
    return true;
}

void SRDWindowsPlatform::shutdown() {
    std::cout << "Shutting down SRDWindows platform..." << std::endl;
    
    // Unhook global hooks
    if (keyboard_hook_) {
        UnhookSRDWindowsHookEx(keyboard_hook_);
        keyboard_hook_ = nullptr;
    }
    
    if (mouse_hook_) {
        UnhookSRDWindowsHookEx(mouse_hook_);
        mouse_hook_ = nullptr;
    }
    
    // Clean up windows
    for (auto& pair : window_map_) {
        if (pair.second) {
            delete pair.second;
        }
    }
    window_map_.clear();
    
    std::cout << "SRDWindows platform shutdown complete" << std::endl;
}

// SRDWindow decoration implementations
void SRDWindowsPlatform::set_window_decorations(SRDWindow* window, bool enabled) {
    std::cout << "SRDWindowsPlatform: Set window decorations " << (enabled ? "enabled" : "disabled") << std::endl;
    
    if (!window) return;
    
    // Find the HWND for this window
    HWND hwnd = nullptr;
    for (const auto& pair : window_map_) {
        if (pair.second == window) {
            hwnd = pair.first;
            break;
        }
    }
    
    if (!hwnd) return;
    
    decorations_enabled_ = enabled;
    
    if (enabled) {
        // Enable native window decorations
        LONG style = GetSRDWindowLong(hwnd, GWL_STYLE);
        style |= WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
        SetSRDWindowLong(hwnd, GWL_STYLE, style);
    } else {
        // Disable native window decorations
        LONG style = GetSRDWindowLong(hwnd, GWL_STYLE);
        style &= ~(WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX);
        SetSRDWindowLong(hwnd, GWL_STYLE, style);
    }
    
    // Force window redraw
    SetSRDWindowPos(hwnd, nullptr, 0, 0, 0, 0, 
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
}

void SRDWindowsPlatform::set_window_border_color(SRDWindow* window, int r, int g, int b) {
    std::cout << "SRDWindowsPlatform: Set border color RGB(" << r << "," << g << "," << b << ")" << std::endl;
    
    if (!window) return;
    
    // Find the HWND for this window
    HWND hwnd = nullptr;
    for (const auto& pair : window_map_) {
        if (pair.second == window) {
            hwnd = pair.first;
            break;
        }
    }
    
    if (!hwnd) return;
    
    COLORREF color = RGB(r, g, b);
    apply_dwm_border_color(hwnd, r, g, b);
}

void SRDWindowsPlatform::set_window_border_width(SRDWindow* window, int width) {
    std::cout << "SRDWindowsPlatform: Set border width " << width << std::endl;
    
    if (!window) return;
    
    border_width_ = width;
    
    // SRDWindows doesn't support custom border widths via DWM
    // This would need to be implemented with custom window frames
}

bool SRDWindowsPlatform::get_window_decorations(SRDWindow* window) const {
    if (!window) return false;
    
    return decorations_enabled_;
}

void SRDWindowsPlatform::apply_dwm_border_color(HWND hwnd, int r, int g, int b) {
    // Use DWM API to set border color (SRDWindows 11 feature)
    COLORREF color = RGB(r, g, b);
    
    HRESULT result = DwmSetSRDWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &color, sizeof(color));
    if (FAILED(result)) {
        std::cerr << "Failed to set DWM border color" << std::endl;
    }
}

void SRDWindowsPlatform::remove_dwm_border_color(HWND hwnd) {
    // Remove custom border color
    COLORREF color = DWMWA_COLOR_DEFAULT;
    
    HRESULT result = DwmSetSRDWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &color, sizeof(color));
    if (FAILED(result)) {
        std::cerr << "Failed to remove DWM border color" << std::endl;
    }
}

bool SRDWindowsPlatform::register_window_class() {
    WNDCLASSEX wc = {};
    wc.cbSize = sizeof(WNDCLASSEX);
    wc.lpfnWndProc = window_proc;
    wc.hInstance = h_instance_;
    wc.lpszClassName = L"SRDWM_SRDWindow";
    wc.hCursor = LoadCursor(nullptr, IDC_ARROW);
    wc.hbrBackground = (HBRUSH)(COLOR_WINDOW + 1);
    wc.style = CS_HREDRAW | CS_VREDRAW;
    
    return RegisterClassEx(&wc) != 0;
}

void SRDWindowsPlatform::setup_global_hooks() {
    std::cout << "Setting up global hooks..." << std::endl;
    
    // Global keyboard hook
    keyboard_hook_ = SetSRDWindowsHookEx(WH_KEYBOARD_LL, 
        keyboard_proc, h_instance_, 0);
    if (!keyboard_hook_) {
        std::cerr << "Failed to set keyboard hook" << std::endl;
    }
    
    // Global mouse hook
    mouse_hook_ = SetSRDWindowsHookEx(WH_MOUSE_LL, 
        mouse_proc, h_instance_, 0);
    if (!mouse_hook_) {
        std::cerr << "Failed to set mouse hook" << std::endl;
    }
    
    std::cout << "Global hooks setup complete" << std::endl;
}

LRESULT CALLBACK SRDWindowsPlatform::window_proc(HWND hwnd, UINT msg, 
                                             WPARAM wparam, LPARAM lparam) {
    switch (msg) {
        case WM_CREATE:
            std::cout << "SRDWindow created: " << hwnd << std::endl;
            break;
            
        case WM_DESTROY:
            std::cout << "SRDWindow destroyed: " << hwnd << std::endl;
            break;
            
        case WM_SIZE:
            // Handle window resizing
            break;
            
        case WM_MOVE:
            // Handle window moving
            break;
            
        case WM_SETFOCUS:
            // Handle window focus
            break;
            
        case WM_KILLFOCUS:
            // Handle window unfocus
            break;
            
        case WM_CLOSE:
            // Handle window close request
            break;
            
        default:
            return DefSRDWindowProc(hwnd, msg, wparam, lparam);
    }
    return 0;
}

LRESULT CALLBACK SRDWindowsPlatform::keyboard_proc(int nCode, WPARAM wparam, LPARAM lparam) {
    if (nCode >= 0) {
        KBDLLHOOKSTRUCT* kbhs = (KBDLLHOOKSTRUCT*)lparam;
        
        if (wparam == WM_KEYDOWN || wparam == WM_SYSKEYDOWN) {
            std::cout << "Key pressed: " << kbhs->vkCode << std::endl;
        } else if (wparam == WM_KEYUP || wparam == WM_SYSKEYUP) {
            std::cout << "Key released: " << kbhs->vkCode << std::endl;
        }
    }
    return CallNextHookEx(nullptr, nCode, wparam, lparam);
}

LRESULT CALLBACK SRDWindowsPlatform::mouse_proc(int nCode, WPARAM wparam, LPARAM lparam) {
    if (nCode >= 0) {
        MSLLHOOKSTRUCT* mhs = (MSLLHOOKSTRUCT*)lparam;
        
        switch (wparam) {
            case WM_LBUTTONDOWN:
                std::cout << "Left mouse button down at (" << mhs->pt.x << ", " << mhs->pt.y << ")" << std::endl;
                break;
                
            case WM_LBUTTONUP:
                std::cout << "Left mouse button up at (" << mhs->pt.x << ", " << mhs->pt.y << ")" << std::endl;
                break;
                
            case WM_RBUTTONDOWN:
                std::cout << "Right mouse button down at (" << mhs->pt.x << ", " << mhs->pt.y << ")" << std::endl;
                break;
                
            case WM_RBUTTONUP:
                std::cout << "Right mouse button up at (" << mhs->pt.x << ", " << mhs->pt.y << ")" << std::endl;
                break;
                
            case WM_MOUSEMOVE:
                // Handle mouse movement
                break;
        }
    }
    return CallNextHookEx(nullptr, nCode, wparam, lparam);
}

void SRDWindowsPlatform::convert_win32_message(UINT msg, WPARAM wparam, LPARAM lparam, std::vector<Event>& events) {
    Event event;
    
    switch (msg) {
        case WM_CREATE:
            event.type = EventType::SRDWindowCreated;
            break;
        case WM_DESTROY:
            event.type = EventType::SRDWindowDestroyed;
            break;
        case WM_MOVE:
            event.type = EventType::SRDWindowMoved;
            break;
        case WM_SIZE:
            event.type = EventType::SRDWindowResized;
            break;
        case WM_SETFOCUS:
            event.type = EventType::SRDWindowFocused;
            break;
        case WM_KILLFOCUS:
            event.type = EventType::SRDWindowUnfocused;
            break;
        case WM_KEYDOWN:
            event.type = EventType::KeyPress;
            break;
        case WM_KEYUP:
            event.type = EventType::KeyRelease;
            break;
        case WM_LBUTTONDOWN:
        case WM_RBUTTONDOWN:
        case WM_MBUTTONDOWN:
            event.type = EventType::MouseButtonPress;
            break;
        case WM_LBUTTONUP:
        case WM_RBUTTONUP:
        case WM_MBUTTONUP:
            event.type = EventType::MouseButtonRelease;
            break;
        case WM_MOUSEMOVE:
            event.type = EventType::MouseMotion;
            break;
        default:
            return; // Skip unknown messages
    }
    
    event.data = nullptr;
    event.data_size = 0;
    events.push_back(event);
}

bool SRDWindowsPlatform::poll_events(std::vector<Event>& events) {
    if (!initialized_) return false;
    
    events.clear();
    
    // Process SRDWindows messages
    MSG msg;
    while (PeekMessage(&msg, nullptr, 0, 0, PM_REMOVE)) {
        TranslateMessage(&msg);
        DispatchMessage(&msg);
        
        // Convert SRDWindows message to SRDWM event
        convert_win32_message(msg.message, msg.wParam, msg.lParam, events);
    }
    
    return !events.empty();
}

void SRDWindowsPlatform::process_event(const Event& event) {
    // TODO: Implement event processing
}

std::unique_ptr<SRDWindow> SRDWindowsPlatform::create_window(const std::string& title, int x, int y, int width, int height) {
    // Convert title to wide string
    int title_len = MultiByteToWideChar(CP_UTF8, 0, title.c_str(), -1, nullptr, 0);
    std::wstring wtitle(title_len, 0);
    MultiByteToWideChar(CP_UTF8, 0, title.c_str(), -1, &wtitle[0], title_len);
    
    // Create window
    HWND hwnd = CreateSRDWindowEx(
        WS_EX_OVERLAPPEDWINDOW,
        L"SRDWM_SRDWindow",
        wtitle.c_str(),
        WS_OVERLAPPEDWINDOW,
        x, y, width, height,
        nullptr, nullptr, h_instance_, nullptr
    );
    
    if (!hwnd) {
        std::cerr << "Failed to create SRDWindows window" << std::endl;
        return nullptr;
    }
    
    // Create SRDWM window object
    auto window = std::make_unique<SRDWindow>();
    // TODO: Set window properties
    
    // Store window mapping
    window_map_[hwnd] = window.get();
    
    std::cout << "Created SRDWindows window: " << hwnd << " (" << title << ")" << std::endl;
    return window;
}

void SRDWindowsPlatform::destroy_window(SRDWindow* window) {
    // Find window handle
    for (auto& pair : window_map_) {
        if (pair.second == window) {
            DestroySRDWindow(pair.first);
            window_map_.erase(pair.first);
            break;
        }
    }
}

void SRDWindowsPlatform::set_window_position(SRDWindow* window, int x, int y) {
    // Find window handle
    for (auto& pair : window_map_) {
        if (pair.second == window) {
            SetSRDWindowPos(pair.first, nullptr, x, y, 0, 0, 
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            break;
        }
    }
}

void SRDWindowsPlatform::set_window_size(SRDWindow* window, int width, int height) {
    // Find window handle
    for (auto& pair : window_map_) {
        if (pair.second == window) {
            SetSRDWindowPos(pair.first, nullptr, 0, 0, width, height, 
                        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            break;
        }
    }
}

void SRDWindowsPlatform::set_window_title(SRDWindow* window, const std::string& title) {
    // Find window handle
    for (auto& pair : window_map_) {
        if (pair.second == window) {
            // Convert title to wide string
            int title_len = MultiByteToWideChar(CP_UTF8, 0, title.c_str(), -1, nullptr, 0);
            std::wstring wtitle(title_len, 0);
            MultiByteToWideChar(CP_UTF8, 0, title.c_str(), -1, &wtitle[0], title_len);
            
            SetSRDWindowText(pair.first, wtitle.c_str());
            break;
        }
    }
}

void SRDWindowsPlatform::focus_window(SRDWindow* window) {
    // Find window handle
    for (auto& pair : window_map_) {
        if (pair.second == window) {
            SetForegroundSRDWindow(pair.first);
            SetFocus(pair.first);
            break;
        }
    }
}

void SRDWindowsPlatform::minimize_window(SRDWindow* window) {
    // Find window handle
    for (auto& pair : window_map_) {
        if (pair.second == window) {
            ShowSRDWindow(pair.first, SW_MINIMIZE);
            break;
        }
    }
}

void SRDWindowsPlatform::maximize_window(SRDWindow* window) {
    // Find window handle
    for (auto& pair : window_map_) {
        if (pair.second == window) {
            ShowSRDWindow(pair.first, SW_MAXIMIZE);
            break;
        }
    }
}

void SRDWindowsPlatform::close_window(SRDWindow* window) {
    // Find window handle
    for (auto& pair : window_map_) {
        if (pair.second == window) {
            PostMessage(pair.first, WM_CLOSE, 0, 0);
            break;
        }
    }
}

std::vector<Monitor> SRDWindowsPlatform::get_monitors() {
    std::vector<Monitor> monitors;
    
    // Enumerate monitors using EnumDisplayMonitors
    EnumDisplayMonitors(nullptr, nullptr, 
        [](HMONITOR hMonitor, HDC hdcMonitor, LPRECT lprcMonitor, LPARAM dwData) -> BOOL {
            auto* monitors = reinterpret_cast<std::vector<Monitor>*>(dwData);
            
            MONITORINFOEX monitorInfo;
            monitorInfo.cbSize = sizeof(MONITORINFOEX);
            GetMonitorInfo(hMonitor, &monitorInfo);
            
            Monitor monitor;
            monitor.id = reinterpret_cast<int>(hMonitor);
            monitor.name = std::string(monitorInfo.szDevice);
            monitor.x = monitorInfo.rcMonitor.left;
            monitor.y = monitorInfo.rcMonitor.top;
            monitor.width = monitorInfo.rcMonitor.right - monitorInfo.rcMonitor.left;
            monitor.height = monitorInfo.rcMonitor.bottom - monitorInfo.rcMonitor.top;
            monitor.refresh_rate = 60; // TODO: Get actual refresh rate
            
            monitors->push_back(monitor);
            
            return TRUE;
        }, reinterpret_cast<LPARAM>(&monitors));
    
    return monitors;
}

Monitor SRDWindowsPlatform::get_primary_monitor() {
    auto monitors = get_monitors();
    if (!monitors.empty()) {
        return monitors[0];
    }
    
    // Fallback to primary display
    Monitor monitor;
    monitor.id = 0;
    monitor.name = "Primary Display";
    monitor.x = 0;
    monitor.y = 0;
    monitor.width = GetSystemMetrics(SM_CXSCREEN);
    monitor.height = GetSystemMetrics(SM_CYSCREEN);
    monitor.refresh_rate = 60;
    
    return monitor;
}

void SRDWindowsPlatform::grab_keyboard() {
    // TODO: Implement keyboard grabbing
    std::cout << "Keyboard grabbing setup" << std::endl;
}

void SRDWindowsPlatform::ungrab_keyboard() {
    // TODO: Implement keyboard ungrab
    std::cout << "Keyboard ungrab" << std::endl;
}

void SRDWindowsPlatform::grab_pointer() {
    // TODO: Implement pointer grabbing
    std::cout << "Pointer grabbing setup" << std::endl;
}

void SRDWindowsPlatform::ungrab_pointer() {
    // TODO: Implement pointer ungrab
    std::cout << "Pointer ungrab" << std::endl;
}

void SRDWindowsPlatform::convert_windows_message_to_event(const MSG& msg, std::vector<Event>& events) {
    // TODO: Convert SRDWindows messages to SRDWM events
    switch (msg.message) {
        case WM_KEYDOWN:
        case WM_SYSKEYDOWN:
            // Convert to key press event
            break;
            
        case WM_KEYUP:
        case WM_SYSKEYUP:
            // Convert to key release event
            break;
            
        case WM_LBUTTONDOWN:
        case WM_RBUTTONDOWN:
        case WM_MBUTTONDOWN:
            // Convert to button press event
            break;
            
        case WM_LBUTTONUP:
        case WM_RBUTTONUP:
        case WM_MBUTTONUP:
            // Convert to button release event
            break;
            
        case WM_MOUSEMOVE:
            // Convert to mouse motion event
            break;
            
        case WM_SIZE:
            // Convert to window resize event
            break;
            
        case WM_MOVE:
            // Convert to window move event
            break;
    }
}

void SRDWindowsPlatform::handle_global_keyboard(WPARAM wparam, KBDLLHOOKSTRUCT* kbhs) {
    // TODO: Implement global keyboard handling
}

void SRDWindowsPlatform::handle_global_mouse(WPARAM wparam, MSLLHOOKSTRUCT* mhs) {
    // TODO: Implement global mouse handling
}


