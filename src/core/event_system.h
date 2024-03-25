#ifndef SRDWM_EVENT_SYSTEM_H
#define SRDWM_EVENT_SYSTEM_H

#include <functional>
#include <map>
#include <vector>
#include <string>
#include <memory>

// Forward declarations
class SRDWindow;
class Monitor;

// Event types
enum class EventType {
    WINDOW_CREATED,
    WINDOW_DESTROYED,
    WINDOW_MOVED,
    WINDOW_RESIZED,
    WINDOW_FOCUSED,
    WINDOW_UNFOCUSED,
    WINDOW_MINIMIZED,
    WINDOW_MAXIMIZED,
    WINDOW_RESTORED,
    MONITOR_ADDED,
    MONITOR_REMOVED,
    MONITOR_CHANGED,
    KEY_PRESSED,
    KEY_RELEASED,
    MOUSE_MOVED,
    MOUSE_PRESSED,
    MOUSE_RELEASED,
    MOUSE_WHEEL,
    CUSTOM_EVENT
};

// Base event class
class Event {
public:
    virtual ~Event() = default;
    EventType type;
    
    Event(EventType t) : type(t) {}
};

// SRDWindow events
class SRDWindowEvent : public Event {
public:
    SRDWindow* window;
    
    SRDWindowEvent(EventType t, SRDWindow* w) : Event(t), window(w) {}
};

class SRDWindowCreatedEvent : public SRDWindowEvent {
public:
    SRDWindowCreatedEvent(SRDWindow* w) : SRDWindowEvent(EventType::WINDOW_CREATED, w) {}
};

class SRDWindowDestroyedEvent : public SRDWindowEvent {
public:
    SRDWindowDestroyedEvent(SRDWindow* w) : SRDWindowEvent(EventType::WINDOW_DESTROYED, w) {}
};

class SRDWindowMovedEvent : public SRDWindowEvent {
public:
    int x, y;
    
    SRDWindowMovedEvent(SRDWindow* w, int new_x, int new_y) 
        : SRDWindowEvent(EventType::WINDOW_MOVED, w), x(new_x), y(new_y) {}
};

class SRDWindowResizedEvent : public SRDWindowEvent {
public:
    int width, height;
    
    SRDWindowResizedEvent(SRDWindow* w, int new_width, int new_height) 
        : SRDWindowEvent(EventType::WINDOW_RESIZED, w), width(new_width), height(new_height) {}
};

// Input events
class KeyEvent : public Event {
public:
    unsigned int keycode;
    unsigned int modifiers;
    
    KeyEvent(EventType t, unsigned int kc, unsigned int mods) 
        : Event(t), keycode(kc), modifiers(mods) {}
};

class MouseEvent : public Event {
public:
    int x, y;
    unsigned int button;
    unsigned int modifiers;
    
    MouseEvent(EventType t, int mouse_x, int mouse_y, unsigned int btn, unsigned int mods) 
        : Event(t), x(mouse_x), y(mouse_y), button(btn), modifiers(mods) {}
};

// Event handler function type
using EventHandler = std::function<void(const Event&)>;

// Event system class
class EventSystem {
public:
    EventSystem();
    ~EventSystem();
    
    // Register event handlers
    void register_handler(EventType type, EventHandler handler);
    void unregister_handler(EventType type, EventHandler handler);
    
    // Emit events
    void emit_event(const Event& event);
    void emit_window_event(EventType type, SRDWindow* window);
    void emit_key_event(EventType type, unsigned int keycode, unsigned int modifiers);
    void emit_mouse_event(EventType type, int x, int y, unsigned int button, unsigned int modifiers);
    
    // Process event queue
    void process_events();
    
    // Clear all handlers
    void clear_handlers();

private:
    std::map<EventType, std::vector<EventHandler>> handlers;
    std::vector<std::unique_ptr<Event>> event_queue;
    bool processing_events;
};

// Global event system instance
extern EventSystem g_event_system;

#endif // SRDWM_EVENT_SYSTEM_H
