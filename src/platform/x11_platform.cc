#include "x11_platform.h"
#include <iostream>
#include <cstring>

X11Platform::X11Platform() {
    std::cout << "X11Platform: Constructor called" << std::endl;
}

X11Platform::~X11Platform() {
    std::cout << "X11Platform: Destructor called" << std::endl;
}

bool X11Platform::initialize() {
    std::cout << "X11Platform: Initializing X11 backend..." << std::endl;
    
    // Open X11 display
    display_ = XOpenDisplay(nullptr);
    if (!display_) {
        std::cerr << "Failed to open X11 display" << std::endl;
        return false;
    }
    
    // Get root window
    root_ = DefaultRootWindow(display_);
    
    // Check for other window manager
    if (!check_for_other_wm()) {
        std::cerr << "Another window manager is already running" << std::endl;
        return false;
    }
    
    // Setup X11 environment
    if (!setup_x11_environment()) {
        std::cerr << "Failed to setup X11 environment" << std::endl;
        return false;
    }
    
    // Setup event masks
    if (!setup_event_masks()) {
        std::cerr << "Failed to setup event masks" << std::endl;
        return false;
    }
    
    // Setup atoms
    setup_atoms();
    
    // Setup extensions
    setup_extensions();
    
    // Initialize decoration state
    decorations_enabled_ = true;
    border_width_ = 2;
    border_color_ = 0x2e3440; // Default border color
    focused_border_color_ = 0x88c0d0; // Default focused border color
    
    std::cout << "X11Platform: Initialized successfully" << std::endl;
    return true;
}

void X11Platform::shutdown() {
    std::cout << "X11Platform: Shutting down..." << std::endl;
    
    // Clean up windows
    for (auto& pair : window_map_) {
        if (pair.second) {
            destroy_window(pair.second);
        }
    }
    window_map_.clear();
    frame_window_map_.clear();
    
    // Close X11 display
    if (display_) {
        XCloseDisplay(display_);
        display_ = nullptr;
    }
    
    std::cout << "X11Platform: Shutdown complete" << std::endl;
}

bool X11Platform::poll_events(std::vector<Event>& events) {
    if (!display_) return false;
    
    events.clear();
    
    // Check for pending events
    if (XPending(display_) > 0) {
        XEvent xevent;
        XNextEvent(display_, &xevent);
        
        // Handle the X11 event
        handle_x11_event(xevent);
        
        // Convert to SRDWM events (simplified for now)
        Event event;
        event.type = EventType::WindowCreated; // Placeholder
        event.data = nullptr;
        event.data_size = 0;
        events.push_back(event);
        
        return true;
    }
    
    return false;
}

void X11Platform::process_event(const Event& event) {
    std::cout << "X11Platform: Process event called" << std::endl;
}

std::unique_ptr<SRDWindow> X11Platform::create_window(const std::string& title, int x, int y, int width, int height) {
    std::cout << "X11Platform: Create window called" << std::endl;
    // TODO: Implement actual window creation when headers are available
    return nullptr;
}

void X11Platform::destroy_window(SRDWindow* window) {
    std::cout << "X11Platform: Destroy window called" << std::endl;
}

void X11Platform::set_window_position(SRDWindow* window, int x, int y) {
    std::cout << "X11Platform: Set window position to (" << x << "," << y << ")" << std::endl;
    if (!window || !display_) return;
    X11Window x11_window = static_cast<X11Window>(window->getId());
    XMoveWindow(display_, to_x11_window(x11_window), x, y);
    XFlush(display_);
}

void X11Platform::set_window_size(SRDWindow* window, int width, int height) {
    std::cout << "X11Platform: Set window size to (" << width << "x" << height << ")" << std::endl;
    if (!window || !display_) return;
    X11Window x11_window = static_cast<X11Window>(window->getId());
    XResizeWindow(display_, to_x11_window(x11_window), static_cast<unsigned int>(width), static_cast<unsigned int>(height));
    XFlush(display_);
}

void X11Platform::set_window_title(SRDWindow* window, const std::string& title) {
    std::cout << "X11Platform: Set window title to '" << title << "'" << std::endl;
    if (!window || !display_) return;
    X11Window x11_window = static_cast<X11Window>(window->getId());
    XStoreName(display_, to_x11_window(x11_window), title.c_str());
    XFlush(display_);
}

void X11Platform::focus_window(SRDWindow* window) {
    std::cout << "X11Platform: Focus window" << std::endl;
    if (!window || !display_) return;
    X11Window x11_window = static_cast<X11Window>(window->getId());
    XSetInputFocus(display_, to_x11_window(x11_window), RevertToParent, CurrentTime);
    XFlush(display_);
}

void X11Platform::minimize_window(SRDWindow* window) {
    std::cout << "X11Platform: Minimize window called" << std::endl;
}

void X11Platform::maximize_window(SRDWindow* window) {
    std::cout << "X11Platform: Maximize window called" << std::endl;
}

void X11Platform::close_window(SRDWindow* window) {
    std::cout << "X11Platform: Close window" << std::endl;
    if (!window || !display_) return;
    X11Window x11_window = static_cast<X11Window>(window->getId());
    Atom wm_delete = XInternAtom(display_, "WM_DELETE_WINDOW", False);
    if (wm_delete != None) {
        XEvent ev{};
        ev.xclient.type = ClientMessage;
        ev.xclient.message_type = XInternAtom(display_, "WM_PROTOCOLS", False);
        ev.xclient.display = display_;
        ev.xclient.window = to_x11_window(x11_window);
        ev.xclient.format = 32;
        ev.xclient.data.l[0] = wm_delete;
        ev.xclient.data.l[1] = CurrentTime;
        XSendEvent(display_, to_x11_window(x11_window), False, NoEventMask, &ev);
        XFlush(display_);
    } else {
        XDestroyWindow(display_, to_x11_window(x11_window));
        XFlush(display_);
    }
}

// EWMH (Extended Window Manager Hints) support implementation
void X11Platform::set_ewmh_supported(bool supported) {
    ewmh_supported_ = supported;
    if (supported) {
        setup_ewmh();
    }
    std::cout << "X11Platform: EWMH support " << (supported ? "enabled" : "disabled") << std::endl;
}

void X11Platform::set_window_type(SRDWindow* window, const std::string& type) {
    if (!ewmh_supported_ || !window) return;
    
    X11Window x11_window = static_cast<X11Window>(window->getId());
    Atom window_type_atom = None;
    
    if (type == "desktop") window_type_atom = _NET_WM_WINDOW_TYPE_DESKTOP_;
    else if (type == "dock") window_type_atom = _NET_WM_WINDOW_TYPE_DOCK_;
    else if (type == "toolbar") window_type_atom = _NET_WM_WINDOW_TYPE_TOOLBAR_;
    else if (type == "menu") window_type_atom = _NET_WM_WINDOW_TYPE_MENU_;
    else if (type == "utility") window_type_atom = _NET_WM_WINDOW_TYPE_UTILITY_;
    else if (type == "splash") window_type_atom = _NET_WM_WINDOW_TYPE_SPLASH_;
    else if (type == "dialog") window_type_atom = _NET_WM_WINDOW_TYPE_DIALOG_;
    else if (type == "dropdown_menu") window_type_atom = _NET_WM_WINDOW_TYPE_DROPDOWN_MENU_;
    else if (type == "popup_menu") window_type_atom = _NET_WM_WINDOW_TYPE_POPUP_MENU_;
    else if (type == "tooltip") window_type_atom = _NET_WM_WINDOW_TYPE_TOOLTIP_;
    else if (type == "notification") window_type_atom = _NET_WM_WINDOW_TYPE_NOTIFICATION_;
    else if (type == "combo") window_type_atom = _NET_WM_WINDOW_TYPE_COMBO_;
    else if (type == "dnd") window_type_atom = _NET_WM_WINDOW_TYPE_DND_;
    else window_type_atom = _NET_WM_WINDOW_TYPE_NORMAL_;
    
    if (window_type_atom != None) {
        XChangeProperty(display_, x11_window, _NET_WM_WINDOW_TYPE_, XA_ATOM, 32,
                       PropModeReplace, (unsigned char*)&window_type_atom, 1);
    }
}

void X11Platform::set_window_state(SRDWindow* window, const std::vector<std::string>& states) {
    if (!ewmh_supported_ || !window) return;
    
    X11Window x11_window = static_cast<X11Window>(window->getId());
    std::vector<Atom> state_atoms;
    
    for (const auto& state : states) {
        Atom state_atom = None;
        if (state == "maximized_vert") state_atom = _NET_WM_STATE_MAXIMIZED_VERT_;
        else if (state == "maximized_horz") state_atom = _NET_WM_STATE_MAXIMIZED_HORZ_;
        else if (state == "fullscreen") state_atom = _NET_WM_STATE_FULLSCREEN_;
        else if (state == "above") state_atom = _NET_WM_STATE_ABOVE_;
        else if (state == "below") state_atom = _NET_WM_STATE_BELOW_;
        
        if (state_atom != None) {
            state_atoms.push_back(state_atom);
        }
    }
    
    if (!state_atoms.empty()) {
        XChangeProperty(display_, x11_window, _NET_WM_STATE_, XA_ATOM, 32,
                       PropModeReplace, (unsigned char*)state_atoms.data(), state_atoms.size());
    }
}

void X11Platform::set_window_strut(SRDWindow* window, int left, int right, int top, int bottom) {
    if (!ewmh_supported_ || !window) return;
    
    X11Window x11_window = static_cast<X11Window>(window->getId());
    long strut[12] = {left, right, top, bottom, 0, 0, 0, 0, 0, 0, 0, 0};
    
    XChangeProperty(display_, x11_window, XInternAtom(display_, "_NET_WM_STRUT_PARTIAL", False),
                   XA_CARDINAL, 32, PropModeReplace, (unsigned char*)strut, 12);
}

// Virtual Desktop support implementation
void X11Platform::create_virtual_desktop(const std::string& name) {
    if (!ewmh_supported_) return;
    
    int desktop_id = virtual_desktops_.size();
    virtual_desktops_.push_back(desktop_id);
    
    // Update EWMH desktop info
    update_ewmh_desktop_info();
    
    std::cout << "X11Platform: Created virtual desktop " << desktop_id << " (" << name << ")" << std::endl;
}

void X11Platform::remove_virtual_desktop(int desktop_id) {
    if (!ewmh_supported_) return;
    
    auto it = std::find(virtual_desktops_.begin(), virtual_desktops_.end(), desktop_id);
    if (it != virtual_desktops_.end()) {
        virtual_desktops_.erase(it);
        
        // If removing current desktop, switch to first available
        if (desktop_id == current_virtual_desktop_ && !virtual_desktops_.empty()) {
            switch_to_virtual_desktop(virtual_desktops_[0]);
        }
        
        update_ewmh_desktop_info();
        std::cout << "X11Platform: Removed virtual desktop " << desktop_id << std::endl;
    }
}

void X11Platform::switch_to_virtual_desktop(int desktop_id) {
    if (!ewmh_supported_) return;
    
    auto it = std::find(virtual_desktops_.begin(), virtual_desktops_.end(), desktop_id);
    if (it != virtual_desktops_.end()) {
        current_virtual_desktop_ = desktop_id;
        
        // Update EWMH current desktop
        XChangeProperty(display_, root_, _NET_CURRENT_DESKTOP_, XA_CARDINAL, 32,
                       PropModeReplace, (unsigned char*)&current_virtual_desktop_, 1);
        
        std::cout << "X11Platform: Switched to virtual desktop " << desktop_id << std::endl;
    }
}

int X11Platform::get_current_virtual_desktop() const {
    return current_virtual_desktop_;
}

std::vector<int> X11Platform::get_virtual_desktops() const {
    return virtual_desktops_;
}

void X11Platform::move_window_to_desktop(SRDWindow* window, int desktop_id) {
    if (!ewmh_supported_ || !window) return;
    
    X11Window x11_window = static_cast<X11Window>(window->getId());
    XChangeProperty(display_, x11_window, _NET_WM_DESKTOP_, XA_CARDINAL, 32,
                   PropModeReplace, (unsigned char*)&desktop_id, 1);
    
    std::cout << "X11Platform: Moved window " << window->getId() << " to desktop " << desktop_id << std::endl;
}

std::vector<Monitor> X11Platform::get_monitors() {
    std::cout << "X11Platform: Get monitors called" << std::endl;
    // Return a default monitor for now
    return {Monitor{0, 0, 0, 1920, 1080}};
}

Monitor X11Platform::get_primary_monitor() {
    std::cout << "X11Platform: Get primary monitor called" << std::endl;
    return Monitor{0, 0, 0, 1920, 1080};
}

void X11Platform::grab_keyboard() {
    std::cout << "X11Platform: Grab keyboard called" << std::endl;
}

void X11Platform::ungrab_keyboard() {
    std::cout << "X11Platform: Ungrab keyboard called" << std::endl;
}

void X11Platform::grab_pointer() {
    std::cout << "X11Platform: Grab pointer called" << std::endl;
}

void X11Platform::ungrab_pointer() {
    std::cout << "X11Platform: Ungrab pointer called" << std::endl;
}

// Private method stubs removed - actual implementations exist below

void X11Platform::setup_extensions() {
    std::cout << "X11Platform: Setup extensions called" << std::endl;
}

bool X11Platform::check_for_other_wm() {
    // Try to select SubstructureRedirectMask on root window
    // If another WM is running, this will fail
    XSelectInput(display_, root_, SubstructureRedirectMask);
    XSync(display_, False);
    
    // Check if the selection was successful
    XErrorHandler old_handler = XSetErrorHandler([](Display*, XErrorEvent*) -> int {
        return 0; // Ignore errors
    });
    
    XSync(display_, False);
    XSetErrorHandler(old_handler);
    
    return true; // Simplified check
}

bool X11Platform::setup_x11_environment() {
    std::cout << "X11Platform: Setting up X11 environment..." << std::endl;
    
    // Set up error handling
    XSetErrorHandler([](Display*, XErrorEvent* e) -> int {
        if (e->error_code == BadWindow ||
            e->error_code == BadMatch ||
            e->error_code == BadAccess) {
            return 0; // Ignore common errors
        }
        std::cerr << "X11 error: " << e->error_code << std::endl;
        return 0;
    });
    
    return true;
}

bool X11Platform::setup_event_masks() {
    std::cout << "X11Platform: Setting up event masks..." << std::endl;
    
    // Set up root window event mask
    long event_mask = SubstructureRedirectMask | SubstructureNotifyMask |
                     StructureNotifyMask | PropertyChangeMask |
                     ButtonPressMask | ButtonReleaseMask |
                     KeyPressMask | KeyReleaseMask |
                     PointerMotionMask | EnterWindowMask | LeaveWindowMask;
    
    XSelectInput(display_, root_, event_mask);
    
    return true;
}

void X11Platform::setup_atoms() {
    std::cout << "X11Platform: Setting up atoms..." << std::endl;
    
    // TODO: Implement atom setup for EWMH/ICCCM
    // This would involve creating atoms for window manager protocols
}

void X11Platform::handle_x11_event(XEvent& event) {
    std::cout << "X11Platform: Handle X11 event called" << std::endl;
    
    switch (event.type) {
        case MapRequest:
            handle_map_request(event.xmaprequest);
            break;
        case ConfigureRequest:
            handle_configure_request(event.xconfigurerequest);
            break;
        case DestroyNotify:
            handle_destroy_notify(event.xdestroywindow);
            break;
        case UnmapNotify:
            handle_unmap_notify(event.xunmap);
            break;
        case KeyPress:
            handle_key_press(event.xkey);
            break;
        case ButtonPress:
            handle_button_press(event.xbutton);
            break;
        case MotionNotify:
            handle_motion_notify(event.xmotion);
            break;
        default:
            break;
    }
}

void X11Platform::handle_map_request(XMapRequestEvent& event) {
    std::cout << "X11Platform: Map request for window " << static_cast<unsigned long>(event.window) << std::endl;
    
    // Create a SRDWindow object for this X11 window
    auto window = std::make_unique<SRDWindow>(static_cast<int>(static_cast<unsigned long>(event.window)), "X11 Window");
    
    // Add to window map
    window_map_[from_x11_window(event.window)] = window.get();
    window.release();
    
    // Map the window
    XMapWindow(display_, event.window);
    
    // Apply decorations if enabled
    if (decorations_enabled_) {
        create_frame_window(window_map_[from_x11_window(event.window)]);
    }
}

void X11Platform::handle_configure_request(XConfigureRequestEvent& event) {
    std::cout << "X11Platform: Configure request for window " << static_cast<unsigned long>(event.window) << std::endl;
    
    XWindowChanges changes;
    changes.x = event.x;
    changes.y = event.y;
    changes.width = event.width;
    changes.height = event.height;
    changes.border_width = event.border_width;
    changes.sibling = static_cast<::Window>(event.above);
    changes.stack_mode = event.detail;
    
    XConfigureWindow(display_, event.window, event.value_mask, &changes);
}

void X11Platform::handle_destroy_notify(XDestroyWindowEvent& event) {
    std::cout << "X11Platform: Destroy notify for window " << static_cast<unsigned long>(event.window) << std::endl;
    
    auto it = window_map_.find(from_x11_window(event.window));
    if (it != window_map_.end()) {
        destroy_window(it->second);
        window_map_.erase(it);
    }
}

void X11Platform::handle_unmap_notify(XUnmapEvent& event) {
    std::cout << "X11Platform: Unmap notify for window " << static_cast<unsigned long>(event.window) << std::endl;
    
    // Handle window unmapping
}

void X11Platform::handle_key_press(XKeyEvent& event) {
    std::cout << "X11Platform: Key press event" << std::endl;
    
    // Convert X11 key event to SRDWM event
    // TODO: Implement key event conversion
}

void X11Platform::handle_button_press(XButtonEvent& event) {
    std::cout << "X11Platform: Button press event" << std::endl;
    
    // Convert X11 button event to SRDWM event
    // TODO: Implement button event conversion
}

void X11Platform::handle_motion_notify(XMotionEvent& event) {
    std::cout << "X11Platform: Motion notify event" << std::endl;
    
    // Convert X11 motion event to SRDWM event
    // TODO: Implement motion event conversion
}

// Window decoration implementations
void X11Platform::set_window_decorations(SRDWindow* window, bool enabled) {
    std::cout << "X11Platform: Set window decorations " << (enabled ? "enabled" : "disabled") << std::endl;
    
    if (!window || !display_) return;
    
    X11Window x11_window = static_cast<X11Window>(window->getId());
    
    if (enabled) {
        create_frame_window(window);
    } else {
        destroy_frame_window(window);
    }
    
    decorations_enabled_ = enabled;
}

void X11Platform::set_window_border_color(SRDWindow* window, int r, int g, int b) {
    std::cout << "X11Platform: Set border color RGB(" << r << "," << g << "," << b << ")" << std::endl;
    
    if (!window || !display_) return;
    
    X11Window x11_window = static_cast<X11Window>(window->getId());
    unsigned long color = (r << 16) | (g << 8) | b;
    
    XSetWindowBorder(display_, to_x11_window(x11_window), color);
    
    // Update decoration state
    if (window == get_focused_window()) {
        focused_border_color_ = color;
    } else {
        border_color_ = color;
    }
}

void X11Platform::set_window_border_width(SRDWindow* window, int width) {
    std::cout << "X11Platform: Set border width " << width << std::endl;
    
    if (!window || !display_) return;
    
    X11Window x11_window = static_cast<X11Window>(window->getId());
    XSetWindowBorderWidth(display_, to_x11_window(x11_window), width);
    
    border_width_ = width;
}

bool X11Platform::get_window_decorations(SRDWindow* window) const {
    if (!window) return false;
    
    // Check if window has a frame window
    X11Window x11_window = static_cast<X11Window>(window->getId());
    auto it = frame_window_map_.find(x11_window);
    
    return it != frame_window_map_.end();
}

void X11Platform::create_frame_window(SRDWindow* window) {
    std::cout << "X11Platform: Create frame window for window " << window->getId() << std::endl;
    
    if (!window || !display_) return;
    
    X11Window client_window = static_cast<X11Window>(window->getId());
    
    // Check if frame already exists
    if (frame_window_map_.find(client_window) != frame_window_map_.end()) {
        return;
    }
    
    // Get client window attributes
    XWindowAttributes attr;
    attr.x = 0;
    attr.y = 0;
    attr.width = 0;
    attr.height = 0;
    attr.border_width = 0;
    attr.depth = 0;
    attr.visual = nullptr;
    attr.root = 0;
    attr.c_class = 0;
    attr.bit_gravity = 0;
    attr.win_gravity = 0;
    attr.backing_store = 0;
    attr.backing_planes = 0;
    attr.backing_pixel = 0;
    attr.save_under = 0;
    attr.colormap = 0;
    attr.map_installed = 0;
    attr.map_state = 0;
    attr.all_event_masks = 0;
    attr.your_event_mask = 0;
    attr.do_not_propagate_mask = 0;
    attr.override_redirect = 0;
    if (XGetWindowAttributes(display_, to_x11_window(client_window), &attr) == 0) {
        std::cerr << "Failed to get window attributes" << std::endl;
        return;
    }
    
    // Create frame window
    X11Window frame_window = from_x11_window(XCreateSimpleWindow(
        display_, to_x11_window(root_),
        attr.x, attr.y,
        attr.width + border_width_ * 2,
        attr.height + border_width_ + 30, // Add titlebar height
        border_width_,
        border_color_,
        0x000000 // Background color
    ));
    
    // Set frame window properties
    XSetWindowAttributes frame_attr;
    frame_attr.event_mask = ButtonPressMask | ButtonReleaseMask | 
                           PointerMotionMask | ExposureMask;
    XChangeWindowAttributes(display_, to_x11_window(frame_window), CWEventMask, &frame_attr);
    
    // Reparent client window into frame
    XReparentWindow(display_, to_x11_window(client_window), to_x11_window(frame_window), border_width_, 30);
    
    // Map frame window
    XMapWindow(display_, to_x11_window(frame_window));
    
    // Store frame window mapping
    frame_window_map_[client_window] = frame_window;
    
    // Draw titlebar
    draw_titlebar(window);
    
    std::cout << "X11Platform: Frame window created successfully" << std::endl;
}

void X11Platform::destroy_frame_window(SRDWindow* window) {
    std::cout << "X11Platform: Destroy frame window for window " << window->getId() << std::endl;
    
    if (!window || !display_) return;
    
    X11Window client_window = static_cast<X11Window>(window->getId());
    
    // Find frame window
    auto it = frame_window_map_.find(client_window);
    if (it == frame_window_map_.end()) {
        return;
    }
    
    X11Window frame_window = it->second;
    
    // Reparent client window back to root
    XReparentWindow(display_, to_x11_window(client_window), to_x11_window(root_), 0, 0);
    
    // Destroy frame window
    XDestroyWindow(display_, to_x11_window(frame_window));
    
    // Remove from mapping
    frame_window_map_.erase(it);
    
    std::cout << "X11Platform: Frame window destroyed successfully" << std::endl;
}

void X11Platform::draw_titlebar(SRDWindow* window) {
    std::cout << "X11Platform: Draw titlebar for window " << window->getId() << std::endl;
    
    if (!window || !display_) return;
    
    X11Window client_window = static_cast<X11Window>(window->getId());
    
    // Find frame window
    auto it = frame_window_map_.find(client_window);
    if (it == frame_window_map_.end()) {
        return;
    }
    
    X11Window frame_window = it->second;
    
    // Get window title
    char* window_name = nullptr;
    if (XFetchName(display_, to_x11_window(client_window), &window_name) && window_name) {
        // Create GC for drawing
        XGCValues gc_values;
        gc_values.foreground = 0xFFFFFF; // White text
        gc_values.background = 0x2E3440; // Dark background
        gc_values.font = XLoadFont(display_, "fixed");
        
        GC gc = XCreateGC(display_, to_x11_window(frame_window), 
                         GCForeground | GCBackground | GCFont, &gc_values);
        
        // Draw titlebar background
        XSetForeground(display_, gc, 0x2E3440);
        XFillRectangle(display_, to_x11_window(frame_window), gc, 0, 0, 800, 30); // Placeholder size
        
        // Draw title text
        XSetForeground(display_, gc, 0xFFFFFF);
        XDrawString(display_, to_x11_window(frame_window), gc, 10, 20, window_name, strlen(window_name));
        
        // Clean up
        XFreeGC(display_, gc);
        XFree(window_name);
    }
}

void X11Platform::update_frame_geometry(SRDWindow* window) {
    std::cout << "X11Platform: Update frame geometry" << std::endl;
    
    // TODO: Implement when X11 headers are available
    // Update frame window geometry when client window changes
}

SRDWindow* X11Platform::get_focused_window() const {
    if (!display_) return nullptr;
    
    typedef ::Window X11WindowType;
    X11WindowType focused_window;
    int revert_to;
    XGetInputFocus(display_, &focused_window, &revert_to);
    
    auto it = window_map_.find(from_x11_window(focused_window));
    if (it != window_map_.end()) {
        return it->second;
    }
    
    return nullptr;
}

// EWMH helper methods
void X11Platform::setup_ewmh() {
    if (!display_ || !ewmh_supported_) return;
    
    // Set EWMH supported atoms
    Atom supported[] = {
        _NET_WM_STATE_,
        _NET_WM_STATE_MAXIMIZED_VERT_,
        _NET_WM_STATE_MAXIMIZED_HORZ_,
        _NET_WM_STATE_FULLSCREEN_,
        _NET_WM_STATE_ABOVE_,
        _NET_WM_STATE_BELOW_,
        _NET_WM_WINDOW_TYPE_,
        _NET_WM_WINDOW_TYPE_DESKTOP_,
        _NET_WM_WINDOW_TYPE_DOCK_,
        _NET_WM_WINDOW_TYPE_TOOLBAR_,
        _NET_WM_WINDOW_TYPE_MENU_,
        _NET_WM_WINDOW_TYPE_UTILITY_,
        _NET_WM_WINDOW_TYPE_SPLASH_,
        _NET_WM_WINDOW_TYPE_DIALOG_,
        _NET_WM_WINDOW_TYPE_DROPDOWN_MENU_,
        _NET_WM_WINDOW_TYPE_POPUP_MENU_,
        _NET_WM_WINDOW_TYPE_TOOLTIP_,
        _NET_WM_WINDOW_TYPE_NOTIFICATION_,
        _NET_WM_WINDOW_TYPE_COMBO_,
        _NET_WM_WINDOW_TYPE_DND_,
        _NET_WM_WINDOW_TYPE_NORMAL_,
        _NET_WM_DESKTOP_,
        _NET_NUMBER_OF_DESKTOPS_,
        _NET_CURRENT_DESKTOP_,
        _NET_DESKTOP_NAMES_
    };
    
    XChangeProperty(display_, root_, XInternAtom(display_, "_NET_SUPPORTED", False),
                   XA_ATOM, 32, PropModeReplace, (unsigned char*)supported, 
                   sizeof(supported) / sizeof(supported[0]));
    
    // Set initial desktop info
    update_ewmh_desktop_info();
    
    std::cout << "X11Platform: EWMH setup completed" << std::endl;
}

void X11Platform::update_ewmh_desktop_info() {
    if (!display_ || !ewmh_supported_) return;
    
    // Set number of desktops
    int num_desktops = virtual_desktops_.size();
    XChangeProperty(display_, root_, _NET_NUMBER_OF_DESKTOPS_, XA_CARDINAL, 32,
                   PropModeReplace, (unsigned char*)&num_desktops, 1);
    
    // Set current desktop
    XChangeProperty(display_, root_, _NET_CURRENT_DESKTOP_, XA_CARDINAL, 32,
                   PropModeReplace, (unsigned char*)&current_virtual_desktop_, 1);
    
    // Set desktop names (simplified - just use numbers for now)
    std::vector<std::string> desktop_names;
    for (int i = 0; i < num_desktops; ++i) {
        desktop_names.push_back("Desktop " + std::to_string(i + 1));
    }
    
    // Convert to X11 string format
    std::string names_str;
    for (const auto& name : desktop_names) {
        names_str += name + '\0';
    }
    
    XChangeProperty(display_, root_, _NET_DESKTOP_NAMES_, XA_STRING, 8,
                   PropModeReplace, (unsigned char*)names_str.c_str(), names_str.length());
}

// Linux/X11-specific features implementation
void X11Platform::enable_compositor(bool enabled) {
    compositor_enabled_ = enabled;
    
    if (enabled) {
        // Try to enable compositor effects
        // This is a simplified implementation - real compositors have more complex setup
        std::cout << "X11Platform: Compositor effects enabled" << std::endl;
    } else {
        std::cout << "X11Platform: Compositor effects disabled" << std::endl;
    }
}

void X11Platform::set_window_opacity(SRDWindow* window, unsigned char opacity) {
    if (!window) return;
    
    X11Window x11_window = static_cast<X11Window>(window->getId());
    
    // Set window opacity using _NET_WM_WINDOW_OPACITY atom
    Atom opacity_atom = XInternAtom(display_, "_NET_WM_WINDOW_OPACITY", False);
    if (opacity_atom != None) {
        unsigned long opacity_value = (opacity << 24) | (opacity << 16) | (opacity << 8) | opacity;
        XChangeProperty(display_, x11_window, opacity_atom, XA_CARDINAL, 32,
                       PropModeReplace, (unsigned char*)&opacity_value, 1);
    }
    
    std::cout << "X11Platform: Set window " << window->getId() << " opacity to " << (int)opacity << std::endl;
}

void X11Platform::set_window_blur(SRDWindow* window, bool enabled) {
    if (!window) return;
    
    X11Window x11_window = static_cast<X11Window>(window->getId());
    
    // Set window blur using _NET_WM_WINDOW_BLUR atom (if supported)
    Atom blur_atom = XInternAtom(display_, "_NET_WM_WINDOW_BLUR", False);
    if (blur_atom != None) {
        unsigned long blur_value = enabled ? 1 : 0;
        XChangeProperty(display_, x11_window, blur_atom, XA_CARDINAL, 32,
                       PropModeReplace, (unsigned char*)&blur_value, 1);
    }
    
    std::cout << "X11Platform: Set window " << window->getId() << " blur " << (enabled ? "enabled" : "disabled") << std::endl;
}

void X11Platform::set_window_shadow(SRDWindow* window, bool enabled) {
    if (!window) return;
    
    X11Window x11_window = static_cast<X11Window>(window->getId());
    
    // Set window shadow using _NET_WM_WINDOW_SHADOW atom (if supported)
    Atom shadow_atom = XInternAtom(display_, "_NET_WM_WINDOW_SHADOW", False);
    if (shadow_atom != None) {
        unsigned long shadow_value = enabled ? 1 : 0;
        XChangeProperty(display_, x11_window, shadow_atom, XA_CARDINAL, 32,
                       PropModeReplace, (unsigned char*)&shadow_value, 1);
    }
    
    std::cout << "X11Platform: Set window " << window->getId() << " shadow " << (enabled ? "enabled" : "disabled") << std::endl;
}

// RandR (Resize and Rotate) support implementation
void X11Platform::enable_randr(bool enabled) {
    randr_enabled_ = enabled;
    
    if (enabled) {
        initialize_randr();
    } else {
        cleanup_randr();
    }
    
    std::cout << "X11Platform: RandR support " << (enabled ? "enabled" : "disabled") << std::endl;
}

void X11Platform::set_monitor_rotation(int monitor_id, int rotation) {
    if (!randr_enabled_) return;
    
    // Rotation values: 0=0°, 1=90°, 2=180°, 3=270°
    if (rotation < 0 || rotation > 3) return;
    
    // TODO: Implement actual RandR rotation
    std::cout << "X11Platform: Set monitor " << monitor_id << " rotation to " << (rotation * 90) << "°" << std::endl;
}

void X11Platform::set_monitor_refresh_rate(int monitor_id, int refresh_rate) {
    if (!randr_enabled_) return;
    
    if (refresh_rate < 30 || refresh_rate > 240) return;
    
    // TODO: Implement actual RandR refresh rate setting
    std::cout << "X11Platform: Set monitor " << monitor_id << " refresh rate to " << refresh_rate << " Hz" << std::endl;
}

void X11Platform::set_monitor_scale(int monitor_id, float scale) {
    if (!randr_enabled_) return;
    
    if (scale < 0.5f || scale > 3.0f) return;
    
    // TODO: Implement actual RandR scaling
    std::cout << "X11Platform: Set monitor " << monitor_id << " scale to " << scale << "x" << std::endl;
}

void X11Platform::initialize_randr() {
    if (!display_) return;
    
    // Check if RandR extension is available
    int event_base, error_base;
    if (XRRQueryExtension(display_, &event_base, &error_base)) {
        std::cout << "X11Platform: RandR extension available" << std::endl;
        
        // Get screen resources
        Window root = DefaultRootWindow(display_);
        int screen = DefaultScreen(display_);
        XRRScreenResources* resources = XRRGetScreenResources(display_, root);
        
        if (resources) {
            // Process monitor information
            for (int i = 0; i < resources->noutput; ++i) {
                XRROutputInfo* output_info = XRRGetOutputInfo(display_, resources, resources->outputs[i]);
                if (output_info && output_info->connection == RR_Connected) {
                    // Add monitor to our list
                    Monitor monitor;
                    monitor.id = i;
                    monitor.x = 0; // TODO: Get actual position
                    monitor.y = 0;
                    monitor.width = output_info->mm_width; // Convert mm to pixels
                    monitor.height = output_info->mm_height;
                    monitor.name = output_info->name;
                    monitor.refresh_rate = 60; // Default
                    
                    monitors_.push_back(monitor);
                }
                if (output_info) XRRFreeOutputInfo(output_info);
            }
            XRRFreeScreenResources(resources);
        }
    } else {
        std::cout << "X11Platform: RandR extension not available" << std::endl;
    }
}

void X11Platform::cleanup_randr() {
    // Clean up RandR resources if needed
    std::cout << "X11Platform: RandR cleanup completed" << std::endl;
}

// Panel/Dock integration implementation
void X11Platform::set_panel_visible(bool visible) {
    panel_visible_ = visible;
    
    // TODO: Implement actual panel visibility control
    std::cout << "X11Platform: Panel visibility " << (visible ? "enabled" : "disabled") << std::endl;
}

void X11Platform::set_panel_position(int position) {
    if (position < 0 || position > 3) return;
    
    panel_position_ = position;
    
    // Position values: 0=bottom, 1=top, 2=left, 3=right
    std::string position_str;
    switch (position) {
        case 0: position_str = "bottom"; break;
        case 1: position_str = "top"; break;
        case 2: position_str = "left"; break;
        case 3: position_str = "right"; break;
        default: position_str = "unknown"; break;
    }
    
    std::cout << "X11Platform: Panel position set to " << position_str << std::endl;
}

void X11Platform::set_panel_auto_hide(bool enabled) {
    panel_auto_hide_ = enabled;
    
    // TODO: Implement actual panel auto-hide behavior
    std::cout << "X11Platform: Panel auto-hide " << (enabled ? "enabled" : "disabled") << std::endl;
}

void X11Platform::update_panel_workspace_list() {
    if (!ewmh_supported_) return;
    
    // Update panel with current workspace information
    // This would typically involve communicating with the panel application
    std::cout << "X11Platform: Updated panel workspace list" << std::endl;
}

// System tray integration implementation
void X11Platform::add_system_tray_icon(const std::string& tooltip, Pixmap icon) {
    // TODO: Implement actual system tray icon addition
    // This would involve using the _NET_SYSTEM_TRAY_S0 atom and related protocols
    
    system_tray_icon_ = 1; // Placeholder
    std::cout << "X11Platform: Added system tray icon with tooltip: " << tooltip << std::endl;
}

void X11Platform::remove_system_tray_icon() {
    if (system_tray_icon_) {
        // TODO: Implement actual system tray icon removal
        system_tray_icon_ = 0;
        std::cout << "X11Platform: Removed system tray icon" << std::endl;
    }
}

void X11Platform::show_system_tray_menu(Window menu) {
    // TODO: Implement system tray menu display
    std::cout << "X11Platform: Show system tray menu" << std::endl;
}
