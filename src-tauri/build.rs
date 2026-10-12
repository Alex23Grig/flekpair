fn main() {
    // Where pairing files are uploaded to is compiled in: see `send.rs`.
    println!("cargo:rerun-if-env-changed=FLEKPAIR_SERVER");
    if std::env::var("FLEKPAIR_SERVER").map_or(true, |server| server.trim().is_empty()) {
        println!(
            "cargo:warning=FLEKPAIR_SERVER is not set: this build saves pairing files and uploads none"
        );
    }

    tauri_build::build()
}
