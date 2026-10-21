//! `scpdm-server`: the SolidCraft PDM server and its administration.

use std::io::BufRead;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use scpdm_server::Server;

#[derive(Parser)]
#[command(name = "scpdm-server", about = "SolidCraft PDM server: vaults shared by a small team", version)]
struct Cli {
    /// The data folder (users, vaults); also $SCPDM_SERVER_DATA
    #[arg(long, env = "SCPDM_SERVER_DATA", default_value = "pdm-server-data", global = true)]
    data: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server
    Serve {
        /// Address and port to listen on
        #[arg(long, env = "SCPDM_SERVER_LISTEN", default_value = "0.0.0.0:7380")]
        listen: String,
        /// The name clients show for this server
        #[arg(long, env = "SCPDM_SERVER_NAME", default_value = "SolidCraft PDM")]
        name: String,
    },
    /// Users
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
    /// Vaults
    Vault {
        #[command(subcommand)]
        command: VaultCommand,
    },
    /// Let a user read, change (write) or administer (admin) a vault
    Grant { user: String, vault: String, access: String },
    /// Take a user's access to a vault away
    Revoke { user: String, vault: String },
}

#[derive(Subcommand)]
enum UserCommand {
    /// Add a user (asks for the password)
    Add {
        name: String,
        /// Administrator: every vault, and breaking others' check-outs
        #[arg(long)]
        admin: bool,
        /// Read the password from the first line of standard input (scripts, Docker)
        #[arg(long)]
        password_stdin: bool,
    },
    /// Set a user's password (ends their logins)
    Password {
        name: String,
        #[arg(long)]
        password_stdin: bool,
    },
    Remove { name: String },
    List,
}

#[derive(Subcommand)]
enum VaultCommand {
    Add { name: String },
    List,
}

fn password(from_stdin: bool) -> anyhow::Result<String> {
    if from_stdin {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        return Ok(line.trim_end_matches(['\r', '\n']).to_owned());
    }
    let first = rpassword::prompt_password("Password: ")?;
    let again = rpassword::prompt_password("Again: ")?;
    anyhow::ensure!(first == again, "the passwords differ");
    Ok(first)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve { listen, name } => {
            let server = Server::open(&cli.data, &name)?;
            if server.accounts.users()?.is_empty() {
                eprintln!("No users yet: add one with `scpdm-server user add NAME --admin`.");
            }
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(async {
                let listener = tokio::net::TcpListener::bind(&listen).await?;
                println!("{name}: serving {} on http://{}", cli.data.display(), listener.local_addr()?);
                // Ctrl+C, or SIGTERM from `docker stop` and service managers.
                let shutdown = async {
                    #[cfg(unix)]
                    {
                        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
                        tokio::select! {
                            _ = tokio::signal::ctrl_c() => {}
                            _ = term.recv() => {}
                        }
                    }
                    #[cfg(not(unix))]
                    let _ = tokio::signal::ctrl_c().await;
                    println!("stopping");
                };
                scpdm_server::serve(server, listener, shutdown).await?;
                anyhow::Ok(())
            })?;
        }
        Command::User { command } => {
            let server = Server::open(&cli.data, "")?;
            match command {
                UserCommand::Add { name, admin, password_stdin } => {
                    server.accounts.add_user(&name, &password(password_stdin)?, admin)?;
                    println!("added {name}{}", if admin { " (administrator)" } else { "" });
                }
                UserCommand::Password { name, password_stdin } => {
                    server.accounts.set_password(&name, &password(password_stdin)?)?;
                    println!("new password for {name}");
                }
                UserCommand::Remove { name } => {
                    server.accounts.remove_user(&name)?;
                    println!("removed {name}");
                }
                UserCommand::List => {
                    for (name, admin) in server.accounts.users()? {
                        println!("{name}{}", if admin { "  (administrator)" } else { "" });
                    }
                }
            }
        }
        Command::Vault { command } => {
            let server = Server::open(&cli.data, "")?;
            match command {
                VaultCommand::Add { name } => {
                    server.add_vault(&name)?;
                    println!("added vault {name}");
                }
                VaultCommand::List => {
                    for name in server.vault_names()? {
                        println!("{name}");
                    }
                }
            }
        }
        Command::Grant { user, vault, access } => {
            let server = Server::open(&cli.data, "")?;
            let access = scpdm_server::accounts::parse_access(&access).ok_or_else(|| anyhow::anyhow!("access is read, write or admin"))?;
            anyhow::ensure!(server.vault_names()?.contains(&vault), "no vault {vault}");
            server.accounts.grant(&user, &vault, access)?;
            println!("{user} may {} {vault}", scpdm_server::accounts::access_name(access));
        }
        Command::Revoke { user, vault } => {
            let server = Server::open(&cli.data, "")?;
            server.accounts.revoke(&user, &vault)?;
            println!("{user} may no longer use {vault}");
        }
    }
    Ok(())
}
