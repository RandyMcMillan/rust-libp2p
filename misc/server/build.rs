use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    // Re-run this build script if the script changes.
    println!("cargo:rerun-if-changed=run.sh");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("run.sh");

    // Copy the script to the OUT_DIR.
    fs::copy("run.sh", &dest_path).expect("Failed to copy run.sh");

    // Make the copied script executable.
    if cfg!(target_os = "linux") || cfg!(target_os = "macos") {
        Command::new("chmod")
            .arg("+x")
            .arg(&dest_path)
            .status()
            .expect("Failed to make run.sh executable");
    }

    // Tell cargo to include the script in the package.
    println!("cargo:rustc-env=INSTALL_SCRIPT={}", dest_path.display());
}
