# The Flutter tooling requires that developers have a version of Visual Studio
# installed that includes CMake 3.14 or later. You should not increase this
# version, as doing so will cause the plugin to fail to compile for some
# customers of the plugin.
cmake_minimum_required(VERSION 3.14)

# Project-level configuration.
set(PROJECT_NAME "rust_lib_m3u8_downloader")
project(${PROJECT_NAME} LANGUAGES CXX)

include("../cargokit/cmake/cargokit.cmake")
apply_cargokit(${PROJECT_NAME} ../../../../../../rust rust_lib_m3u8_downloader "")

# List of absolute paths to libraries that should be bundled with the plugin.
# This list could contain prebuilt libraries, or libraries created by an
# external build triggered from this build file.
set(rust_lib_m3u8_downloader_bundled_libraries
  "${${PROJECT_NAME}_cargokit_lib}"
  PARENT_SCOPE
)

# Flutter builds INSTALL.vcxproj with project references disabled. Build the
# FFI artifact from the install script itself before the parent project copies
# the bundled DLL, and fail the install if Cargokit fails.
install(CODE "
  execute_process(
    COMMAND \"${CMAKE_COMMAND}\" --build \"${CMAKE_BINARY_DIR}\"
      --config \"\${CMAKE_INSTALL_CONFIG_NAME}\"
      --target ${PROJECT_NAME}_cargokit
    RESULT_VARIABLE CARGOKIT_BUILD_RESULT
  )
  if(NOT CARGOKIT_BUILD_RESULT EQUAL 0)
    message(FATAL_ERROR \"Cargokit failed with exit code \${CARGOKIT_BUILD_RESULT}\")
  endif()
" COMPONENT Runtime)
