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
    println!("  {PKG_NAME}              Start the terminal music player");
    println!("  {PKG_NAME} scrobble-auth  Obtain a Last.fm / Libre.fm session key");
    println!();
    println!("Configuration: ~/.config/ratune/config.toml");
    println!("{PKG_REPOSITORY}");
    println!();
    println!("Options:");
    println!("  -V, --version  Print version and exit");
    println!("  -h, --help     Print this help and exit");
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
            "scrobble-auth" => return ratune::scrobble_auth().await,
            other => {
                eprintln!("{PKG_NAME}: unknown argument '{other}'");
                eprintln!("Try '{PKG_NAME} --help' for usage.");
                process::exit(2);
            }
        },
        _ => {
            eprintln!("{PKG_NAME}: too many arguments");
            eprintln!("Try '{PKG_NAME} --help' for usage.");
            process::exit(2);
        }
    }

    ratune::run().await
}
