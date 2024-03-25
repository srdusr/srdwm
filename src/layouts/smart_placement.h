#ifndef SRDWM_SMART_PLACEMENT_H
#define SRDWM_SMART_PLACEMENT_H

#include "layout.h"
#include <vector>
#include <memory>

// Forward declarations
class SRDWindow;
class Monitor;

// Smart placement algorithm that mimics SRDWindows 11 behavior
class SmartPlacement {
public:
    struct PlacementResult {
        int x, y, width, height;
        bool success;
        std::string reason;
    };

    // Main placement function
    static PlacementResult place_window(const SRDWindow* window, const Monitor& monitor, 
                                       const std::vector<SRDWindow*>& existing_windows);

    // Grid-based placement (SRDWindows 11 style)
    static PlacementResult place_in_grid(const SRDWindow* window, const Monitor& monitor,
                                        const std::vector<SRDWindow*>& existing_windows);

    // Snap-to-edge placement
    static PlacementResult snap_to_edge(const SRDWindow* window, const Monitor& monitor,
                                       const std::vector<SRDWindow*>& existing_windows);

    // Cascade placement for overlapping windows
    static PlacementResult cascade_place(const SRDWindow* window, const Monitor& monitor,
                                        const std::vector<SRDWindow*>& existing_windows);

    // Smart tiling placement
    static PlacementResult smart_tile(const SRDWindow* window, const Monitor& monitor,
                                     const std::vector<SRDWindow*>& existing_windows);

private:
    // Helper functions
    static bool windows_overlap(const SRDWindow* w1, const SRDWindow* w2);
    static int calculate_overlap_score(const SRDWindow* window, const Monitor& monitor,
                                      const std::vector<SRDWindow*>& existing_windows);
    static std::vector<std::pair<int, int>> find_free_spaces(const Monitor& monitor,
                                                             const std::vector<SRDWindow*>& existing_windows);
    static bool is_position_valid(int x, int y, int width, int height, const Monitor& monitor);
    
    // Grid calculations
    static std::pair<int, int> calculate_grid_position(const SRDWindow* window, const Monitor& monitor);
    static int calculate_optimal_grid_size(const Monitor& monitor, int window_count);
    
    // Constants
    static constexpr int MIN_WINDOW_WIDTH = 200;
    static constexpr int MIN_WINDOW_HEIGHT = 150;
    static constexpr int GRID_MARGIN = 10;
    static constexpr int CASCADE_OFFSET = 30;
};

#endif // SRDWM_SMART_PLACEMENT_H

