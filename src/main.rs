use anyhow::Result;
use clap::{Parser, Subcommand};

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Init) => println!("init: not yet implemented"),
        Some(Command::Start) => println!("start: not yet implemented"),
        Some(Command::Stop) => println!("stop: not yet implemented"),
        Some(Command::Status) => println!("status: not yet implemented"),
        None => println!("rl {} — passe --help", redlight::VERSION),
    }
    Ok(())
}
