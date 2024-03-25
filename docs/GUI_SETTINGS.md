# SRDWM GUI Settings Program

## Overview
The SRDWM GUI Settings program provides a user-friendly interface for configuring the window manager without editing Lua files directly. It integrates seamlessly with existing system settings structures on Windows, macOS, and Linux.

## Architecture

### Cross-Platform GUI Framework
- **Linux**: GTK4 with native desktop integration
- **Windows**: WinUI 3 with Windows Settings integration
- **macOS**: SwiftUI with System Preferences integration

### System Integration
- **Windows**: Appears in Windows Settings > System > Window Manager
- **macOS**: Appears in System Preferences > Desktop & Screen Saver > Window Manager
- **Linux**: Appears in GNOME Settings, KDE System Settings, etc.

## Main Interface

### 1. General Settings Tab
```
┌─────────────────────────────────────────────────────────┐
│ General Settings                                        │
├─────────────────────────────────────────────────────────┤
│ Default Layout: [Dynamic ▼]                             │
│ Smart Window Placement: ☑                              │
│ Window Gap: [8] pixels                                 │
│ Border Width: [2] pixels                               │
│ Enable Animations: ☑                                   │
│ Animation Duration: [200] ms                           │
│                                                         │
│ Focus Follows Mouse: ☐                                 │
│ Mouse Follows Focus: ☑                                 │
│ Auto Raise Windows: ☐                                  │
│ Auto Focus Windows: ☑                                  │
└─────────────────────────────────────────────────────────┘
```

### 2. Key Bindings Tab
```
┌─────────────────────────────────────────────────────────┐
│ Key Bindings                                           │
├─────────────────────────────────────────────────────────┤
│ Layout Switching                                       │
│ ├─ Tiling Layout: [Mod4+1] [Change] [Remove]          │
│ ├─ Dynamic Layout: [Mod4+2] [Change] [Remove]         │
│ └─ Floating Layout: [Mod4+3] [Change] [Remove]        │
│                                                         │
│ Window Management                                      │
│ ├─ Close Window: [Mod4+q] [Change] [Remove]           │
│ ├─ Minimize Window: [Mod4+m] [Change] [Remove]        │
│ └─ Maximize Window: [Mod4+f] [Change] [Remove]        │
│                                                         │
│ [Add New Binding]                                      │
└─────────────────────────────────────────────────────────┘
```

### 3. Layouts Tab
```
┌─────────────────────────────────────────────────────────┐
│ Layouts                                                │
├─────────────────────────────────────────────────────────┤
│ Tiling Layout                                          │
│ ├─ Split Ratio: [50]% [Reset]                         │
│ ├─ Master Ratio: [60]% [Reset]                        │
│ ├─ Auto Swap: ☑                                       │
│ └─ Gaps: Inner [8] Outer [16] [Reset]                 │
│                                                         │
│ Dynamic Layout                                         │
│ ├─ Snap Threshold: [50]px [Reset]                     │
│ ├─ Grid Size: [6] [Reset]                             │
│ ├─ Cascade Offset: [30]px [Reset]                     │
│ └─ Smart Placement: ☑                                 │
│                                                         │
│ [Add Custom Layout]                                    │
└─────────────────────────────────────────────────────────┘
```

### 4. Themes Tab
```
┌─────────────────────────────────────────────────────────┐
│ Themes                                                 │
├─────────────────────────────────────────────────────────┤
│ Current Theme: [Nord ▼] [Preview]                     │
│                                                         │
│ Colors                                                 │
│ ├─ Background: [■] #2e3440 [Change]                   │
│ ├─ Foreground: [■] #eceff4 [Change]                   │
│ ├─ Primary: [■] #88c0d0 [Change]                      │
│ └─ Secondary: [■] #81a1c1 [Change]                    │
│                                                         │
│ Window Decorations                                     │
│ ├─ Border Width: [2]px [Reset]                        │
│ ├─ Title Bar Height: [24]px [Reset]                   │
│ └─ Font: [JetBrains Mono 10] [Change]                 │
│                                                         │
│ [Import Theme] [Export Theme] [Create New]             │
└─────────────────────────────────────────────────────────┘
```

### 5. Window Rules Tab
```
┌─────────────────────────────────────────────────────────┐
│ Window Rules                                           │
├─────────────────────────────────────────────────────────┤
│ Rule 1: Firefox → Dynamic Layout                      │
│ ├─ Match: Class = "firefox"                           │
│ ├─ Action: Layout = "dynamic"                         │
│ └─ [Edit] [Delete]                                    │
│                                                         │
│ Rule 2: Terminal → Tiling Layout                      │
│ ├─ Match: Class = "terminal"                          │
│ ├─ Action: Layout = "tiling"                          │
│ └─ [Edit] [Delete]                                    │
│                                                         │
│ [Add New Rule]                                         │
└─────────────────────────────────────────────────────────┘
```

### 6. Performance Tab
```
┌─────────────────────────────────────────────────────────┐
│ Performance                                            │
├─────────────────────────────────────────────────────────┤
│ Graphics                                              │
│ ├─ Enable V-Sync: ☑                                   │
│ ├─ Max FPS: [60] [Reset]                              │
│ └─ Enable Caching: ☑                                  │
│                                                         │
│ Memory                                                 │
│ ├─ Window Cache Size: [100] [Reset]                   │
│ ├─ Event Queue Size: [1000] [Reset]                   │
│ └─ Layout Timeout: [16]ms [Reset]                     │
│                                                         │
│ [Optimize for Performance] [Reset to Defaults]         │
└─────────────────────────────────────────────────────────┘
```

## Key Binding Editor

### Add/Edit Key Binding Dialog
```
┌─────────────────────────────────────────────────────────┐
│ Edit Key Binding                                      │
├─────────────────────────────────────────────────────────┤
│ Key Combination: [Press keys here...]                 │
│ Current: Mod4+Shift+1                                 │
│                                                         │
│ Action Type: [Window Management ▼]                     │
│ Action: [Close Window ▼]                              │
│                                                         │
│ Custom Command: [________________]                     │
│                                                         │
│ [Test Binding] [OK] [Cancel]                          │
└─────────────────────────────────────────────────────────┘
```

### Key Combination Parser
- **Mod4**: Super/Windows key
- **Mod1**: Alt key
- **Mod2**: Num Lock
- **Mod3**: Scroll Lock
- **Shift**: Shift key
- **Ctrl**: Control key

## Theme Editor

### Color Picker Integration
```
┌─────────────────────────────────────────────────────────┐
│ Color Picker                                           │
├─────────────────────────────────────────────────────────┤
│ Color: [■] #88c0d0                                    │
│                                                         │
│ RGB: R [136] G [192] B [208]                          │
│ HSV: H [199] S [35] V [82]                            │
│                                                         │
│ Preset Colors:                                         │
│ [■][■][■][■][■][■][■][■]                              │
│                                                         │
│ [Pick from Screen] [OK] [Cancel]                      │
└─────────────────────────────────────────────────────────┘
```

### Font Selector
```
┌─────────────────────────────────────────────────────────┐
│ Font Selection                                         │
├─────────────────────────────────────────────────────────┤
│ Font Family: [JetBrains Mono ▼]                       │
│ Font Size: [10] [Reset]                               │
│ Font Weight: [Normal ▼]                               │
│ Font Style: [Normal ▼]                                │
│                                                         │
│ Preview: The quick brown fox jumps over the lazy dog   │
│                                                         │
│ [OK] [Cancel]                                          │
└─────────────────────────────────────────────────────────┘
```

## System Integration

### Windows Integration
```cpp
// Windows Settings integration
class WindowsSettingsIntegration {
public:
    void register_with_settings();
    void create_settings_page();
    void handle_settings_changes();
    
private:
    void add_to_windows_settings();
    void create_registry_entries();
    void register_protocol_handler();
};
```

### macOS Integration
```swift
// macOS System Preferences integration
class MacOSSettingsIntegration: NSObject {
    func registerWithSystemPreferences()
    func createPreferencesPane()
    func handlePreferencesChanges()
    
    private func addToSystemPreferences()
    func createPreferencePaneBundle()
    func registerURLScheme()
}
```

### Linux Integration
```cpp
// Linux desktop integration
class LinuxDesktopIntegration {
public:
    void register_with_desktop();
    void create_settings_app();
    void handle_settings_changes();
    
private:
    void add_to_gnome_settings();
    void add_to_kde_settings();
    void create_desktop_file();
    void register_mime_types();
};
```

## Configuration Management

### Auto-Save and Validation
```cpp
class ConfigurationManager {
public:
    void auto_save_changes();
    bool validate_configuration();
    void backup_configuration();
    void restore_configuration();
    
private:
    void save_to_lua_files();
    void validate_lua_syntax();
    void create_backup();
    void notify_user_of_changes();
};
```

### Import/Export
```cpp
class ConfigurationIO {
public:
    bool import_configuration(const std::string& path);
    bool export_configuration(const std::string& path);
    bool import_from_other_wm(const std::string& wm_name);
    
private:
    void parse_import_format();
    void convert_to_srdwm_format();
    void validate_imported_config();
};
```

## Advanced Features

### Live Preview
- **Real-time updates**: Changes apply immediately
- **Window preview**: See how windows will look
- **Layout preview**: Visualize layout changes
- **Theme preview**: Live theme switching

### Configuration Sync
- **Cloud sync**: Sync settings across devices
- **Version control**: Track configuration changes
- **Backup/restore**: Automatic configuration backups
- **Migration tools**: Import from other window managers

### Accessibility
- **High contrast**: High contrast mode support
- **Screen reader**: Full screen reader compatibility
- **Keyboard navigation**: Complete keyboard navigation
- **Large text**: Scalable interface elements

## Installation and Distribution

### Package Integration
```bash
# Linux (Debian/Ubuntu)
sudo apt install srdwm-settings

# Linux (Arch)
sudo pacman -S srdwm-settings

# Windows (Chocolatey)
choco install srdwm-settings

# macOS (Homebrew)
brew install srdwm-settings
```

### System Integration
```bash
# Linux desktop files
~/.local/share/applications/srdwm-settings.desktop

# Windows registry
HKEY_CURRENT_USER\Software\SRDWM\Settings

# macOS preferences
~/Library/Preferences/com.srdwm.settings.plist
```

## Development

### Building the GUI
```bash
# Linux (GTK4)
meson build -Dgui=true
ninja -C build

# Windows (WinUI 3)
dotnet build src/gui/SRDWM.Settings.Windows

# macOS (SwiftUI)
xcodebuild -project src/gui/SRDWM.Settings.macOS.xcodeproj
```

### Testing
```bash
# Unit tests
ninja -C build test

# Integration tests
ninja -C build integration-test

# GUI tests
ninja -C build gui-test
```

## User Experience

### First Run Experience
1. **Welcome dialog**: Introduction to SRDWM
2. **Quick setup**: Essential settings configuration
3. **Tutorial**: Interactive configuration guide
4. **Import options**: Import from existing setups

### Contextual Help
- **Tooltips**: Hover for help text
- **Help button**: Context-sensitive help
- **Documentation**: Integrated user manual
- **Examples**: Sample configurations

### Error Handling
- **Validation**: Real-time configuration validation
- **Error messages**: Clear, actionable error messages
- **Recovery**: Automatic error recovery
- **Logging**: Detailed error logging

This GUI settings program provides a professional, user-friendly interface that integrates seamlessly with existing system structures while maintaining the power and flexibility of the Lua configuration system.


