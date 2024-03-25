#include "window.h"

SRDWindow::SRDWindow(int id, const std::string& title)
    : id_(id), title_(title), x_(0), y_(0), width_(0), height_(0), decorated_(true) {
}

int SRDWindow::getId() const {
    return id_;
}

const std::string& SRDWindow::getTitle() const {
    return title_;
}

int SRDWindow::getX() const {
    return x_;
}

int SRDWindow::getY() const {
    return y_;
}

int SRDWindow::getWidth() const {
    return width_;
}

int SRDWindow::getHeight() const {
    return height_;
}

bool SRDWindow::isDecorated() const {
    return decorated_;
}

void SRDWindow::setPosition(int x, int y) {
    x_ = x;
    y_ = y;
}

void SRDWindow::setSize(int width, int height) {
    width_ = width;
    height_ = height;
}

void SRDWindow::setGeometry(int x, int y, int width, int height) {
    x_ = x;
    y_ = y;
    width_ = width;
    height_ = height;
}

void SRDWindow::setDimensions(int x, int y, int width, int height) {
    x_ = x;
    y_ = y;
    width_ = width;
    height_ = height;
}

void SRDWindow::setDecorated(bool decorated) {
    decorated_ = decorated;
}

void SRDWindow::setId(int id) {
    id_ = id;
}

// Basic methods for managing window state (will be platform-specific)
void SRDWindow::map() {
    // Platform-specific implementation to show the window
}

void SRDWindow::unmap() {
    // Platform-specific implementation to hide the window
}

void SRDWindow::focus() {
    // Platform-specific implementation to give focus to the window
}

void SRDWindow::close() {
    // Platform-specific implementation to close the window
}
