fn main() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    // Tell cargo to pass our linker script to the linker...
    println!("cargo:rustc-link-arg=-Tlinker-{arch}.ld");
    // ...and to re-run this build script if it changes.
    println!("cargo:rerun-if-changed=linker-{arch}.ld");
}
