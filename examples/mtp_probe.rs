//! Manual MTP smoke test.
//!
//! Plug in your phone (with MTP / "File transfer" enabled), then run:
//!
//!     cargo run --example mtp_probe
//!
//! Mounts the first connected MTP device via `jmtpfs`, lists the root, and
//! prints a few entries. Useful to verify that the prerequisites
//! (`jmtpfs` + FUSE) are installed and that the bridge actually talks to
//! the device.

use std::path::Path;

use redlight::bridges::{Bridge, MtpBridge, MtpDeviceMatcher};

fn main() -> anyhow::Result<()> {
    // Matcher is stored for future disambiguation but currently unused —
    // jmtpfs picks the first device.
    let matcher = MtpDeviceMatcher {
        vendor_id: String::new(),
        product_id: String::new(),
        serial: None,
    };
    let bridge = MtpBridge::new(matcher);

    println!("Mounting MTP device...");
    let mounted = bridge.mount()?;
    println!("Mounted at {}", mounted.mount_point().display());

    println!("\nListing /...");
    let files = mounted.list_files(Path::new("/"))?;
    println!("Total files: {}", files.len());

    println!("\nFirst 20 entries:");
    for f in files.iter().take(20) {
        println!("  {}  {} bytes", f.path.display(), f.size);
    }

    println!("\nUnmounting on exit...");
    Ok(())
}
