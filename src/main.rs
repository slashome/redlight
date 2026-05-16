use anyhow::Result;
use clap::{Parser, Subcommand};

use redlight::bridges::{AdbBridge, MtpBridge};
use redlight::config::{Bridge as BridgeKind, Config, config_dir};

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
}

fn cmd_doctor() -> Result<()> {
    let dir = config_dir();
    let devices_toml = dir.join("devices.toml");
    if !devices_toml.exists() {
        println!(
            "Aucune config trouvée à {}.\nLance `rl init` pour en créer une.",
            dir.display()
        );
        return Ok(());
    }

    let config = Config::load(&dir)?;
    if config.devices.is_empty() {
        println!("Config trouvée mais aucun device déclaré.");
        return Ok(());
    }

    println!(
        "Vérification de {} device(s) configuré(s)…\n",
        config.devices.len()
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

        match check {
            Ok(msg) => println!("  ✓  {name}{label}  bridge {} — {msg}", device.bridge),
            Err(e) => {
                issues += 1;
                println!("  ✗  {name}{label}  bridge {} — {e}", device.bridge);
            }
        }
    }

    println!();
    if issues > 0 {
        println!("{issues} problème(s) détecté(s).");
        std::process::exit(1);
    }
    println!("Tout OK.");
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
        None => println!("rl {} — passe --help", redlight::VERSION),
    }
    Ok(())
}
