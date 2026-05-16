//! Manual ADB smoke test.
//!
//! Prerequisites:
//!  - `adb` installed (`brew install android-platform-tools` or
//!    `apt install android-tools-adb`)
//!  - Developer Options + USB Debugging enabled on the phone
//!  - Host authorized on the phone (the first `adb` command triggers a
//!    prompt; accept it)
//!
//! Then plug the phone in and run:
//!
//!     cargo run --example adb_probe
//!
//! Lists `/sdcard/` and prints the first 20 entries with sizes.

use std::path::Path;

use redlight::bridges::{AdbBridge, Bridge};

fn main() -> anyhow::Result<()> {
    AdbBridge::check_prerequisites()?;

    // No serial → adb picks the only connected device (errors if multiple).
    let bridge = AdbBridge::new(None);

    let start = Path::new("/sdcard/");
    println!("Listing {} ...", start.display());
    let files = bridge.list_files(start)?;
    println!("Total files: {}", files.len());

    println!("\nFirst 20 entries:");
    for f in files.iter().take(20) {
        println!(
            "  {}  {} bytes  mtime={}",
            f.path.display(),
            f.size,
            f.mtime
        );
    }

    Ok(())
}
