use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let libraw_dir = manifest_dir.join("../../3rdpart/libraw");
    let wrapper = manifest_dir.join("src/backends/libraw/wrapper.cpp");

    build_apple_media(&manifest_dir);
    build_libjpeg(&manifest_dir);

    println!("cargo:rerun-if-changed={}", wrapper.display());
    println!(
        "cargo:rerun-if-changed={}",
        libraw_dir.join("libraw").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        libraw_dir.join("internal").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        libraw_dir.join("src").display()
    );

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .include(&libraw_dir)
        .define("LIBRAW_NODLL", None)
        .warnings(false)
        .extra_warnings(false)
        .flag_if_supported("-std=c++11")
        .flag_if_supported("-Wno-deprecated-declarations")
        .flag_if_supported("-Wno-unused-result")
        .flag_if_supported("-Wno-format-overflow")
        .flag_if_supported("-Wno-format-truncation")
        .file(wrapper);

    add_cpp_sources(&mut build, &libraw_dir.join("src"));
    build.compile("oxy_libraw");
}

fn build_libjpeg(manifest_dir: &Path) {
    let source = manifest_dir.join("../../3rdpart/libjpeg-turbo");
    let stitch_wrapper = manifest_dir.join("src/backends/libjpeg/stitch.c");
    let decode_wrapper = manifest_dir.join("src/backends/libjpeg/wrapper.c");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", stitch_wrapper.display());
    println!("cargo:rerun-if-changed={}", decode_wrapper.display());
    println!("cargo:rerun-if-env-changed=NASM");
    let mut config = cmake::Config::new(&source);
    config
        .out_dir(out.join("jpeg-build"))
        .profile("Release")
        .define("ENABLE_SHARED", "OFF")
        .define("ENABLE_STATIC", "ON")
        .define("WITH_TURBOJPEG", "OFF")
        .define("WITH_SIMD", "ON")
        .define("REQUIRE_SIMD", "ON")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .build_target("jpeg-static");
    if let Some(nasm) = env::var_os("NASM") {
        config.define("CMAKE_ASM_NASM_COMPILER", nasm);
    }
    let built = config.build();
    println!(
        "cargo:rustc-link-search=native={}",
        built.join("build/Release").display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        built.join("build").display()
    );
    let msvc = env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|value| value == "msvc");
    cc::Build::new()
        .include(source.join("src"))
        .include(built.join("build"))
        .file(stitch_wrapper)
        .file(decode_wrapper)
        .opt_level(3)
        .compile("oxy_libjpeg");
    println!(
        "cargo:rustc-link-lib=static={}",
        if msvc { "jpeg-static" } else { "jpeg" }
    );
}

fn build_apple_media(manifest_dir: &Path) {
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo must provide the target OS");
    match target_os.as_str() {
        "windows" | "linux" => return,
        "macos" => {}
        target_os => panic!("oxy-media does not support target OS {target_os}"),
    }

    let image_io_wrapper = manifest_dir.join("src/backends/apple_image_io/wrapper.c");
    let core_image_wrapper = manifest_dir.join("src/backends/apple_core_image/wrapper.m");
    println!("cargo:rerun-if-changed={}", image_io_wrapper.display());
    println!("cargo:rerun-if-changed={}", core_image_wrapper.display());
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=framework=CoreImage");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=ImageIO");
    println!("cargo:rustc-link-lib=framework=Accelerate");
    cc::Build::new()
        .warnings(false)
        .extra_warnings(false)
        .file(image_io_wrapper)
        .compile("oxy_apple_image_io");
    cc::Build::new()
        .warnings(false)
        .extra_warnings(false)
        .flag("-fblocks")
        .flag("-fobjc-arc")
        .file(core_image_wrapper)
        .compile("oxy_apple_core_image");
}

fn add_cpp_sources(build: &mut cc::Build, directory: &Path) {
    let mut entries = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();

    for path in entries {
        if path.is_dir() {
            add_cpp_sources(build, &path);
        } else if path.extension().is_some_and(|extension| extension == "cpp")
            && !path
                .file_stem()
                .is_some_and(|stem| stem.to_string_lossy().ends_with("_ph"))
        {
            build.file(path);
        }
    }
}
