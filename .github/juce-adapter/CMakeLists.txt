# What the JUCE adapter workflow builds: the example EQ through
# adapters/juce, as the formats the adapter ships on macOS, for both Apple
# architectures, and as a VST3 on Windows. With HAPTO_CHECK_PLUGIN set to
# midi-probe, it builds the MIDI probe the same way instead, for a check of
# MIDI through the adapter in a real host by hand.
cmake_minimum_required(VERSION 3.25)

# The same C runtime as Rust's MSVC builds: the dynamic one.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>DLL")

project(hapto_adapter_check VERSION 0.1.0 LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

include(FetchContent)

# The JUCE commit bench/ builds against; the workflow reads it from there.
set(HAPTO_CHECK_JUCE_COMMIT "" CACHE STRING "The JUCE commit to build against")
if(NOT HAPTO_CHECK_JUCE_COMMIT)
    message(FATAL_ERROR "set HAPTO_CHECK_JUCE_COMMIT")
endif()
FetchContent_Declare(juce
    GIT_REPOSITORY https://github.com/juce-framework/JUCE.git
    GIT_TAG ${HAPTO_CHECK_JUCE_COMMIT})
FetchContent_MakeAvailable(juce)

include(${CMAKE_CURRENT_LIST_DIR}/../../adapters/juce/HaptoJuce.cmake)
if(APPLE)
    set(formats VST3 AU)
else()
    set(formats VST3)
endif()
set(HAPTO_CHECK_PLUGIN "eq" CACHE STRING "The example to build: eq or midi-probe")
hapto_add_juce_plugin(HaptoAdapterCheck
    PACKAGE hapto-example-${HAPTO_CHECK_PLUGIN}
    SETTINGS "${CMAKE_CURRENT_LIST_DIR}/../../examples/${HAPTO_CHECK_PLUGIN}/hapto-juce.cmake"
    FORMATS ${formats})
