use std::{env, fs, path::PathBuf};

fn main() {
    let crate_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    for input in [
        "build.rs",
        "cbindgen.toml",
        "src",
        "../light-bridge-core/src",
        "../light-bridge-core/Cargo.toml",
        "Cargo.toml",
        "../../Cargo.toml",
        "../../Cargo.lock",
    ] {
        println!("cargo:rerun-if-changed={input}");
    }
    println!("cargo:rerun-if-env-changed=LLB_HEADER_DIR");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let bindings = cbindgen::generate(&crate_dir).expect("generate C ABI header");
    let generated = out_dir.join("light_bridge.h");
    bindings.write_to_file(&generated);
    println!(
        "cargo:rustc-env=LLB_GENERATED_HEADER={}",
        generated.display()
    );

    // Optional installation for CMake/C consumers or refreshing the checked-in snapshot.
    // Normal builds only write into Cargo's OUT_DIR, never into the source tree.
    if let Some(destination) = env::var_os("LLB_HEADER_DIR") {
        let destination = PathBuf::from(destination);
        assert!(destination.is_absolute(), "LLB_HEADER_DIR must be absolute");
        fs::create_dir_all(&destination).expect("create C header directory");
        bindings.write_to_file(destination.join("light_bridge.h"));
    }
}
