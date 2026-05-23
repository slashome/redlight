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
        Some(Command::Status) => println!("status: not yet implemented"),
        Some(Command::Doctor) => cmd_doctor()?,
        Some(Command::Sync {
            dry_run,
            drive_mount,
        }) => cmd_sync(dry_run, drive_mount)?,
        Some(Command::Daemon) => cmd_daemon()?,
        None => println!("rl {} — pass --help", redlight::VERSION),
    }
    Ok(())
}
