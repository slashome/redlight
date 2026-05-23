use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use colored::Colorize;

use redlight::bridges::{AdbBridge, MtpBridge};
use redlight::config::{Bridge as BridgeKind, Config, config_dir, state_dir};
use redlight::daemon::{self, DaemonOpts};
use redlight::sync::{SyncOpts, run_sync};
use redlight::watcher::{AdbPollWatcher, MultiWatcher, PollWatcher};

#[derive(Parser)]
#[command(name = "rl", version, about = "Redlight — sync USB multi-devices.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Initialise the config directory with a starter host entry.
    ///
    /// Creates `~/.config/redlight/devices.toml`, `items.toml`, and
    /// `bindings.toml`. By default asks two interactive questions
    /// (short name + free-form description) for the host entry; pass
    /// `--name` and/or `--description` to skip the prompts.
    ///
    /// Does NOT register a system service. Use `brew services start
    /// redlight` (macOS) or `systemctl --user enable redlight`
    /// (Linux) after `rl init` to make the daemon start at login.
    Init {
        /// Skip the prompt; use this short name for the host entry.
        #[arg(long)]
        name: Option<String>,
        /// Free-form description for the host entry.
        #[arg(long)]
        description: Option<String>,
    },
    /// Start the daemon.
    Start,
    /// Stop the daemon.
    Stop,
    /// Show daemon and device status.
    Status,
    /// Show which devices have which files for a given item.
    ///
    /// Builds a matrix from the host manifest + per-device snapshots
    /// (written after every successful sync). Cells:
    ///   `✓` present, `·` absent, `!` present with diverging hash,
    ///   `?` device never synced from this host.
    Ls {
        /// Item name (must exist in items.toml).
        item: String,
    },
    /// Show recent sync activity from the local sync log.
    Log {
        /// Maximum number of entries to show (newest first).
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Only show entries for this device.
        #[arg(long)]
        device: Option<String>,
        /// Only show entries for this item.
        #[arg(long)]
        item: Option<String>,
        /// Only show warnings and errors.
        #[arg(long)]
        errors: bool,
    },
    /// Verify that system prerequisites (jmtpfs, adb…) are available for
    /// each configured device.
    Doctor,
    /// Run a host ↔ drive(s) synchronisation.
    ///
    /// Detects mounted drives (via `/Volumes/<label>` or `/media/<label>`)
    /// and syncs each shared item with the host. Phones are handled by
    /// the daemon (Phase 4).
    Sync {
        /// Show what would be done without transferring anything.
        #[arg(long)]
        dry_run: bool,
        /// Force a drive's mount point: `--drive-mount materia=/Volumes/MATERIA`.
        /// Repeatable for multiple drives.
        #[arg(long = "drive-mount", value_name = "NAME=PATH")]
        drive_mount: Vec<String>,
    },
    /// Run the daemon in the foreground.
    ///
    /// Polls `/Volumes` (macOS) or `/media` + `/mnt` (Linux) every ~2s;
    /// each recognised drive triggers an immediate sync. Ctrl-C to
    /// exit (proper signal handler arrives in Phase 4.5/4.6). Phones
    /// aren't detected yet.
    Daemon,
    /// Manage devices in the config.
    Device {
        #[command(subcommand)]
        action: DeviceAction,
    },
    /// Manage items in the config.
    Item {
        #[command(subcommand)]
        action: ItemAction,
    },
    /// Manage bindings (item × device pairs) in the config.
    Bind {
        #[command(subcommand)]
        action: BindAction,
    },
}

#[derive(Subcommand)]
enum DeviceAction {
    /// Add a device to devices.toml. Validates the result; reverts if
    /// the resulting config would be invalid.
    Add(Box<DeviceAddArgs>),
    /// List all configured devices.
    List,
}

#[derive(Subcommand)]
enum ItemAction {
    /// Add an item to items.toml. Reverts on validation error.
    Add(ItemAddArgs),
    /// List all configured items.
    List,
}

#[derive(Subcommand)]
enum BindAction {
    /// Add a binding (an item × device pair) to bindings.toml.
    /// Reverts on validation error.
    Add(BindAddArgs),
    /// List all bindings, grouped by item.
    List,
}

#[derive(clap::Args, Debug)]
struct BindAddArgs {
    /// Item name (must exist in items.toml).
    #[arg(long)]
    item: String,
    /// Device name (must exist in devices.toml).
    #[arg(long)]
    device: String,
    /// Path on the device. Optional; defaults per device type
    /// ($HOME for host, `/` for drive, `/storage/emulated/0/` for phone).
    #[arg(long)]
    path: Option<String>,
    /// read_write | read_only
    #[arg(long)]
    role: String,
    /// Device-specific allowlist glob (repeatable, intersects with item's).
    #[arg(long)]
    include: Vec<String>,
    /// Device-specific denylist glob (repeatable, unions with item's).
    #[arg(long)]
    exclude: Vec<String>,
}

#[derive(clap::Args, Debug)]
struct ItemAddArgs {
    /// Short name for the item (the key in items.toml).
    name: String,
    /// folder | file
    #[arg(long)]
    kind: String,
    /// Free-form tag.
    #[arg(long)]
    category: Option<String>,
    /// Free-form description.
    #[arg(long)]
    description: Option<String>,
    /// Allowlist glob pattern. Repeatable: `--include "**/*.mp3" --include "**/*.flac"`.
    #[arg(long)]
    include: Vec<String>,
    /// Denylist glob pattern. Repeatable.
    #[arg(long)]
    exclude: Vec<String>,
}

#[derive(clap::Args, Debug)]
struct DeviceAddArgs {
    /// Short name for the device (the key in devices.toml).
    name: String,
    /// host | phone | drive
    #[arg(long = "type", value_name = "TYPE")]
    device_type: String,
    /// fs | mtp | adb
    #[arg(long)]
    bridge: String,
    /// USB serial (required for phones).
    #[arg(long)]
    serial: Option<String>,
    /// USB vendor ID (hex, e.g. 18d1).
    #[arg(long)]
    vendor_id: Option<String>,
    /// USB product ID.
    #[arg(long)]
    product_id: Option<String>,
    /// Filesystem volume label (drives: at least this or --volume-uuid).
    #[arg(long)]
    volume_label: Option<String>,
    /// Filesystem volume UUID.
    #[arg(long)]
    volume_uuid: Option<String>,
    /// Hostname for multi-host configs.
    #[arg(long)]
    hostname: Option<String>,
    /// Free-form description.
    #[arg(long)]
    description: Option<String>,
}

fn default_short_hostname() -> String {
    gethostname::gethostname()
        .to_string_lossy()
        .split('.')
        .next()
        .unwrap_or("host")
        .to_string()
}

fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    use std::io::{Write, stdin, stdout};
    let suffix = match default {
        Some(d) => format!(" [{}]", d.dimmed()),
        None => String::new(),
    };
    print!("{label}{suffix}: ");
    stdout().flush()?;

    let mut line = String::new();
    stdin().read_line(&mut line)?;
    let trimmed = line.trim();
    Ok(if trimmed.is_empty() {
        default.map(String::from).unwrap_or_default()
    } else {
        trimmed.to_string()
    })
}

fn init_config_files(dir: &Path, name: &str, hostname: &str, description: &str) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let desc_line = if description.is_empty() {
        String::new()
    } else {
        format!("description = \"{}\"\n", description.replace('"', "\\\""))
    };

    let devices_content = format!(
        "[{name}]\n\
         type = \"host\"\n\
         bridge = \"fs\"\n\
         {desc_line}match.hostname = \"{hostname}\"\n"
    );

    std::fs::write(dir.join("devices.toml"), devices_content)
        .with_context(|| format!("writing {}", dir.join("devices.toml").display()))?;
    std::fs::write(dir.join("items.toml"), "")
        .with_context(|| format!("writing {}", dir.join("items.toml").display()))?;
    std::fs::write(dir.join("bindings.toml"), "")
        .with_context(|| format!("writing {}", dir.join("bindings.toml").display()))?;

    Ok(())
}

fn cmd_init(name_arg: Option<String>, description_arg: Option<String>) -> Result<()> {
    let dir = config_dir();
    if dir.join("devices.toml").exists() {
        bail!(
            "Config already exists at {}. Edit it manually or remove it first.",
            dir.display()
        );
    }

    println!("{}", "Welcome to Redlight.".bold());
    println!();
    println!(
        "Setting up your config at {}.",
        dir.display().to_string().dimmed()
    );
    println!();

    let full_hostname = gethostname::gethostname().to_string_lossy().into_owned();
    let default_name = default_short_hostname();

    let name = match name_arg {
        Some(n) => n,
        None => prompt("Short name for this machine", Some(&default_name))?,
    };
    if name.is_empty() {
        bail!("name is required");
    }

    let description = match description_arg {
        Some(d) => d,
        None => prompt("Description (optional, free-form)", None)?,
    };

    init_config_files(&dir, &name, &full_hostname, &description)
        .with_context(|| format!("writing config to {}", dir.display()))?;

    // Sanity check: it parses.
    Config::load(&dir).context("the freshly-written config did not parse")?;

    println!();
    println!("{} Configuration written.", "✓".green());
    println!();
    println!("{}", "Next steps:".bold());
    println!("  {}    declare a phone or drive", "rl device add".cyan());
    println!("  {}      declare what to sync", "rl item add".cyan());
    println!("  {}    pair items with devices", "rl bind add".cyan());
    println!(
        "  {}        run the watcher in foreground",
        "rl daemon".cyan()
    );
    println!("  {}        check system prerequisites", "rl doctor".cyan());
    println!();
    println!(
        "To start the daemon at login, use {} (macOS) or {} (Linux).",
        "brew services start redlight".dimmed(),
        "systemctl --user enable redlight".dimmed()
    );

    Ok(())
}

fn cmd_device_add(args: DeviceAddArgs) -> Result<()> {
    let dir = config_dir();
    let path = dir.join("devices.toml");
    if !path.exists() {
        bail!("No config found at {}. Run `rl init` first.", dir.display());
    }

    let original =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;

    let mut entry = String::new();
    entry.push_str(&format!("[{}]\n", args.name));
    entry.push_str(&format!("type = \"{}\"\n", args.device_type));
    entry.push_str(&format!("bridge = \"{}\"\n", args.bridge));
    if let Some(d) = &args.description {
        entry.push_str(&format!("description = \"{}\"\n", d.replace('"', "\\\"")));
    }
    for (key, value) in [
        ("match.vendor_id", &args.vendor_id),
        ("match.product_id", &args.product_id),
        ("match.serial", &args.serial),
        ("match.volume_label", &args.volume_label),
        ("match.volume_uuid", &args.volume_uuid),
        ("match.hostname", &args.hostname),
    ] {
        if let Some(v) = value {
            entry.push_str(&format!("{key} = \"{}\"\n", v.replace('"', "\\\"")));
        }
    }

    let new_content = if original.trim().is_empty() {
        entry
    } else if original.ends_with('\n') {
        format!("{original}\n{entry}")
    } else {
        format!("{original}\n\n{entry}")
    };

    std::fs::write(&path, &new_content).with_context(|| format!("writing {}", path.display()))?;

    match Config::load(&dir) {
        Ok(_) => {
            println!(
                "{} added device {} to {}.",
                "✓".green(),
                args.name.bold(),
                path.display().to_string().dimmed()
            );
            Ok(())
        }
        Err(e) => {
            // Revert.
            std::fs::write(&path, &original).ok();
            bail!("config would be invalid, reverted:\n{e:#}");
        }
    }
}

fn cmd_device_list() -> Result<()> {
    let dir = config_dir();
    if !dir.join("devices.toml").exists() {
        println!("No config found at {}.", dir.display());
        println!("Run {} to create one.", "rl init".bold());
        return Ok(());
    }
    let config = Config::load(&dir)?;

    if config.devices.is_empty() {
        println!(
            "No devices configured. Add some with {}.",
            "rl device add".cyan()
        );
        return Ok(());
    }

    println!(
        "{}",
        format!("{} device(s) configured:\n", config.devices.len()).bold()
    );

    let name_width = config.devices.keys().map(String::len).max().unwrap_or(0);

    for (name, device) in &config.devices {
        let m = &device.matcher;
        let mut matchers = Vec::new();
        if let Some(v) = &m.vendor_id {
            matchers.push(format!("vendor_id={v}"));
        }
        if let Some(p) = &m.product_id {
            matchers.push(format!("product_id={p}"));
        }
        if let Some(s) = &m.serial {
            matchers.push(format!("serial={s}"));
        }
        if let Some(l) = &m.volume_label {
            matchers.push(format!("label={l}"));
        }
        if let Some(u) = &m.volume_uuid {
            matchers.push(format!("uuid={u}"));
        }
        if let Some(h) = &m.hostname {
            matchers.push(format!("hostname={h}"));
        }
        let matchers_str = if matchers.is_empty() {
            "—".to_string()
        } else {
            matchers.join(" ")
        };

        println!(
            "  {:<width$}  {:<6}  bridge {:<3}  {}",
            name.bold(),
            device.device_type.to_string().cyan(),
            device.bridge.to_string().cyan(),
            matchers_str.dimmed(),
            width = name_width,
        );
        if let Some(d) = &device.description {
            println!("  {:<width$}    {}", "", d.dimmed(), width = name_width);
        }
    }
    Ok(())
}

fn toml_string_array(patterns: &[String]) -> String {
    let escaped: Vec<String> = patterns
        .iter()
        .map(|p| format!("\"{}\"", p.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    format!("[{}]", escaped.join(", "))
}

fn cmd_item_add(args: ItemAddArgs) -> Result<()> {
    let dir = config_dir();
    let path = dir.join("items.toml");
    if !path.exists() {
        bail!("No config found at {}. Run `rl init` first.", dir.display());
    }

    let original =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;

    let mut entry = String::new();
    entry.push_str(&format!("[{}]\n", args.name));
    entry.push_str(&format!("kind = \"{}\"\n", args.kind));
    if let Some(c) = &args.category {
        entry.push_str(&format!("category = \"{}\"\n", c.replace('"', "\\\"")));
    }
    if let Some(d) = &args.description {
        entry.push_str(&format!("description = \"{}\"\n", d.replace('"', "\\\"")));
    }
    if !args.include.is_empty() {
        entry.push_str(&format!("include = {}\n", toml_string_array(&args.include)));
    }
    if !args.exclude.is_empty() {
        entry.push_str(&format!("exclude = {}\n", toml_string_array(&args.exclude)));
    }

    let new_content = if original.trim().is_empty() {
        entry
    } else if original.ends_with('\n') {
        format!("{original}\n{entry}")
    } else {
        format!("{original}\n\n{entry}")
    };

    std::fs::write(&path, &new_content).with_context(|| format!("writing {}", path.display()))?;

    match Config::load(&dir) {
        Ok(_) => {
            println!(
                "{} added item {} to {}.",
                "✓".green(),
                args.name.bold(),
                path.display().to_string().dimmed()
            );
            Ok(())
        }
        Err(e) => {
            std::fs::write(&path, &original).ok();
            bail!("config would be invalid, reverted:\n{e:#}");
        }
    }
}

fn cmd_item_list() -> Result<()> {
    let dir = config_dir();
    if !dir.join("items.toml").exists() {
        println!("No config found at {}.", dir.display());
        println!("Run {} to create one.", "rl init".bold());
        return Ok(());
    }
    let config = Config::load(&dir)?;

    if config.items.is_empty() {
        println!(
            "No items configured. Add some with {}.",
            "rl item add".cyan()
        );
        return Ok(());
    }

    println!(
        "{}",
        format!("{} item(s) configured:\n", config.items.len()).bold()
    );

    let name_width = config.items.keys().map(String::len).max().unwrap_or(0);

    for (name, item) in &config.items {
        let kind = match item.kind {
            redlight::config::ItemKind::Folder => "folder",
            redlight::config::ItemKind::File => "file",
        };
        let mut bits = Vec::new();
        if !item.include.is_empty() {
            bits.push(format!("include={}", item.include.len()));
        }
        if !item.exclude.is_empty() {
            bits.push(format!("exclude={}", item.exclude.len()));
        }
        if let Some(c) = &item.category {
            bits.push(format!("category={c}"));
        }
        let suffix = if bits.is_empty() {
            "—".to_string()
        } else {
            bits.join(" ")
        };

        println!(
            "  {:<width$}  {:<6}  {}",
            name.bold(),
            kind.cyan(),
            suffix.dimmed(),
            width = name_width,
        );
        if let Some(d) = &item.description {
            println!("  {:<width$}    {}", "", d.dimmed(), width = name_width);
        }
    }
    Ok(())
}

fn cmd_bind_add(args: BindAddArgs) -> Result<()> {
    let dir = config_dir();
    let path = dir.join("bindings.toml");
    if !path.exists() {
        bail!("No config found at {}. Run `rl init` first.", dir.display());
    }

    let original =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;

    let mut entry = String::from("[[binding]]\n");
    entry.push_str(&format!("item = \"{}\"\n", args.item));
    entry.push_str(&format!("device = \"{}\"\n", args.device));
    if let Some(p) = &args.path {
        entry.push_str(&format!("path = \"{}\"\n", p.replace('"', "\\\"")));
    }
    entry.push_str(&format!("role = \"{}\"\n", args.role));
    if !args.include.is_empty() {
        entry.push_str(&format!("include = {}\n", toml_string_array(&args.include)));
    }
    if !args.exclude.is_empty() {
        entry.push_str(&format!("exclude = {}\n", toml_string_array(&args.exclude)));
    }

    let new_content = if original.trim().is_empty() {
        entry
    } else if original.ends_with('\n') {
        format!("{original}\n{entry}")
    } else {
        format!("{original}\n\n{entry}")
    };

    std::fs::write(&path, &new_content).with_context(|| format!("writing {}", path.display()))?;

    match Config::load(&dir) {
        Ok(_) => {
            println!(
                "{} bound item {} on device {}.",
                "✓".green(),
                args.item.bold(),
                args.device.bold()
            );
            Ok(())
        }
        Err(e) => {
            std::fs::write(&path, &original).ok();
            bail!("config would be invalid, reverted:\n{e:#}");
        }
    }
}

fn cmd_bind_list() -> Result<()> {
    use std::collections::BTreeMap;

    let dir = config_dir();
    if !dir.join("bindings.toml").exists() {
        println!("No config found at {}.", dir.display());
        println!("Run {} to create one.", "rl init".bold());
        return Ok(());
    }
    let config = Config::load(&dir)?;

    if config.bindings.is_empty() {
        println!(
            "No bindings configured. Add some with {}.",
            "rl bind add".cyan()
        );
        return Ok(());
    }

    println!(
        "{}",
        format!("{} binding(s) configured:\n", config.bindings.len()).bold()
    );

    // Group by item.
    let mut by_item: BTreeMap<&str, Vec<&redlight::config::Binding>> = BTreeMap::new();
    for b in &config.bindings {
        by_item.entry(b.item.as_str()).or_default().push(b);
    }

    for (item_name, bindings) in &by_item {
        let kind = config
            .items
            .get(*item_name)
            .map(|i| match i.kind {
                redlight::config::ItemKind::Folder => "folder",
                redlight::config::ItemKind::File => "file",
            })
            .unwrap_or("?");
        println!("  {} {}", item_name.bold(), format!("({kind})").dimmed());

        let max_dev = bindings.iter().map(|b| b.device.len()).max().unwrap_or(0);

        for b in bindings {
            let path_display = b.path.as_deref().unwrap_or("(default)");
            let role = match b.role {
                redlight::config::Role::ReadWrite => "read_write",
                redlight::config::Role::ReadOnly => "read_only",
            };
            let mut bits = Vec::new();
            if !b.include.is_empty() {
                bits.push(format!("include={}", b.include.len()));
            }
            if !b.exclude.is_empty() {
                bits.push(format!("exclude={}", b.exclude.len()));
            }
            let filters = if bits.is_empty() {
                String::new()
            } else {
                format!(" · {}", bits.join(" "))
            };

            println!(
                "    {:<width$}  → {}  {}{}",
                b.device.cyan(),
                path_display.dimmed(),
                role.dimmed(),
                filters.dimmed(),
                width = max_dev,
            );
        }
        println!();
    }

    Ok(())
}

fn cmd_status() -> Result<()> {
    use redlight::config::DeviceType;
    use redlight::manifest::Manifest;
    use redlight::snapshot::{self, Snapshot};

    let dir = config_dir();
    if !dir.join("devices.toml").exists() {
        println!("No config found at {}.", dir.display());
        println!("Run {} to create one.", "rl init".bold());
        return Ok(());
    }
    let config = Config::load(&dir)?;
    let state = state_dir();

    // Host (this machine).
    let current_host = current_hostname();
    let host = config
        .devices
        .values()
        .find(|d| {
            d.device_type == DeviceType::Host
                && d.matcher.hostname.as_deref() == Some(current_host.as_str())
        })
        .or_else(|| {
            config
                .devices
                .values()
                .filter(|d| d.device_type == DeviceType::Host)
                .find(|_| {
                    config
                        .devices
                        .values()
                        .filter(|d| d.device_type == DeviceType::Host)
                        .count()
                        == 1
                })
        });

    let host_manifest_path = dir.join("manifest.toml");
    let host_manifest = if host_manifest_path.exists() {
        Some(Manifest::load(&host_manifest_path)?)
    } else {
        None
    };

    println!("{}", "host".bold());
    match host {
        Some(h) => {
            println!(
                "  {} {}  ({})",
                "▣".green(),
                h.name.bold(),
                current_host.dimmed()
            );
            if let Some(desc) = &h.description {
                println!("    {}", desc.dimmed());
            }
            match &host_manifest {
                Some(m) => {
                    let total: usize = m.items.values().map(|i| i.files.len()).sum();
                    println!("    {} item(s), {} file(s) tracked", m.items.len(), total);
                }
                None => println!("    {}", "no manifest yet — never synced".dimmed()),
            }
        }
        None => println!(
            "  {} no host matches this machine's hostname '{}'.\n    Set {} on one of the host entries in devices.toml.",
            "✗".red(),
            current_host,
            "match.hostname".bold()
        ),
    }
    println!();

    // Other devices via snapshots.
    let snaps_dir = snapshot::snapshots_dir(&state);
    let snapshots = snapshot::list(&snaps_dir)?;
    let snap_by_device: std::collections::HashMap<&str, &Snapshot> =
        snapshots.iter().map(|s| (s.device.as_str(), s)).collect();

    let other_devices: Vec<_> = config
        .devices
        .values()
        .filter(|d| d.device_type != DeviceType::Host)
        .collect();

    if other_devices.is_empty() {
        println!(
            "{} (declare some with `rl device add`)",
            "no other devices".dimmed()
        );
        return Ok(());
    }

    println!("{}", "other devices".bold());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for d in &other_devices {
        let dot = match snap_by_device.get(d.name.as_str()) {
            Some(_) => "▣".green(),
            None => "▢".dimmed(),
        };
        let type_label = match d.device_type {
            DeviceType::Drive => "drive",
            DeviceType::Phone => "phone",
            DeviceType::Host => "host",
        };
        println!(
            "  {} {}  {}",
            dot,
            d.name.bold(),
            format!("({type_label})").dimmed()
        );
        if let Some(desc) = &d.description {
            println!("    {}", desc.dimmed());
        }
        match snap_by_device.get(d.name.as_str()) {
            Some(snap) => {
                let total: usize = snap.manifest.items.values().map(|i| i.files.len()).sum();
                println!(
                    "    last seen {} ago via {} · {} item(s), {} file(s)",
                    humanize_age(now - snap.last_synced_at).dimmed(),
                    snap.last_synced_with.dimmed(),
                    snap.manifest.items.len(),
                    total
                );
            }
            None => println!("    {}", "never synced from this host".dimmed()),
        }
    }

    Ok(())
}

fn current_hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

fn humanize_age(secs: i64) -> String {
    if secs < 0 {
        return "moments".into();
    }
    if secs < 60 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 48 {
        return format!("{hours}h");
    }
    let days = hours / 24;
    format!("{days}d")
}

fn cmd_ls(item_name: String) -> Result<()> {
    use redlight::manifest::{FileEntry, Manifest};
    use redlight::snapshot;
    use std::collections::{BTreeSet, HashMap};

    let dir = config_dir();
    if !dir.join("devices.toml").exists() {
        println!("No config found at {}.", dir.display());
        println!("Run {} to create one.", "rl init".bold());
        return Ok(());
    }
    let config = Config::load(&dir)?;
    if !config.items.contains_key(&item_name) {
        bail!("item '{}' not declared in items.toml.", item_name);
    }

    let state = state_dir();
    let host_manifest_path = dir.join("manifest.toml");
    let host_manifest = if host_manifest_path.exists() {
        Some(Manifest::load(&host_manifest_path)?)
    } else {
        None
    };
    let snaps = snapshot::list(&snapshot::snapshots_dir(&state))?;

    // Columns: every device bound to this item, in config order.
    let cols: Vec<&str> = config
        .bindings
        .iter()
        .filter(|b| b.item == item_name)
        .map(|b| b.device.as_str())
        .collect();
    if cols.is_empty() {
        println!("Item '{item_name}' has no bindings yet.");
        println!("Bind it to a device with {}.", "rl bind add".cyan());
        return Ok(());
    }

    // Collect file entries from every available source.
    let mut device_files: HashMap<&str, &HashMap<String, FileEntry>> = HashMap::new();
    if let Some(hm) = &host_manifest
        && let Some(entries) = hm.items.get(&item_name)
    {
        device_files.insert(hm.device.as_str(), &entries.files);
    }
    for s in &snaps {
        if let Some(entries) = s.manifest.items.get(&item_name) {
            device_files.insert(s.device.as_str(), &entries.files);
        }
    }

    // Union of all file paths.
    let mut paths: BTreeSet<&str> = BTreeSet::new();
    for files in device_files.values() {
        for k in files.keys() {
            paths.insert(k.as_str());
        }
    }
    if paths.is_empty() {
        println!("No files tracked for item '{item_name}' yet.");
        return Ok(());
    }

    let path_width = paths.iter().map(|p| p.len()).max().unwrap_or(20).max(20);
    let col_widths: Vec<usize> = cols.iter().map(|c| c.len().max(3)).collect();

    // Header.
    print!("{:<path_width$}", "");
    for (c, w) in cols.iter().zip(&col_widths) {
        print!("  {}", format!("{c:<w$}").bold());
    }
    println!();

    // Rows.
    let mut diverging = 0usize;
    for p in &paths {
        // If every device that has this file agrees on the hash → ✓.
        // Any disagreement → every present cell gets `!` (no
        // arbitrary "winner": the user has to investigate).
        let mut hashes: BTreeSet<&str> = BTreeSet::new();
        for c in &cols {
            if let Some(files) = device_files.get(*c)
                && let Some(fe) = files.get(*p)
            {
                hashes.insert(fe.hash.as_str());
            }
        }
        let row_has_divergence = hashes.len() > 1;

        print!("{p:<path_width$}");
        for (c, w) in cols.iter().zip(&col_widths) {
            let cell = match device_files.get(*c) {
                None => "?".dimmed().to_string(),
                Some(files) => match files.get(*p) {
                    None => "·".dimmed().to_string(),
                    Some(_) => {
                        if row_has_divergence {
                            "!".yellow().to_string()
                        } else {
                            "✓".green().to_string()
                        }
                    }
                },
            };
            print!("  {cell:<w$}");
        }
        println!();
        if row_has_divergence {
            diverging += 1;
        }
    }

    println!();
    let summary = format!(
        "{} file(s) across {} device(s){}",
        paths.len(),
        cols.len(),
        if diverging > 0 {
            format!(", {diverging} diverging")
        } else {
            String::new()
        }
    );
    if diverging > 0 {
        println!("{}", summary.yellow());
    } else {
        println!("{}", summary.dimmed());
    }
    Ok(())
}

fn cmd_log(
    limit: usize,
    device: Option<String>,
    item: Option<String>,
    errors_only: bool,
) -> Result<()> {
    use redlight::sync_log::{LogLevel, Operation, SyncLog};

    let log_path = state_dir().join("sync_log.toml");
    if !log_path.exists() {
        println!("No sync log yet at {}.", log_path.display());
        println!("Run {} or {} first.", "rl sync".bold(), "rl daemon".bold());
        return Ok(());
    }
    let log = SyncLog::open(&log_path)?;

    let mut entries: Vec<_> = log
        .entries()
        .iter()
        .filter(|e| device.as_deref().is_none_or(|d| e.device == d))
        .filter(|e| item.as_deref().is_none_or(|i| e.item == i))
        .filter(|e| !errors_only || matches!(e.level, LogLevel::Warning | LogLevel::Error))
        .collect();
    // Newest first.
    entries.reverse();
    entries.truncate(limit);

    if entries.is_empty() {
        println!("No matching log entries.");
        return Ok(());
    }

    let max_dev = entries.iter().map(|e| e.device.len()).max().unwrap_or(0);
    let max_item = entries.iter().map(|e| e.item.len()).max().unwrap_or(0);

    for e in entries.iter().rev() {
        let icon = match e.level {
            LogLevel::Info => "✓".green(),
            LogLevel::Warning => "⚠".yellow(),
            LogLevel::Error => "✗".red(),
        };
        let op = match e.operation {
            Operation::Create => "create",
            Operation::Update => "update",
            Operation::Delete => "delete",
            Operation::Skip => "skip",
        };
        let ts = format_utc_ts(e.timestamp);
        let err = e
            .error
            .as_deref()
            .map(|s| format!("  ({s})").red().to_string())
            .unwrap_or_default();
        println!(
            "{}  {} {:<6}  {:<dwidth$}  {:<iwidth$}  {}{}",
            ts.dimmed(),
            icon,
            op,
            e.device.cyan(),
            e.item.dimmed(),
            e.file,
            err,
            dwidth = max_dev,
            iwidth = max_item,
        );
    }
    Ok(())
}

/// Format a unix timestamp as `YYYY-MM-DD HH:MM` in UTC.
///
/// We render in UTC (with the `Z` suffix elided for compactness) to
/// avoid pulling chrono / time-tz just for the log viewer. Local
/// timezone display is on the deferred list — see PLAN.md.
fn format_utc_ts(ts: i64) -> String {
    // Days since epoch + remainder seconds. Same algorithm as
    // chrono::NaiveDateTime::from_timestamp_opt — accurate for the
    // Gregorian range we care about.
    if ts <= 0 {
        return "(unknown)".into();
    }
    let secs = ts;
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;

    // Days → calendar date via the algorithm from Howard Hinnant's
    // "date" library (public domain), shifted from 1970-01-01.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if mo <= 2 { y + 1 } else { y };

    format!("{year:04}-{mo:02}-{d:02} {h:02}:{m:02}")
}

fn cmd_doctor() -> Result<()> {
    let dir = config_dir();
    let devices_toml = dir.join("devices.toml");
    if !devices_toml.exists() {
        println!(
            "No config found at {}.\nRun {} to create one.",
            dir.display(),
            "rl init".bold()
        );
        return Ok(());
    }

    let config = Config::load(&dir)?;
    if config.devices.is_empty() {
        println!("Config found but no device declared.");
        return Ok(());
    }

    println!(
        "{}\n",
        format!("Checking {} configured device(s)…", config.devices.len()).bold()
    );

    let mut issues = 0;
    for (name, device) in &config.devices {
        let label = device
            .description
            .as_deref()
            .map(|d| format!(" ({d})"))
            .unwrap_or_default();

        let check = match device.bridge {
            BridgeKind::Fs => Ok("no external prerequisite".to_string()),
            BridgeKind::Mtp => {
                MtpBridge::check_prerequisites().map(|_| "jmtpfs available".to_string())
            }
            BridgeKind::Adb => {
                AdbBridge::check_prerequisites().map(|_| "adb available".to_string())
            }
        };

        let icon = if check.is_ok() {
            "✓".green().bold()
        } else {
            "✗".red().bold()
        };
        let prefix = format!(
            "  {} {}{} · bridge {}",
            icon,
            name.bold(),
            label.dimmed(),
            device.bridge.to_string().cyan(),
        );

        match check {
            Ok(msg) => println!("{prefix} · {}", msg.dimmed()),
            Err(e) => {
                issues += 1;
                println!("{prefix}\n      {}", e.to_string().red());
            }
        }
    }

    println!();
    if issues > 0 {
        let msg = if issues == 1 {
            "1 issue detected.".to_string()
        } else {
            format!("{issues} issues detected.")
        };
        println!("{}", msg.red().bold());
        std::process::exit(1);
    }
    println!("{}", "All good.".green().bold());
    Ok(())
}

fn parse_drive_mounts(raw: &[String]) -> Result<HashMap<String, PathBuf>> {
    raw.iter()
        .map(|s| {
            s.split_once('=')
                .with_context(|| format!("invalid --drive-mount: expected NAME=PATH, got `{s}`"))
                .map(|(name, path)| (name.to_string(), PathBuf::from(path)))
        })
        .collect()
}

fn cmd_sync(dry_run: bool, drive_mount: Vec<String>) -> Result<()> {
    let dir = config_dir();
    if !dir.join("devices.toml").exists() {
        println!(
            "No config found at {}.\nRun {} to create one.",
            dir.display(),
            "rl init".bold()
        );
        return Ok(());
    }

    let config = Config::load(&dir)?;
    let drive_mounts = parse_drive_mounts(&drive_mount)?;
    let opts = SyncOpts {
        dry_run,
        drive_mounts,
        snapshots_dir: Some(redlight::snapshot::snapshots_dir(&state_dir())),
    };

    let summary = run_sync(
        &config,
        &dir.join("manifest.toml"),
        &state_dir().join("sync_log.toml"),
        &opts,
    )?;

    println!();
    if !summary.skipped_devices.is_empty() {
        for s in &summary.skipped_devices {
            println!("{} {}", "skipped:".dimmed(), s.dimmed());
        }
    }

    let line = if opts.dry_run {
        format!("Dry run. {} pair(s) examined.", summary.pairs)
    } else {
        format!(
            "Done. {} pair(s), {} action(s), {} failure(s).",
            summary.pairs, summary.successes, summary.failures
        )
    };
    let styled = if summary.failures > 0 {
        line.red().bold()
    } else {
        line.green().bold()
    };
    println!("{styled}");

    if summary.failures > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_daemon() -> Result<()> {
    let dir = config_dir();
    if !dir.join("devices.toml").exists() {
        println!(
            "No config found at {}.\nRun {} to create one.",
            dir.display(),
            "rl init".bold()
        );
        return Ok(());
    }
    let config = Config::load(&dir)?;

    let mut watcher = MultiWatcher::new();
    watcher.add(Box::new(PollWatcher::for_os()));
    match AdbPollWatcher::new() {
        Ok(adb) => {
            watcher.add(Box::new(adb));
            println!("{}", "  + adb watcher enabled".dimmed());
        }
        Err(_) => {
            println!(
                "{}",
                "  - adb not in PATH, phone watching disabled".dimmed()
            );
        }
    }

    let stop = Arc::new(AtomicBool::new(false));
    let opts = DaemonOpts {
        host_manifest_path: dir.join("manifest.toml"),
        log_path: state_dir().join("sync_log.toml"),
        snapshots_dir: Some(redlight::snapshot::snapshots_dir(&state_dir())),
        stop,
    };
    daemon::run(&config, watcher, &opts)?;
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Init { name, description }) => cmd_init(name, description)?,
        Some(Command::Start) => println!("start: not yet implemented"),
        Some(Command::Stop) => println!("stop: not yet implemented"),
        Some(Command::Status) => cmd_status()?,
        Some(Command::Ls { item }) => cmd_ls(item)?,
        Some(Command::Log {
            limit,
            device,
            item,
            errors,
        }) => cmd_log(limit, device, item, errors)?,
        Some(Command::Doctor) => cmd_doctor()?,
        Some(Command::Sync {
            dry_run,
            drive_mount,
        }) => cmd_sync(dry_run, drive_mount)?,
        Some(Command::Daemon) => cmd_daemon()?,
        Some(Command::Device { action }) => match action {
            DeviceAction::Add(args) => cmd_device_add(*args)?,
            DeviceAction::List => cmd_device_list()?,
        },
        Some(Command::Item { action }) => match action {
            ItemAction::Add(args) => cmd_item_add(args)?,
            ItemAction::List => cmd_item_list()?,
        },
        Some(Command::Bind { action }) => match action {
            BindAction::Add(args) => cmd_bind_add(args)?,
            BindAction::List => cmd_bind_list()?,
        },
        None => println!("rl {} — pass --help", redlight::VERSION),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_utc_ts_unix_epoch() {
        assert_eq!(format_utc_ts(0), "(unknown)");
    }

    #[test]
    fn format_utc_ts_known_dates() {
        // 2024-01-01 00:00:00 UTC = 1704067200
        assert_eq!(format_utc_ts(1_704_067_200), "2024-01-01 00:00");
        // 2026-05-23 08:34:56 UTC = 1779525296
        assert_eq!(format_utc_ts(1_779_525_296), "2026-05-23 08:34");
        // Y2K → 2000-01-01 00:00:00 UTC = 946684800
        assert_eq!(format_utc_ts(946_684_800), "2000-01-01 00:00");
        // End of February in a leap year → 2024-02-29
        assert_eq!(format_utc_ts(1_709_164_800), "2024-02-29 00:00");
    }

    #[test]
    fn format_utc_ts_negative_is_unknown() {
        assert_eq!(format_utc_ts(-1), "(unknown)");
    }
}
