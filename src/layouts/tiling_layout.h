#ifndef SRDWM_TILING_LAYOUT_H
#define SRDWM_TILING_LAYOUT_H

#include "layout.h"
#include <iostream>
#include <vector>

class TilingLayout : public Layout {
public:
    TilingLayout();
    ~TilingLayout();
    
    // Implement the pure virtual method from the base class
    void arrange_windows(const std::vector<SRDWindow*>& windows, const Monitor& monitor) override;
};

#endif // SRDWM_TILING_LAYOUT_H
