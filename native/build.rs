// The app's version: ORBIT_VERSION (a release sets it from its tag), else Cargo.toml's.
fn main() {
    println!("cargo:rerun-if-env-changed=ORBIT_VERSION");
    let version = std::env::var("ORBIT_VERSION")
        .ok()
        .map(|v| v.trim().trim_start_matches('v').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=ORBIT_VERSION={version}");
}
