#include "smart_placement.h"
#include <algorithm>
#include <cmath>
#include <iostream>

// Constants
constexpr int SmartPlacement::MIN_WINDOW_WIDTH;
constexpr int SmartPlacement::MIN_WINDOW_HEIGHT;
constexpr int SmartPlacement::GRID_MARGIN;
constexpr int SmartPlacement::CASCADE_OFFSET;

SmartPlacement::PlacementResult SmartPlacement::place_window(
    const SRDWindow* window, const Monitor& monitor, 
    const std::vector<SRDWindow*>& existing_windows) {
    
    // Try grid placement first (SRDWindows 11 style)
    auto grid_result = place_in_grid(window, monitor, existing_windows);
    if (grid_result.success) {
        return grid_result;
    }
    
    // Fall back to cascade placement
    return cascade_place(window, monitor, existing_windows);
}

SmartPlacement::PlacementResult SmartPlacement::place_in_grid(
    const SRDWindow* window, const Monitor& monitor,
    const std::vector<SRDWindow*>& existing_windows) {
    
    PlacementResult result = {0, 0, 0, 0, false, "Grid placement failed"};
    
    // Calculate optimal grid size based on monitor and window count
    int window_count = existing_windows.size() + 1;
    int grid_size = calculate_optimal_grid_size(monitor, window_count);
    
    if (grid_size <= 0) {
        result.reason = "Invalid grid size";
        return result;
    }
    
    // Calculate grid position for this window
    auto [grid_x, grid_y] = calculate_grid_position(window, monitor);
    
    // Calculate cell dimensions
    int cell_width = (monitor.width - (grid_size + 1) * GRID_MARGIN) / grid_size;
    int cell_height = (monitor.height - (grid_size + 1) * GRID_MARGIN) / grid_size;
    
    // Ensure minimum cell size
    cell_width = std::max(cell_width, MIN_WINDOW_WIDTH);
    cell_height = std::max(cell_height, MIN_WINDOW_HEIGHT);
    
    // Calculate window position
    int x = monitor.x + GRID_MARGIN + grid_x * (cell_width + GRID_MARGIN);
    int y = monitor.y + GRID_MARGIN + grid_y * (cell_height + GRID_MARGIN);
    
    // Check if position is valid
    if (is_position_valid(x, y, cell_width, cell_height, monitor)) {
        result.x = x;
        result.y = y;
        result.width = cell_width;
        result.height = cell_height;
        result.success = true;
        result.reason = "Grid placement successful";
    }
    
    return result;
}

SmartPlacement::PlacementResult SmartPlacement::snap_to_edge(
    const SRDWindow* window, const Monitor& monitor,
    const std::vector<SRDWindow*>& existing_windows) {
    
    PlacementResult result = {0, 0, 0, 0, false, "Snap placement failed"};
    
    // For now, implement simple edge snapping
    // In a full implementation, this would detect when windows are dragged near edges
    
    int x = monitor.x + monitor.width / 4;
    int y = monitor.y + monitor.height / 4;
    int width = monitor.width / 2;
    int height = monitor.height / 2;
    
    if (is_position_valid(x, y, width, height, monitor)) {
        result.x = x;
        result.y = y;
        result.width = width;
        result.height = height;
        result.success = true;
        result.reason = "Snap placement successful";
    }
    
    return result;
}

SmartPlacement::PlacementResult SmartPlacement::cascade_place(
    const SRDWindow* window, const Monitor& monitor,
    const std::vector<SRDWindow*>& existing_windows) {
    
    PlacementResult result = {0, 0, 0, 0, false, "Cascade placement failed"};
    
    // Find a free space for cascading
    auto free_spaces = find_free_spaces(monitor, existing_windows);
    
    if (free_spaces.empty()) {
        // No free spaces, use default position
        int x = monitor.x + CASCADE_OFFSET;
        int y = monitor.y + CASCADE_OFFSET;
        int width = std::min(800, monitor.width - 2 * CASCADE_OFFSET);
        int height = std::min(600, monitor.height - 2 * CASCADE_OFFSET);
        
        if (is_position_valid(x, y, width, height, monitor)) {
            result.x = x;
            result.y = y;
            result.width = width;
            result.height = height;
            result.success = true;
            result.reason = "Default cascade placement";
        }
    } else {
        // Use the first free space
        auto [x, y] = free_spaces[0];
        int width = std::min(800, monitor.width - x - CASCADE_OFFSET);
        int height = std::min(600, monitor.height - y - CASCADE_OFFSET);
        
        if (is_position_valid(x, y, width, height, monitor)) {
            result.x = x;
            result.y = y;
            result.width = width;
            result.height = height;
            result.success = true;
            result.reason = "Cascade placement in free space";
        }
    }
    
    return result;
}

SmartPlacement::PlacementResult SmartPlacement::smart_tile(
    const SRDWindow* window, const Monitor& monitor,
    const std::vector<SRDWindow*>& existing_windows) {
    
    PlacementResult result = {0, 0, 0, 0, false, "Smart tile placement failed"};
    
    // Calculate overlap score to find best position
    int best_score = -1;
    int best_x = monitor.x;
    int best_y = monitor.y;
    int best_width = monitor.width / 2;
    int best_height = monitor.height / 2;
    
    // Try different positions and find the one with least overlap
    for (int x = monitor.x; x < monitor.x + monitor.width - MIN_WINDOW_WIDTH; x += 50) {
        for (int y = monitor.y; y < monitor.y + monitor.height - MIN_WINDOW_HEIGHT; y += 50) {
            int width = std::min(800, monitor.width - x);
            int height = std::min(600, monitor.height - y);
            
            if (is_position_valid(x, y, width, height, monitor)) {
                int score = calculate_overlap_score(window, monitor, existing_windows);
                if (score > best_score) {
                    best_score = score;
                    best_x = x;
                    best_y = y;
                    best_width = width;
                    best_height = height;
                }
            }
        }
    }
    
    if (best_score >= 0) {
        result.x = best_x;
        result.y = best_y;
        result.width = best_width;
        result.height = best_height;
        result.success = true;
        result.reason = "Smart tile placement successful";
    }
    
    return result;
}

bool SmartPlacement::windows_overlap(const SRDWindow* w1, const SRDWindow* w2) {
    // Simple AABB overlap detection
    int x1 = w1->getX();
    int y1 = w1->getY();
    int w1_width = w1->getWidth();
    int w1_height = w1->getHeight();
    
    int x2 = w2->getX();
    int y2 = w2->getY();
    int w2_width = w2->getWidth();
    int w2_height = w2->getHeight();
    
    return !(x1 + w1_width <= x2 || x2 + w2_width <= x1 ||
             y1 + w1_height <= y2 || y2 + w2_height <= y1);
}

int SmartPlacement::calculate_overlap_score(const SRDWindow* window, const Monitor& monitor,
                                           const std::vector<SRDWindow*>& existing_windows) {
    int score = 0;
    
    // Calculate how much this position overlaps with existing windows
    for (const auto* existing : existing_windows) {
        if (windows_overlap(window, existing)) {
            score -= 10; // Penalty for overlap
        } else {
            score += 1;  // Bonus for no overlap
        }
    }
    
    return score;
}

std::vector<std::pair<int, int>> SmartPlacement::find_free_spaces(
    const Monitor& monitor, const std::vector<SRDWindow*>& existing_windows) {
    
    std::vector<std::pair<int, int>> free_spaces;
    
    // Simple algorithm: try positions in a grid pattern
    for (int x = monitor.x; x < monitor.x + monitor.width - MIN_WINDOW_WIDTH; x += 100) {
        for (int y = monitor.y; y < monitor.y + monitor.height - MIN_WINDOW_HEIGHT; y += 100) {
            bool is_free = true;
            
            // Check if this position overlaps with any existing window
            for (const auto* existing : existing_windows) {
                int ex = existing->getX();
                int ey = existing->getY();
                int ew = existing->getWidth();
                int eh = existing->getHeight();
                
                if (x < ex + ew && x + MIN_WINDOW_WIDTH > ex &&
                    y < ey + eh && y + MIN_WINDOW_HEIGHT > ey) {
                    is_free = false;
                    break;
                }
            }
            
            if (is_free) {
                free_spaces.emplace_back(x, y);
            }
        }
    }
    
    return free_spaces;
}

bool SmartPlacement::is_position_valid(int x, int y, int width, int height, const Monitor& monitor) {
    return x >= monitor.x && y >= monitor.y &&
           x + width <= monitor.x + monitor.width &&
           y + height <= monitor.y + monitor.height &&
           width >= MIN_WINDOW_WIDTH && height >= MIN_WINDOW_HEIGHT;
}

std::pair<int, int> SmartPlacement::calculate_grid_position(const SRDWindow* window, const Monitor& monitor) {
    // Simple grid position calculation
    // In a real implementation, this might consider window properties or user preferences
    
    // For now, use a simple pattern: first window top-left, second top-right, etc.
    static int window_counter = 0;
    int grid_x = window_counter % 2;
    int grid_y = window_counter / 2;
    window_counter++;
    
    return {grid_x, grid_y};
}

int SmartPlacement::calculate_optimal_grid_size(const Monitor& monitor, int window_count) {
    // Calculate optimal grid size based on monitor dimensions and window count
    if (window_count <= 0) return 1;
    
    // Simple heuristic: try to create a roughly square grid
    int grid_size = static_cast<int>(std::ceil(std::sqrt(window_count)));
    
    // Ensure grid size is reasonable
    grid_size = std::max(1, std::min(grid_size, 4));
    
    return grid_size;
}
