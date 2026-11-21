# Windows on ARM: what BoringSSL (the browser-like connection, crates/engine/src/browserlike.rs)
# is built with, through config.toml's CMAKE_TOOLCHAIN_FILE_aarch64_pc_windows_msvc.
#
# The cmake crate builds it with Visual Studio's x64-hosted tools, so CMake takes the processor
# for x64's and BoringSSL hands its x64 assembly to MSVC, which can't build it (nor its ARM64
# assembly, which needs clang): this names the processor and builds it in C. That connection is
# the fallback for sites that refuse MDM's own, so its speed matters little.
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR ARM64)
set(OPENSSL_NO_ASM ON CACHE BOOL "BoringSSL in C" FORCE)
# The C runtime built into the programs, as Rust's crt-static (config.toml) and btls-sys do
# without a toolchain file.
set(CMAKE_MSVC_RUNTIME_LIBRARY MultiThreaded)
