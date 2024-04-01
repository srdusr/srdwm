#include "logger.h"
#include <iostream>
#include <fstream>
#include <chrono>
#include <iomanip>
#include <ctime>

// Global logger instance
Logger g_logger;

Logger::Logger() 
    : current_level(LogLevel::INFO)
    , console_enabled(true)
    , file_enabled(false) {
}

Logger::~Logger() {
    if (file_stream) {
        file_stream->flush();
    }
}

void Logger::set_level(LogLevel level) {
    current_level = level;
}

void Logger::debug(const std::string& message) {
    log(LogLevel::DEBUG, message);
}

void Logger::info(const std::string& message) {
    log(LogLevel::INFO, message);
}

void Logger::warning(const std::string& message) {
    log(LogLevel::WARNING, message);
}

void Logger::error(const std::string& message) {
    log(LogLevel::ERROR, message);
}

void Logger::fatal(const std::string& message) {
    log(LogLevel::FATAL, message);
}

void Logger::log(LogLevel level, const std::string& message) {
    if (level >= current_level) {
        write_log(level, message);
    }
}

void Logger::set_output_file(const std::string& filename) {
    output_filename = filename;
    if (file_enabled && !filename.empty()) {
        file_stream = std::make_unique<std::ofstream>(filename, std::ios::app);
        if (!file_stream->good()) {
            std::cerr << "Failed to open log file: " << filename << std::endl;
            file_stream.reset();
        }
    }
}

void Logger::enable_console(bool enable) {
    console_enabled = enable;
}

void Logger::enable_file(bool enable) {
    file_enabled = enable;
    if (enable && !output_filename.empty()) {
        set_output_file(output_filename);
    } else if (!enable) {
        file_stream.reset();
    }
}

std::string Logger::level_to_string(LogLevel level) {
    switch (level) {
        case LogLevel::DEBUG:   return "DEBUG";
        case LogLevel::INFO:    return "INFO";
        case LogLevel::WARNING: return "WARNING";
        case LogLevel::ERROR:   return "ERROR";
        case LogLevel::FATAL:   return "FATAL";
        default:                return "UNKNOWN";
    }
}

std::string Logger::get_timestamp() {
    auto now = std::chrono::system_clock::now();
    auto time_t = std::chrono::system_clock::to_time_t(now);
    auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(
        now.time_since_epoch()) % 1000;
    
    std::stringstream ss;
    ss << std::put_time(std::localtime(&time_t), "%Y-%m-%d %H:%M:%S");
    ss << '.' << std::setfill('0') << std::setw(3) << ms.count();
    return ss.str();
}

void Logger::write_log(LogLevel level, const std::string& message) {
    std::string timestamp = get_timestamp();
    std::string level_str = level_to_string(level);
    std::string log_entry = "[" + timestamp + "] [" + level_str + "] " + message + "\n";
    
    if (console_enabled) {
        if (level == LogLevel::ERROR || level == LogLevel::FATAL) {
            std::cerr << log_entry;
        } else {
            std::cout << log_entry;
        }
    }
    
    if (file_enabled && file_stream && file_stream->good()) {
        *file_stream << log_entry;
        file_stream->flush();
    }
}

// LogStream implementation
LogStream::LogStream(Logger& logger, LogLevel level) 
    : logger(logger), level(level) {
}

LogStream::~LogStream() {
    logger.log(level, stream.str());
}
