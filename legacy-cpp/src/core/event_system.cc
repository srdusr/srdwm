#include "event_system.h"
#include "window.h"
#include <iostream>
#include <algorithm>

// Global event system instance
EventSystem g_event_system;

EventSystem::EventSystem() : processing_events(false) {
}

EventSystem::~EventSystem() {
    clear_handlers();
}

void EventSystem::register_handler(EventType type, EventHandler handler) {
    handlers[type].push_back(handler);
}

void EventSystem::unregister_handler(EventType type, EventHandler handler) {
    auto& type_handlers = handlers[type];
    type_handlers.erase(
        std::remove_if(type_handlers.begin(), type_handlers.end(),
            [&handler](const EventHandler& h) {
                // Note: This is a simplified comparison
                // In a real implementation, you'd want a more sophisticated way to identify handlers
                return false; // For now, we don't remove handlers
            }),
        type_handlers.end()
    );
}

void EventSystem::emit_event(const Event& event) {
    if (processing_events) {
        // Queue the event if we're currently processing events
        event_queue.push_back(std::make_unique<Event>(event));
        return;
    }
    
    auto it = handlers.find(event.type);
    if (it != handlers.end()) {
        for (const auto& handler : it->second) {
            try {
                handler(event);
            } catch (const std::exception& e) {
                std::cerr << "Error in event handler: " << e.what() << std::endl;
            }
        }
    }
}

void EventSystem::emit_window_event(EventType type, SRDWindow* window) {
    switch (type) {
        case EventType::WINDOW_CREATED:
            emit_event(SRDWindowCreatedEvent(window));
            break;
        case EventType::WINDOW_DESTROYED:
            emit_event(SRDWindowDestroyedEvent(window));
            break;
        default:
            // For other window events, we need more context
            break;
    }
}

void EventSystem::emit_key_event(EventType type, unsigned int keycode, unsigned int modifiers) {
    emit_event(KeyEvent(type, keycode, modifiers));
}

void EventSystem::emit_mouse_event(EventType type, int x, int y, unsigned int button, unsigned int modifiers) {
    emit_event(MouseEvent(type, x, y, button, modifiers));
}

void EventSystem::process_events() {
    if (processing_events) {
        return; // Prevent recursive processing
    }
    
    processing_events = true;
    
    // Process queued events
    while (!event_queue.empty()) {
        auto event = std::move(event_queue.front());
        event_queue.erase(event_queue.begin());
        
        emit_event(*event);
    }
    
    processing_events = false;
}

void EventSystem::clear_handlers() {
    handlers.clear();
}
