cmake_minimum_required(VERSION 3.20)

if(POLICY CMP0091)
    cmake_policy(SET CMP0091 NEW)
endif()

project(BackupCore LANGUAGES CXX)

option(BACKUP_CORE_BUILD_TESTS "Build Backup Core tests" ON)

file(GLOB_RECURSE BACKUP_CORE_SOURCES CONFIGURE_DEPENDS
    "${CMAKE_CURRENT_SOURCE_DIR}/src/*.cpp"
)

add_library(backup_core STATIC ${BACKUP_CORE_SOURCES})

target_compile_features(backup_core PUBLIC cxx_std_17)

target_include_directories(backup_core
    PUBLIC
        ${CMAKE_CURRENT_SOURCE_DIR}/include
)

set_target_properties(backup_core PROPERTIES
    OUTPUT_NAME backup_core
    POSITION_INDEPENDENT_CODE ON
    MSVC_RUNTIME_LIBRARY "MultiThreadedDLL"
)

if(MSVC)
    target_compile_options(backup_core PRIVATE /W4 /permissive-)
else()
    target_compile_options(backup_core PRIVATE -Wall -Wextra -Wpedantic)
endif()

if(BACKUP_CORE_BUILD_TESTS)
    include(CTest)
endif()
