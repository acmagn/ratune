//! Thin binary wrapper around [`ratune::run`].

use std::process;

use anyhow::Result;

const PKG_NAME: &str = env!("CARGO_PKG_NAME");
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const PKG_REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

fn print_help() {
    println!("{PKG_NAME} {PKG_VERSION}");
    println!();
    println!("Usage:");
    println!("  {PKG_NAME}                               Start the terminal music player");
    println!("  {PKG_NAME} stop                          Stop background playback");
    println!(
        "  {PKG_NAME} status                        Show whether the playback daemon is running"
    );
    println!(
        "  {PKG_NAME} daemon                        Run the playback daemon in the foreground"
    );
    println!("  {PKG_NAME} scrobble-api-secret             Prompt for API shared secret");
    println!("  {PKG_NAME} scrobble-api-secret --save-keyring  …and store it in the OS keyring");
    println!(
        "  {PKG_NAME} scrobble-auth                     Obtain a Last.fm / Libre.fm session key"
    );
    println!("  {PKG_NAME} scrobble-auth --save-keyring         …and store it in the OS keyring");
    println!();
    println!("Configuration: ~/.config/ratune/config.toml");
    println!("{PKG_REPOSITORY}");
    println!();
    println!("Options:");
    println!("  -V, --version  Print version and exit");
    println!("  -h, --help     Print this help and exit");
}

async fn run_scrobble_subcommand(cmd: &str, save_keyring: bool) -> Result<()> {
    match cmd {
        "scrobble-api-secret" => ratune::scrobble_api_secret(save_keyring),
        "scrobble-auth" => ratune::scrobble_auth(save_keyring).await,
        other => {
            eprintln!("{PKG_NAME}: unknown scrobble subcommand '{other}'");
            eprintln!("Try '{PKG_NAME} --help' for usage.");
            process::exit(2);
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => {}
        [a] => match a.as_str() {
            "-V" | "--version" => {
                println!("{PKG_NAME} {PKG_VERSION}");
                return Ok(());
            }
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            "scrobble-api-secret" | "scrobble-auth" => {
                return run_scrobble_subcommand(a, false).await;
            }
            "stop" => {
                return match ratune::stop_daemon() {
                    Ok(true) => {
                        println!("Stopped ratune playback daemon");
                        Ok(())
                    }
                    Ok(false) => {
                        println!("No ratune playback daemon is running");
                        Ok(())
                    }
                    Err(e) => {
                        eprintln!("error: {e:#}");
                        process::exit(1);
                    }
                };
            }
            "status" => {
                println!("{}", ratune::daemon_status());
                return Ok(());
            }
            "daemon" => {
                #[cfg(target_os = "macos")]
                {
                    return ratune::run_daemon_macos();
                }
                #[cfg(not(target_os = "macos"))]
                {
                    return ratune::run_daemon().await;
                }
            }
            other => {
                eprintln!("{PKG_NAME}: unknown argument '{other}'");
                eprintln!("Try '{PKG_NAME} --help' for usage.");
                process::exit(2);
            }
        },
        [a, b] if b == "--save-keyring" => {
            return run_scrobble_subcommand(a, true).await;
        }
        _ => {
            eprintln!("{PKG_NAME}: too many arguments");
            eprintln!("Try '{PKG_NAME} --help' for usage.");
            process::exit(2);
        }
    }

    ratune::run().await
}
