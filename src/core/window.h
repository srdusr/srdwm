#ifndef WINDOW_H
#define WINDOW_H

#include <string>

class SRDWindow {
public:
    // Constructor
    SRDWindow(int id, const std::string& title);

    // Getters
    int getId() const;
    const std::string& getTitle() const;
    int getX() const;
    int getY() const;
    int getWidth() const;
    int getHeight() const;
    bool isDecorated() const;

    // Setters
    void setTitle(const std::string& title);
    void setPosition(int x, int y);
    void setSize(int width, int height);
    void setGeometry(int x, int y, int width, int height);
    void setDimensions(int x, int y, int width, int height);
    void setDecorated(bool decorated);
    void setId(int id);

    // Window management methods
    void map();
    void unmap();
    void focus();
    void close();

    // Comparison operators for use in containers
    bool operator<(const SRDWindow& other) const { return id_ < other.id_; }
    bool operator==(const SRDWindow& other) const { return id_ == other.id_; }

private:
    int id_;
    std::string title_;
    int x_;
    int y_;
    int width_;
    int height_;
    bool decorated_;
};

#endif // WINDOW_H
