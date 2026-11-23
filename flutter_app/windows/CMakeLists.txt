# LanVault Windows Desktop CMake 极限性能 Release 构建配置（8C16T / MSVC AVX2 + LTCG）
cmake_minimum_required(VERSION 3.14)
project(lanvault_app LANGUAGES CXX)

# 生成的 Windows 可执行文件名称
set(BINARY_NAME "lanvault_app")

# 显式启用现代策略，确保多配置生成器（MSVC）行为一致
cmake_policy(VERSION 3.14...3.25)

# 定义构建类型（默认 Release）
get_property(IS_MULTICONFIG GLOBAL PROPERTY GENERATOR_IS_MULTI_CONFIG)
if(IS_MULTICONFIG)
  set(CMAKE_CONFIGURATION_TYPES "Debug;Profile;Release"
    CACHE STRING "" FORCE)
else()
  if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
    set(CMAKE_BUILD_TYPE "Release" CACHE
      STRING "Flutter build mode" FORCE)
    set_property(CACHE CMAKE_BUILD_TYPE PROPERTY STRINGS
      "Debug" "Profile" "Release")
  endif()
endif()

# 为 Profile 模式配置与 Release 一致的基础标志
set(CMAKE_EXE_LINKER_FLAGS_PROFILE "${CMAKE_EXE_LINKER_FLAGS_RELEASE}")
set(CMAKE_SHARED_LINKER_FLAGS_PROFILE "${CMAKE_SHARED_LINKER_FLAGS_RELEASE}")
set(CMAKE_C_FLAGS_PROFILE "${CMAKE_C_FLAGS_RELEASE}")
set(CMAKE_CXX_FLAGS_PROFILE "${CMAKE_CXX_FLAGS_RELEASE}")

# ---------------------------------------------------------------------------
# MSVC 极限性能 Release 编译器与链接器标志（8C16T + AVX2 + 全程序优化 LTCG）
# ---------------------------------------------------------------------------
if(MSVC)
  # 编译器最高优化：/O2 /Ob2 /Oi /Ot /GL /arch:AVX2 /MP16
  set(LANVAULT_MSVC_RELEASE_COMPILE_FLAGS
    "/O2 /Ob2 /Oi /Ot /GL /arch:AVX2 /MP16")
  # 链接器全程序优化与死代码/重复 COMDAT 折叠：/LTCG /OPT:REF /OPT:ICF
  set(LANVAULT_MSVC_RELEASE_LINK_FLAGS
    "/LTCG /OPT:REF /OPT:ICF")

  string(APPEND CMAKE_C_FLAGS_RELEASE " ${LANVAULT_MSVC_RELEASE_COMPILE_FLAGS}")
  string(APPEND CMAKE_CXX_FLAGS_RELEASE " ${LANVAULT_MSVC_RELEASE_COMPILE_FLAGS}")
  string(APPEND CMAKE_EXE_LINKER_FLAGS_RELEASE " ${LANVAULT_MSVC_RELEASE_LINK_FLAGS}")
  string(APPEND CMAKE_SHARED_LINKER_FLAGS_RELEASE " ${LANVAULT_MSVC_RELEASE_LINK_FLAGS}")
  string(APPEND CMAKE_STATIC_LINKER_FLAGS_RELEASE " /LTCG")
endif()

# 使用 Unicode 字符集
add_definitions(-DUNICODE -D_UNICODE)

# 标准目标编译设置（供 runner 与插件目标调用）
function(APPLY_STANDARD_SETTINGS TARGET)
  target_compile_features(${TARGET} PUBLIC cxx_std_17)
  target_compile_options(${TARGET} PRIVATE /W4 /WX /wd"4100")
  target_compile_options(${TARGET} PRIVATE /EHsc)
  target_compile_definitions(${TARGET} PRIVATE "_HAS_EXCEPTIONS=0")
  target_compile_definitions(${TARGET} PRIVATE "$<$<CONFIG:Debug>:_DEBUG>")
  if(MSVC)
    target_compile_options(${TARGET} PRIVATE
      "$<$<CONFIG:Release>:/O2;/Ob2;/Oi;/Ot;/GL;/arch:AVX2;/MP16>")
    target_link_options(${TARGET} PRIVATE
      "$<$<CONFIG:Release>:/LTCG;/OPT:REF;/OPT:ICF>")
  endif()
endfunction()

# Flutter 引擎与包装库构建
set(FLUTTER_MANAGED_DIR "${CMAKE_CURRENT_SOURCE_DIR}/flutter")
add_subdirectory(${FLUTTER_MANAGED_DIR})

# Windows Runner 主工程
add_subdirectory("runner")

# 自动生成插件规则（若由 `flutter pub get` 生成）
if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/flutter/generated_plugins.cmake")
  include(flutter/generated_plugins.cmake)
endif()

# ---------------------------------------------------------------------------
# Rust 原生 FFI 动态库 (lanvault_ffi.dll) 自动定位与 Release 产物拷贝
# 优先匹配 target/x86_64-pc-windows-msvc/release/lanvault_ffi.dll，
# 次选 target/release/lanvault_ffi.dll
# ---------------------------------------------------------------------------
get_filename_component(LANVAULT_REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)
set(LANVAULT_FFI_DLL_TRIPLE
  "${LANVAULT_REPO_ROOT}/target/x86_64-pc-windows-msvc/release/lanvault_ffi.dll")
set(LANVAULT_FFI_DLL_DEFAULT
  "${LANVAULT_REPO_ROOT}/target/release/lanvault_ffi.dll")

set(LANVAULT_FFI_DLL "")
if(EXISTS "${LANVAULT_FFI_DLL_TRIPLE}")
  set(LANVAULT_FFI_DLL "${LANVAULT_FFI_DLL_TRIPLE}")
elseif(EXISTS "${LANVAULT_FFI_DLL_DEFAULT}")
  set(LANVAULT_FFI_DLL "${LANVAULT_FFI_DLL_DEFAULT}")
endif()

if(LANVAULT_FFI_DLL)
  message(STATUS "[LanVault] Found native FFI library: ${LANVAULT_FFI_DLL}")
  # `runner` is a separate CMake sub-project, so a POST_BUILD hook on its target
  # cannot be attached from this directory; a custom target keeps the copy step
  # here and orders it after the executable.
  add_custom_target(copy_lanvault_ffi ALL
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
      "${LANVAULT_FFI_DLL}"
      "$<TARGET_FILE_DIR:${BINARY_NAME}>/lanvault_ffi.dll"
    DEPENDS ${BINARY_NAME}
    COMMENT "[LanVault] Copying lanvault_ffi.dll to Windows output directory"
  )
else()
  message(WARNING
    "[LanVault] lanvault_ffi.dll not found at:\n"
    "  - ${LANVAULT_FFI_DLL_TRIPLE}\n"
    "  - ${LANVAULT_FFI_DLL_DEFAULT}\n"
    "Run `cargo build --release -p lanvault-ffi --target x86_64-pc-windows-msvc -j 16` before packaging.")
endif()

# ---------------------------------------------------------------------------
# Flutter Windows Release 安装与打包规则
# ---------------------------------------------------------------------------
set(BUILD_BUNDLE_DIR "$<TARGET_FILE_DIR:${BINARY_NAME}>")
set(CMAKE_VS_INCLUDE_INSTALL_TO_DEFAULT_BUILD 1)
if(CMAKE_INSTALL_PREFIX_INITIALIZED_TO_DEFAULT)
  set(CMAKE_INSTALL_PREFIX "${BUILD_BUNDLE_DIR}" CACHE PATH "..." FORCE)
endif()

set(INSTALL_BUNDLE_DATA_DIR "${CMAKE_INSTALL_PREFIX}/data")
set(INSTALL_BUNDLE_LIB_DIR "${CMAKE_INSTALL_PREFIX}")

install(TARGETS ${BINARY_NAME} RUNTIME DESTINATION "${CMAKE_INSTALL_PREFIX}"
  COMPONENT Runtime)

install(FILES "${FLUTTER_ICU_DATA_FILE}" DESTINATION "${INSTALL_BUNDLE_DATA_DIR}"
  COMPONENT Runtime)

install(FILES "${FLUTTER_LIBRARY}" DESTINATION "${INSTALL_BUNDLE_LIB_DIR}"
  COMPONENT Runtime)

if(LANVAULT_FFI_DLL)
  install(FILES "${LANVAULT_FFI_DLL}" DESTINATION "${INSTALL_BUNDLE_LIB_DIR}"
    COMPONENT Runtime)
endif()

if(PLUGIN_BUNDLED_LIBRARIES)
  install(FILES "${PLUGIN_BUNDLED_LIBRARIES}"
    DESTINATION "${INSTALL_BUNDLE_LIB_DIR}"
    COMPONENT Runtime)
endif()

# Flutter 静态资源目录
set(FLUTTER_ASSET_DIR_NAME "flutter_assets")
install(CODE "
  file(REMOVE_RECURSE \"${INSTALL_BUNDLE_DATA_DIR}/${FLUTTER_ASSET_DIR_NAME}\")
  " COMPONENT Runtime)
install(DIRECTORY "${PROJECT_BUILD_DIR}/${FLUTTER_ASSET_DIR_NAME}"
  DESTINATION "${INSTALL_BUNDLE_DATA_DIR}" COMPONENT Runtime)

# Profile / Release 模式附带 AOT 动态库 (app.so)
install(FILES "${AOT_LIBRARY}" DESTINATION "${INSTALL_BUNDLE_DATA_DIR}"
  CONFIGURATIONS Profile;Release
  COMPONENT Runtime)
