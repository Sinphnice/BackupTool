#pragma once

#ifdef __cplusplus
extern "C" {
const char* backup_core_version() noexcept;
}
#else
const char* backup_core_version(void);
#endif
