#ifndef SRDWM_DYNAMIC_LAYOUT_H
#define SRDWM_DYNAMIC_LAYOUT_H

#include "layout.h"
#include "../core/window.h"
#include <vector>

class DynamicLayout : public Layout {
public:
    DynamicLayout();
    ~DynamicLayout();
    
    // Implement the pure virtual method from the base class
    void arrange_windows(const std::vector<SRDWindow*>& windows, const Monitor& monitor) override;
};

#endif // SRDWM_DYNAMIC_LAYOUT_H
