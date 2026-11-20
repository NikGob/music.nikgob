// Fixed version values: a git hash and a random id made fresh on every
// commit and every build rebuilt librespot and everything above it each
// time, for constants that only go into the login packet's version text.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=VERGEN_GIT_SHA=0000000");
    println!("cargo:rustc-env=VERGEN_GIT_COMMIT_DATE=2025-01-01");
    println!("cargo:rustc-env=VERGEN_BUILD_DATE=2025-01-01");
    println!("cargo:rustc-env=LIBRESPOT_BUILD_ID=00000000");
}
