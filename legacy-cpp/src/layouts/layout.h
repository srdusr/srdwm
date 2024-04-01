#ifndef SRDWM_LAYOUT_H
#define SRDWM_LAYOUT_H

#include <vector>
#include "../core/window.h"

struct Monitor {
    int id;
    int x;
    int y;
    int width;
    int height;
    std::string name;
    int refresh_rate;
    
    Monitor() : id(0), x(0), y(0), width(0), height(0), refresh_rate(60) {}
    Monitor(int id, int x, int y, int width, int height, const std::string& name = "", int refresh = 60)
        : id(id), x(x), y(y), width(width), height(height), name(name), refresh_rate(refresh) {}
};

class Layout {
public:
    virtual ~Layout() = default;

    // Pure virtual method to arrange windows on a given monitor
    virtual void arrange_windows(const std::vector<SRDWindow*>& windows, const Monitor& monitor) = 0;

    // You might add other common layout methods here later, e.g.:
    // virtual void add_window(SRDWindow* window) = 0;
    // virtual void remove_window(SRDWindow* window) = 0;
};

#endif // SRDWM_LAYOUT_H
