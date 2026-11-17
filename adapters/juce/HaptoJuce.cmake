# The generic JUCE adapter's build: turns a Hapto plugin crate into a JUCE
# plugin target, with no product C++.
#
#   hapto_add_juce_plugin(<target>
#       PACKAGE <the plugin's cargo package>
#       SETTINGS <its generated hapto-juce.cmake>
#       FORMATS <JUCE formats...>
#       [MANIFEST_PATH <the Cargo.toml to build from; Hapto's workspace by default>]
#       [LIB_NAME <the crate's library name; the package's, with - as _, by default>]
#       [PRODUCT_NAME <a name other than the plugin's>])
#
# 1. It reads the plugin's settings, which `cargo xtask gen` writes from the
#    plugin's PluginInfo: names, codes, categories, MIDI.
# 2. It builds the package as a static library with only its `cabi` feature,
#    in a cargo target folder of its own under the build tree, so the native
#    exporters' entry points stay out of JUCE's binary. Cargo runs on every
#    build, does nothing when nothing changed, and a Rust change relinks the
#    plugin. On macOS and iOS it builds one library per architecture in
#    CMAKE_OSX_ARCHITECTURES (the host's alone when that is empty), each with
#    `cargo --target`, for CMAKE_OSX_DEPLOYMENT_TARGET when it is set, and
#    joins them with lipo, so a universal plugin links a universal library;
#    each architecture's Rust target must be installed (`rustup target add
#    aarch64-apple-darwin x86_64-apple-darwin`; for iOS `aarch64-apple-ios`,
#    and for its simulator `aarch64-apple-ios-sim` and `x86_64-apple-ios`).
# 3. It adds the JUCE plugin, the adapter's sources (HaptoPluginProcessor) and
#    hapto.h, and links the library.
#
# hapto.h is the one committed beside the hapto-host this file comes with, so
# the plugin must build against that same hapto-host; the adapter checks
# HAPTO_ABI_VERSION at run time all the same. JUCE must be added first. The
# Rust side is always built with cargo's release profile; it takes the C
# runtime from whatever the JUCE target links, so Debug and Release builds of
# the plugin both work.

set(HAPTO_JUCE_ADAPTER_DIR "${CMAKE_CURRENT_LIST_DIR}")
get_filename_component(HAPTO_ROOT "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)

# The commands that build the library once per Apple architecture and join
# the archives into one, into `out_build`, and that archive's path into
# `out_archive`.
function(_hapto_apple_build out_build out_archive cargo_dir lib_name)
    set(cargo ${ARGN})
    set(archs ${CMAKE_OSX_ARCHITECTURES})
    if(NOT archs)
        set(archs ${CMAKE_SYSTEM_PROCESSOR})
    endif()
    if(CMAKE_SYSTEM_NAME STREQUAL "iOS")
        set(os ios)
    else()
        set(os darwin)
    endif()
    # Rust's arm64 simulator target is its own; its x86_64 iOS target is the
    # simulator's already.
    set(arm64_suffix)
    string(TOLOWER "${CMAKE_OSX_SYSROOT}" sysroot)
    if(os STREQUAL "ios" AND sysroot MATCHES "simulator")
        set(arm64_suffix -sim)
    endif()
    set(env)
    if(CMAKE_OSX_DEPLOYMENT_TARGET)
        if(os STREQUAL "ios")
            set(env "${CMAKE_COMMAND}" -E env "IPHONEOS_DEPLOYMENT_TARGET=${CMAKE_OSX_DEPLOYMENT_TARGET}")
        else()
            set(env "${CMAKE_COMMAND}" -E env "MACOSX_DEPLOYMENT_TARGET=${CMAKE_OSX_DEPLOYMENT_TARGET}")
        endif()
    endif()
    set(build)
    set(archives)
    foreach(arch IN LISTS archs)
        if(arch STREQUAL "arm64" OR arch STREQUAL "aarch64")
            set(triple "aarch64-apple-${os}${arm64_suffix}")
        elseif(arch STREQUAL "x86_64")
            set(triple "x86_64-apple-${os}")
        else()
            message(FATAL_ERROR "hapto_add_juce_plugin: no Rust target for the Apple architecture ${arch}")
        endif()
        list(APPEND build COMMAND ${env} ${cargo} --target ${triple})
        list(APPEND archives "${cargo_dir}/${triple}/release/lib${lib_name}.a")
    endforeach()
    set(archive "${cargo_dir}/universal/lib${lib_name}.a")
    find_program(HAPTO_LIPO lipo REQUIRED)
    list(APPEND build
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${cargo_dir}/universal"
        COMMAND "${HAPTO_LIPO}" -create -output "${archive}" ${archives})
    set(${out_build} ${build} PARENT_SCOPE)
    set(${out_archive} "${archive}" PARENT_SCOPE)
endfunction()

function(hapto_add_juce_plugin target)
    cmake_parse_arguments(PARSE_ARGV 1 HAPTO ""
        "PACKAGE;SETTINGS;MANIFEST_PATH;LIB_NAME;PRODUCT_NAME" "FORMATS")
    if(NOT HAPTO_PACKAGE OR NOT HAPTO_SETTINGS OR NOT HAPTO_FORMATS)
        message(FATAL_ERROR "hapto_add_juce_plugin(${target}) needs PACKAGE, SETTINGS and FORMATS")
    endif()
    include("${HAPTO_SETTINGS}")
    if(NOT HAPTO_PRODUCT_NAME)
        set(HAPTO_PRODUCT_NAME "${HAPTO_PLUGIN_NAME}")
    endif()
    if(NOT HAPTO_MANIFEST_PATH)
        set(HAPTO_MANIFEST_PATH "${HAPTO_ROOT}/Cargo.toml")
    endif()
    if(NOT HAPTO_LIB_NAME)
        string(REPLACE "-" "_" HAPTO_LIB_NAME "${HAPTO_PACKAGE}")
    endif()

    find_program(HAPTO_CARGO cargo REQUIRED)
    set(cargo_dir "${CMAKE_BINARY_DIR}/hapto-cargo")
    set(cargo "${HAPTO_CARGO}" rustc --manifest-path "${HAPTO_MANIFEST_PATH}"
        --package "${HAPTO_PACKAGE}" --release --locked
        --no-default-features --features cabi --crate-type staticlib
        --target-dir "${cargo_dir}")
    set(build)
    if(APPLE)
        set(system)
        _hapto_apple_build(build archive "${cargo_dir}" "${HAPTO_LIB_NAME}" ${cargo})
    elseif(WIN32)
        set(archive "${cargo_dir}/release/${HAPTO_LIB_NAME}.lib")
        # What rustc lists as the library's native dependencies
        # (`--print native-static-libs`).
        set(system kernel32 ntdll userenv ws2_32 dbghelp)
        set(build COMMAND ${cargo})
    else()
        set(archive "${cargo_dir}/release/lib${HAPTO_LIB_NAME}.a")
        set(system pthread dl m)
        set(build COMMAND ${cargo})
    endif()

    # Beside the manifest, so rustup takes the toolchain its project pins.
    get_filename_component(manifest_dir "${HAPTO_MANIFEST_PATH}" DIRECTORY)
    add_custom_target(${target}_rust
        ${build}
        WORKING_DIRECTORY "${manifest_dir}"
        BYPRODUCTS "${archive}"
        COMMENT "Building ${HAPTO_PACKAGE} as a static library"
        USES_TERMINAL
        VERBATIM)
    add_library(${target}_hapto STATIC IMPORTED)
    set_target_properties(${target}_hapto PROPERTIES IMPORTED_LOCATION "${archive}")
    target_link_libraries(${target}_hapto INTERFACE ${system})

    juce_add_plugin(${target}
        PRODUCT_NAME "${HAPTO_PRODUCT_NAME}"
        COMPANY_NAME "${HAPTO_PLUGIN_VENDOR}"
        COMPANY_WEBSITE "${HAPTO_PLUGIN_URL}"
        COMPANY_EMAIL "${HAPTO_PLUGIN_EMAIL}"
        DESCRIPTION "${HAPTO_PLUGIN_DESCRIPTION}"
        VERSION "${HAPTO_PLUGIN_VERSION}"
        BUNDLE_ID "${HAPTO_PLUGIN_ID}"
        PLUGIN_MANUFACTURER_CODE "${HAPTO_PLUGIN_MANUFACTURER_CODE}"
        PLUGIN_CODE "${HAPTO_PLUGIN_CODE}"
        FORMATS ${HAPTO_FORMATS}
        IS_SYNTH ${HAPTO_PLUGIN_IS_SYNTH}
        NEEDS_MIDI_INPUT ${HAPTO_PLUGIN_MIDI_INPUT}
        NEEDS_MIDI_OUTPUT ${HAPTO_PLUGIN_MIDI_OUTPUT}
        IS_MIDI_EFFECT FALSE
        COPY_PLUGIN_AFTER_BUILD FALSE
        VST3_CATEGORIES ${HAPTO_PLUGIN_VST3_CATEGORIES})

    target_sources(${target} PRIVATE "${HAPTO_JUCE_ADAPTER_DIR}/HaptoPluginProcessor.cpp")
    target_include_directories(${target} PRIVATE
        "${HAPTO_JUCE_ADAPTER_DIR}"
        "${HAPTO_ROOT}/crates/host/include")
    target_compile_definitions(${target} PUBLIC
        JUCE_WEB_BROWSER=0
        JUCE_USE_CURL=0
        JUCE_VST3_CAN_REPLACE_VST2=0)
    target_link_libraries(${target}
        PRIVATE ${target}_hapto juce::juce_audio_processors
        PUBLIC juce::juce_recommended_config_flags juce::juce_recommended_lto_flags
            juce::juce_recommended_warning_flags)
    add_dependencies(${target} ${target}_rust)
endfunction()
