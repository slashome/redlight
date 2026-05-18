use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result};
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
    /// Initialiser la config et enregistrer le service système.
    Init,
    /// Démarrer le daemon.
    Start,
    /// Arrêter le daemon.
    Stop,
    /// Afficher l'état du daemon et des devices.
    Status,
    /// Vérifier que les prérequis système (jmtpfs, adb…) sont disponibles
    /// pour chaque device configuré.
    Doctor,
    /// Lancer une synchronisation host ↔ drive(s).
    ///
    /// Détecte les drives montés (via `/Volumes/<label>` ou
    /// `/media/<label>`) et synchronise chaque item partagé avec le host.
    /// Les phones sont gérés par le daemon (Phase 4).
    Sync {
        /// Affiche ce qui serait fait sans rien transférer.
        #[arg(long)]
        dry_run: bool,
        /// Force un point de montage pour un drive : `--drive-mount materia=/Volumes/MATERIA`.
        /// Répétable pour plusieurs drives.
        #[arg(long = "drive-mount", value_name = "NAME=PATH")]
        drive_mount: Vec<String>,
    },
    /// Lance le daemon en foreground.
    ///
    /// Polle `/Volumes` (macOS) ou `/media` + `/mnt` (Linux) toutes les
    /// ~2s ; chaque drive reconnu déclenche une sync immédiate.
    /// Ctrl-C pour quitter (signal handler propre arrivera avec
    /// Phase 4.5/4.6). Les phones ne sont pas encore détectés.
    Daemon,
}

fn cmd_doctor() -> Result<()> {
    let dir = config_dir();
    let devices_toml = dir.join("devices.toml");
    if !devices_toml.exists() {
        println!(
            "Aucune config trouvée à {}.\nLance {} pour en créer une.",
            dir.display(),
            "rl init".bold()
        );
        return Ok(());
    }

    let config = Config::load(&dir)?;
    if config.devices.is_empty() {
        println!("Config trouvée mais aucun device déclaré.");
        return Ok(());
    }

    println!(
        "{}\n",
        format!(
            "Vérification de {} device(s) configuré(s)…",
            config.devices.len()
        )
        .bold()
    );

    let mut issues = 0;
    for (name, device) in &config.devices {
        let label = device
            .description
            .as_deref()
            .map(|d| format!(" ({d})"))
            .unwrap_or_default();

        let check = match device.bridge {
            BridgeKind::Fs => Ok("aucun prérequis externe".to_string()),
            BridgeKind::Mtp => {
                MtpBridge::check_prerequisites().map(|_| "jmtpfs disponible".to_string())
            }
            BridgeKind::Adb => {
                AdbBridge::check_prerequisites().map(|_| "adb disponible".to_string())
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
            "1 problème détecté.".to_string()
        } else {
            format!("{issues} problèmes détectés.")
        };
        println!("{}", msg.red().bold());
        std::process::exit(1);
    }
    println!("{}", "Tout OK.".green().bold());
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
            "Aucune config trouvée à {}.\nLance {} pour en créer une.",
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
        format!("Dry run. {} paire(s) examinée(s).", summary.pairs)
    } else {
        format!(
            "Terminé. {} paire(s), {} action(s), {} échec(s).",
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
            "Aucune config trouvée à {}.\nLance {} pour en créer une.",
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
        Some(Command::Init) => println!("init: not yet implemented"),
        Some(Command::Start) => println!("start: not yet implemented"),
        Some(Command::Stop) => println!("stop: not yet implemented"),
        Some(Command::Status) => println!("status: not yet implemented"),
        Some(Command::Doctor) => cmd_doctor()?,
        Some(Command::Sync {
            dry_run,
            drive_mount,
        }) => cmd_sync(dry_run, drive_mount)?,
        Some(Command::Daemon) => cmd_daemon()?,
        None => println!("rl {} — passe --help", redlight::VERSION),
    }
    Ok(())
}
