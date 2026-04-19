#pragma once

#include <string>
#include <vector>

class FileReader {
public:
    FileReader(const std::string& path) : path_(path) {}
    std::vector<std::string> readAll();

private:
    std::string path_;
};

struct Config {
    int timeout;
    bool verbose;
};

void processFile(const std::string& filename);
