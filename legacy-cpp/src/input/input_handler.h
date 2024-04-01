#ifndef SRDWM_INPUT_HANDLER_H
#define SRDWM_INPUT_HANDLER_H

#include <memory>

// Define basic event structures (these will need more detail later)
struct KeyboardEvent {
    int key_code;
    // Add modifiers, state (press/release)
};

struct MouseEvent {
    enum class Type { Press, Release, Motion };
    Type type;
    int button; // Valid for Press/Release
    int x, y;
    // Add modifiers
};

class InputHandler {
public:
    virtual ~InputHandler() = default;

    // Pure virtual methods for handling events
    virtual void handle_key_press(const KeyboardEvent& event) = 0;
    virtual void handle_key_release(const KeyboardEvent& event) = 0;
    virtual void handle_mouse_button_press(const MouseEvent& event) = 0;
    virtual void handle_mouse_button_release(const MouseEvent& event) = 0;
    virtual void handle_mouse_motion(const MouseEvent& event) = 0;

    // Other potential input-related methods
    // virtual void initialize() = 0;
    // virtual void shutdown() = 0;
};

#endif // SRDWM_INPUT_HANDLER_H
