use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let libraw_dir = manifest_dir.join("../../3rdpart/libraw");
    let wrapper = manifest_dir.join("src/backends/libraw/wrapper.cpp");

    build_apple_media(&manifest_dir);

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
