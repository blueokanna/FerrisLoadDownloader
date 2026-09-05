#
# To learn more about a Podspec see http://guides.cocoapods.org/syntax/podspec.html.
# Run `pod lib lint rust_lib_m3u8_downloader.podspec` to validate before publishing.
#
Pod::Spec.new do |s|
  s.name             = 'rust_lib_m3u8_downloader'
  s.version          = '0.0.1'
  s.summary          = 'A new Flutter FFI plugin project.'
  s.description      = <<-DESC
A new Flutter FFI plugin project.
                       DESC
  s.homepage         = 'http://example.com'
  s.license          = { :file => '../LICENSE' }
  s.author           = { 'Your Company' => 'email@example.com' }

  # This will ensure the source files in Classes/ are included in the native
  # builds of apps using this FFI plugin. Podspec does not support relative
  # paths, so Classes contains a forwarder C file that relatively imports
  # `../src/*` so that the C sources can be shared among all target platforms.
  s.source           = { :path => '.' }
  s.source_files = 'Classes/**/*'
  s.dependency 'Flutter'
  s.platform = :ios, '11.0'

  # Flutter.framework does not contain a i386 slice.
  s.pod_target_xcconfig = { 'DEFINES_MODULE' => 'YES', 'EXCLUDED_ARCHS[sdk=iphonesimulator*]' => 'i386' }
  s.swift_version = '5.0'

  s.script_phase = {
    :name => 'Build Rust library',
    # First argument is relative path to the `rust` folder, second is name of rust library
    #
    # iOS cdylib link needs `-undefined dynamic_lookup`:
    #   * `crate-type = ["cdylib", "staticlib"]` makes cargo link a cdylib as a
    #     side product. iOS only consumes the staticlib (force-loaded below),
    #     but the cdylib link must still succeed.
    #   * The crate's iOS-only `#[cfg(target_os = "ios")] extern "C"`
    #     declarations (ferrisload_videotoolbox_*) are implemented in the App's
    #     `VideoToolboxBridge.m`, which is only present at the final App link -
    #     not when cargo links the cdylib.
    #   * cargo only reads `rust/.cargo/config.toml` from its current working
    #     directory, and under Xcode that is NOT the `rust/` folder, so that
    #     file's rustflags never reach this build. A per-target rustflags
    #     environment variable is honored regardless of the working directory.
    :script => 'export CARGO_TARGET_AARCH64_APPLE_IOS_RUSTFLAGS="-C link-arg=-undefined -C link-arg=dynamic_lookup" CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUSTFLAGS="-C link-arg=-undefined -C link-arg=dynamic_lookup" CARGO_TARGET_X86_64_APPLE_IOS_RUSTFLAGS="-C link-arg=-undefined -C link-arg=dynamic_lookup"; sh "$PODS_TARGET_SRCROOT/../cargokit/build_pod.sh" ../../rust rust_lib_m3u8_downloader',
    :execution_position => :before_compile,
    :input_files => ['${BUILT_PRODUCTS_DIR}/cargokit_phony'],
    # Let XCode know that the static library referenced in -force_load below is
    # created by this build step.
    :output_files => ["${BUILT_PRODUCTS_DIR}/librust_lib_m3u8_downloader.a"],
  }
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    # Flutter.framework does not contain a i386 slice.
    'EXCLUDED_ARCHS[sdk=iphonesimulator*]' => 'i386',
    'OTHER_LDFLAGS' => '-force_load ${BUILT_PRODUCTS_DIR}/librust_lib_m3u8_downloader.a -lc++ -lz -liconv -lresolv -framework Security -framework SystemConfiguration -framework CoreFoundation -framework CFNetwork',
  }
end