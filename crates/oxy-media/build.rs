use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let libraw_dir = manifest_dir.join("../../3rdpart/libraw");
    let wrapper = manifest_dir.join("src/backends/libraw/wrapper.cpp");

    link_windows_libheif_dependencies();
    build_apple_image_io(&manifest_dir);

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

fn link_windows_libheif_dependencies() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // libheif-sys discovers the vcpkg libheif port, but its Windows helper
    // currently omits the AOM archive enabled by that port. Declare it at the
    // media crate boundary instead of injecting a global RUSTFLAGS link, which
    // would copy the large archive into every downstream Rust staticlib.
    println!("cargo:rustc-link-lib=static=aom");
}

fn build_apple_image_io(manifest_dir: &Path) {
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo must provide the target OS");
    match target_os.as_str() {
        "windows" | "linux" => return,
        "macos" => {}
        target_os => panic!("oxy-media does not support target OS {target_os}"),
    }

    let wrapper = manifest_dir.join("src/backends/apple_image_io/wrapper.c");
    println!("cargo:rerun-if-changed={}", wrapper.display());
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=framework=ImageIO");
    println!("cargo:rustc-link-lib=framework=Accelerate");
    cc::Build::new()
        .warnings(false)
        .extra_warnings(false)
        .file(wrapper)
        .compile("oxy_apple_image_io");
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
