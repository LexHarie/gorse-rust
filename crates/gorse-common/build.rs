// Ports: common/floats/Makefile and the //go:generate lines of common/floats/floats_{amd64,arm64}.go @ add21b5

//! Compiles the SIMD kernels of `common/floats` from the C sources in the Go
//! tree, with the flags Go's `goat` translation used, so the Rust build runs
//! the same machine code semantics as the Go assembly.
//!
//! clang is required: the Go assembly was produced by clang, whose
//! floating-point contraction rules (`-ffp-contract=on`, which fuses
//! `x += a * b` within one expression into an FMA) decide the results of the
//! kernels' scalar tails. gcc fuses differently, so it would change results.
//! The Go assembly records clang 21.1.8; contraction and IEEE semantics are
//! the same in every clang release that builds these files, and
//! `tests/floats_golden.rs` checks the results against Go's bit for bit.

use std::path::PathBuf;

fn main() {
    let src = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .join("../../common/floats/src");
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("CARGO_CFG_TARGET_ARCH");
    // (source, library name, flags)
    let kernels: &[(&str, &str, &[&str])] = match arch.as_str() {
        // go:generate go tool goat src/floats_avx.c -O3 -mavx -mf16c
        // go:generate go tool goat src/floats_avx512.c -O3 -mavx -mfma -mavx512f
        "x86_64" => &[
            (
                "floats_avx.c",
                "gorse_floats_avx",
                &["-O3", "-mavx", "-mf16c"],
            ),
            (
                "floats_avx512.c",
                "gorse_floats_avx512",
                &["-O3", "-mavx", "-mfma", "-mavx512f"],
            ),
        ],
        // go:generate go tool goat src/floats_neon.c -O3
        "aarch64" => &[("floats_neon.c", "gorse_floats_neon", &["-O3"])],
        _ => &[],
    };
    // The options goat adds to every translation (internal/translate.go and
    // the target's ClangOptions in internal/{amd64,arm64}/parser.go of
    // github.com/gorse-io/goat @ 41773a8a105b), less the C++-only -fno-rtti.
    let goat: &[&str] = match arch.as_str() {
        "x86_64" => &["-mno-red-zone", "-mstackrealign"],
        "aarch64" => &["-ffixed-x18"],
        _ => &[],
    };
    for (file, lib, flags) in kernels {
        let path = src.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        let mut build = cc::Build::new();
        build
            .compiler("clang")
            .file(&path)
            .warnings(false)
            .opt_level(3);
        for flag in flags
            .iter()
            .chain(&[
                "-fno-jump-tables",
                "-mllvm",
                "-inline-threshold=1000",
                "-fno-asynchronous-unwind-tables",
                "-fno-exceptions",
                "-fno-builtin",
                // Not a goat option: the kernels then include no C library
                // header (only clang's own), so they cross-compile without a
                // target sysroot. It changes no code generation beyond the
                // -fno-builtin above.
                "-ffreestanding",
            ])
            .chain(goat)
        {
            build.flag(flag);
        }
        build.compile(lib);
    }
    println!("cargo:rerun-if-changed=build.rs");
}
