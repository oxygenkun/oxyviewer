use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let libraw_dir = manifest_dir.join("../../native/libraw/0.22.1");
    let wrapper = manifest_dir.join("src/libraw_wrapper.cpp");

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
