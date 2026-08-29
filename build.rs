fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = std::process::Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|line| line.split_whitespace().nth(1).map(|v| v.trim().to_string()))
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=RUSTC_VERSION={version}");
    println!("cargo:rerun-if-env-changed=RUSTC");
}
