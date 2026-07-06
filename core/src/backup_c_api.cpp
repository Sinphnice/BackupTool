#include "backup_c_api.h"

#include "backup/version.hpp"

#include <string>

const char* backup_core_version() noexcept
{
    try {
        static const std::string version = backup::GetCoreVersion();
        return version.c_str();
    } catch (...) {
        return nullptr;
    }
}
