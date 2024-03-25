#include <gtest/gtest.h>
#include "../src/layouts/smart_placement.h"
#include "../src/core/window.h"
#include "../src/layouts/layout.h"

class SmartPlacementTest : public ::testing::Test {
protected:
    void SetUp() override {
        // Create test monitor
        monitor.id = 1;
        monitor.name = "Test Monitor";
        monitor.x = 0;
        monitor.y = 0;
        monitor.width = 1920;
        monitor.height = 1080;
        monitor.refresh_rate = 60;
        
        // Create test windows
        for (int i = 0; i < 5; ++i) {
            auto window = std::make_unique<Window>();
            window->setId(i + 1);
            window->setTitle("Test Window " + std::to_string(i + 1));
            window->setPosition(100 + i * 50, 100 + i * 50);
            window->setSize(400, 300);
            existing_windows.push_back(std::move(window));
        }
    }
    
    Monitor monitor;
    std::vector<std::unique_ptr<Window>> existing_windows;
};

TEST_F(SmartPlacementTest, GridPlacement) {
    auto window = std::make_unique<Window>();
    window->setId(100);
    window->setTitle("Grid Test Window");
    window->setSize(400, 300);
    
    auto result = SmartPlacement::place_in_grid(window.get(), monitor, existing_windows);
    
    EXPECT_TRUE(result.success);
    EXPECT_GE(result.x, monitor.x);
    EXPECT_LE(result.x + window->getWidth(), monitor.x + monitor.width);
    EXPECT_GE(result.y, monitor.y);
    EXPECT_LE(result.y + window->getHeight(), monitor.y + monitor.height);
}

TEST_F(SmartPlacementTest, CascadePlacement) {
    auto window = std::make_unique<Window>();
    window->setId(101);
    window->setTitle("Cascade Test Window");
    window->setSize(400, 300);
    
    auto result = SmartPlacement::cascade_place(window.get(), monitor, existing_windows);
    
    EXPECT_TRUE(result.success);
    EXPECT_GE(result.x, monitor.x);
    EXPECT_LE(result.x + window->getWidth(), monitor.x + monitor.width);
    EXPECT_GE(result.y, monitor.y);
    EXPECT_LE(result.y + window->getHeight(), monitor.y + monitor.height);
}

TEST_F(SmartPlacementTest, SnapToEdge) {
    auto window = std::make_unique<Window>();
    window->setId(102);
    window->setTitle("Snap Test Window");
    window->setSize(400, 300);
    
    auto result = SmartPlacement::snap_to_edge(window.get(), monitor, existing_windows);
    
    EXPECT_TRUE(result.success);
    EXPECT_GE(result.x, monitor.x);
    EXPECT_LE(result.x + window->getWidth(), monitor.x + monitor.width);
    EXPECT_GE(result.y, monitor.y);
    EXPECT_LE(result.y + window->getHeight(), monitor.y + monitor.height);
}

TEST_F(SmartPlacementTest, SmartTile) {
    auto window = std::make_unique<Window>();
    window->setId(103);
    window->setTitle("Smart Tile Test Window");
    window->setSize(400, 300);
    
    auto result = SmartPlacement::smart_tile(window.get(), monitor, existing_windows);
    
    EXPECT_TRUE(result.success);
    EXPECT_GE(result.x, monitor.x);
    EXPECT_LE(result.x + window->getWidth(), monitor.x + monitor.width);
    EXPECT_GE(result.y, monitor.y);
    EXPECT_LE(result.y + window->getHeight(), monitor.y + monitor.height);
}

TEST_F(SmartPlacementTest, OverlapDetection) {
    auto window1 = std::make_unique<Window>();
    window1->setId(200);
    window1->setPosition(100, 100);
    window1->setSize(400, 300);
    
    auto window2 = std::make_unique<Window>();
    window2->setId(201);
    window2->setPosition(200, 200);
    window2->setSize(400, 300);
    
    EXPECT_TRUE(SmartPlacement::windows_overlap(window1.get(), window2.get()));
    
    auto window3 = std::make_unique<Window>();
    window3->setId(202);
    window3->setPosition(600, 600);
    window3->setSize(400, 300);
    
    EXPECT_FALSE(SmartPlacement::windows_overlap(window1.get(), window3.get()));
}

TEST_F(SmartPlacementTest, OptimalGridSize) {
    auto grid_size = SmartPlacement::calculate_optimal_grid_size(4, monitor);
    EXPECT_GT(grid_size.first, 0);
    EXPECT_GT(grid_size.second, 0);
    EXPECT_LE(grid_size.first * grid_size.second, 6); // Should fit 4 windows with some margin
}

TEST_F(SmartPlacementTest, FreeSpaceFinding) {
    auto free_spaces = SmartPlacement::find_free_spaces(monitor, existing_windows);
    EXPECT_FALSE(free_spaces.empty());
    
    for (const auto& space : free_spaces) {
        EXPECT_GT(space.width, 0);
        EXPECT_GT(space.height, 0);
        EXPECT_GE(space.x, monitor.x);
        EXPECT_GE(space.y, monitor.y);
        EXPECT_LE(space.x + space.width, monitor.x + monitor.width);
        EXPECT_LE(space.y + space.height, monitor.y + monitor.height);
    }
}

TEST_F(SmartPlacementTest, PositionValidation) {
    auto window = std::make_unique<Window>();
    window->setId(300);
    window->setSize(400, 300);
    
    // Valid position
    EXPECT_TRUE(SmartPlacement::is_position_valid(100, 100, window.get(), monitor, existing_windows));
    
    // Invalid position (outside monitor)
    EXPECT_FALSE(SmartPlacement::is_position_valid(-100, -100, window.get(), monitor, existing_windows));
    EXPECT_FALSE(SmartPlacement::is_position_valid(2000, 2000, window.get(), monitor, existing_windows));
}

TEST_F(SmartPlacementTest, GridPositionCalculation) {
    auto grid_size = std::make_pair(2, 2);
    auto cell_size = std::make_pair(monitor.width / 2, monitor.height / 2);
    
    auto pos = SmartPlacement::calculate_grid_position(0, 0, grid_size, cell_size, monitor);
    EXPECT_EQ(pos.first, monitor.x);
    EXPECT_EQ(pos.second, monitor.y);
    
    pos = SmartPlacement::calculate_grid_position(1, 1, grid_size, cell_size, monitor);
    EXPECT_EQ(pos.first, monitor.x + cell_size.first);
    EXPECT_EQ(pos.second, monitor.y + cell_size.second);
}

TEST_F(SmartPlacementTest, OverlapScoreCalculation) {
    auto window1 = std::make_unique<Window>();
    window1->setId(400);
    window1->setPosition(100, 100);
    window1->setSize(400, 300);
    
    auto window2 = std::make_unique<Window>();
    window2->setId(401);
    window2->setPosition(200, 200);
    window2->setSize(400, 300);
    
    auto score = SmartPlacement::calculate_overlap_score(window1.get(), window2.get());
    EXPECT_GT(score, 0);
    
    auto window3 = std::make_unique<Window>();
    window3->setId(402);
    window3->setPosition(600, 600);
    window3->setSize(400, 300);
    
    score = SmartPlacement::calculate_overlap_score(window1.get(), window3.get());
    EXPECT_EQ(score, 0);
}

int main(int argc, char **argv) {
    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
