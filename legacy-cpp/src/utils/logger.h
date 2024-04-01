#ifndef SRDWM_LOGGER_H
#define SRDWM_LOGGER_H

#include <string>
#include <sstream>
#include <memory>

// Log levels
enum class LogLevel {
    DEBUG,
    INFO,
    WARNING,
    ERROR,
    FATAL
};

// Logger class
class Logger {
public:
    Logger();
    ~Logger();
    
    // Set log level
    void set_level(LogLevel level);
    
    // Logging methods
    void debug(const std::string& message);
    void info(const std::string& message);
    void warning(const std::string& message);
    void error(const std::string& message);
    void fatal(const std::string& message);
    
    // Log with level
    void log(LogLevel level, const std::string& message);
    
    // Set output file
    void set_output_file(const std::string& filename);
    
    // Enable/disable console output
    void enable_console(bool enable);
    
    // Enable/disable file output
    void enable_file(bool enable);

private:
    LogLevel current_level;
    bool console_enabled;
    bool file_enabled;
    std::string output_filename;
    std::unique_ptr<std::ostream> file_stream;
    
    std::string level_to_string(LogLevel level);
    std::string get_timestamp();
    void write_log(LogLevel level, const std::string& message);
};

// Global logger instance
extern Logger g_logger;

// Convenience macros
#define LOG_DEBUG(msg) g_logger.debug(msg)
#define LOG_INFO(msg) g_logger.info(msg)
#define LOG_WARNING(msg) g_logger.warning(msg)
#define LOG_ERROR(msg) g_logger.error(msg)
#define LOG_FATAL(msg) g_logger.fatal(msg)

// Stream-based logging
class LogStream {
public:
    LogStream(Logger& logger, LogLevel level);
    ~LogStream();
    
    template<typename T>
    LogStream& operator<<(const T& value) {
        stream << value;
        return *this;
    }

private:
    Logger& logger;
    LogLevel level;
    std::ostringstream stream;
};

// Stream logging macros
#define LOG_STREAM_DEBUG LogStream(g_logger, LogLevel::DEBUG)
#define LOG_STREAM_INFO LogStream(g_logger, LogLevel::INFO)
#define LOG_STREAM_WARNING LogStream(g_logger, LogLevel::WARNING)
#define LOG_STREAM_ERROR LogStream(g_logger, LogLevel::ERROR)
#define LOG_STREAM_FATAL LogStream(g_logger, LogLevel::FATAL)

#endif // SRDWM_LOGGER_H
