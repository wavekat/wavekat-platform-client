//! Manual smoke test for the platform client. Not in CI — needs a
//! reachable platform and (for `login`) a human at a browser.
//!
//! Usage:
//!
//!   cargo run --example smoke -- login
//!   cargo run --example smoke -- whoami --token wkcli_xxx
//!   cargo run --example smoke -- revoke --token wkcli_xxx
//!
//! The base URL defaults to `https://platform.wavekat.com`; override
//! with `--base-url` or `WK_BASE_URL`.

use std::env;
use std::process::ExitCode;

use wavekat_platform_client::{loopback_handshake, Client, HandshakeOptions, Token};

const DEFAULT_BASE_URL: &str = "https://platform.wavekat.com";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to start tokio runtime: {e}");
            return ExitCode::from(1);
        }
    };
    match rt.block_on(run(args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

async fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut iter = args.into_iter();
    let cmd = iter
        .next()
        .ok_or("missing subcommand: login | whoami | revoke")?;

    let mut token: Option<String> = None;
    let mut base_url: Option<String> = None;
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--token" => token = iter.next(),
            "--base-url" => base_url = iter.next(),
            other => return Err(format!("unknown flag: {other}").into()),
        }
    }
    let base_url = base_url
        .or_else(|| env::var("WK_BASE_URL").ok())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

    match cmd.as_str() {
        "login" => login(&base_url).await,
        "whoami" => {
            let t = token.ok_or("whoami requires --token")?;
            whoami(&base_url, t).await
        }
        "revoke" => {
            let t = token.ok_or("revoke requires --token")?;
            revoke(&base_url, t).await
        }
        other => Err(format!("unknown subcommand: {other}").into()),
    }
}

async fn login(base_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let pending = loopback_handshake(base_url, HandshakeOptions::default())?;
    let url = pending.url().to_string();
    println!("Opening {base_url} in your browser to sign in…");
    if let Err(e) = webbrowser::open(&url) {
        eprintln!("(couldn't open the browser automatically: {e})");
        println!("Open this URL manually:\n  {url}\n");
    } else {
        println!("If it didn't open, use:\n  {url}\n");
    }
    println!("Waiting for the browser to redirect back (Ctrl-C to cancel)…");
    let outcome = pending.wait().await?;
    println!("Got token: {:?}", outcome.token);
    if let Some(login) = &outcome.login {
        println!("Login (echoed from platform): {login}");
    }
    let client = Client::new(base_url, outcome.token)?;
    let me = client.whoami().await?;
    println!("Signed in as {} ({}, role: {})", me.login, me.id, me.role);
    Ok(())
}

async fn whoami(base_url: &str, token: String) -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new(base_url, Token::new(token))?;
    let me = client.whoami().await?;
    println!("login: {}", me.login);
    println!("id:    {}", me.id);
    println!("name:  {}", me.name.as_deref().unwrap_or("-"));
    println!("email: {}", me.email.as_deref().unwrap_or("-"));
    println!("role:  {}", me.role);
    Ok(())
}

async fn revoke(base_url: &str, token: String) -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new(base_url, Token::new(token))?;
    client.revoke_current_token().await?;
    println!("Token revoked.");
    Ok(())
}
