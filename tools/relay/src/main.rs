use mm2_relay::{healthcheck_http, start, Relay};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

const USAGE: &str = "usage: mm2-relay [--host 127.0.0.1] [--port 2108] [--healthcheck]";

fn parse_args() -> Result<(String, u16, bool), String> {
    let mut host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let mut port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2108);
    let mut healthcheck = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-h" | "--help" => return Err(USAGE.to_string()),
            "--healthcheck" => {
                healthcheck = true;
            }
            "--host" => {
                i += 1;
                host = args.get(i).ok_or("--host requires an address")?.clone();
            }
            "--port" => {
                i += 1;
                let val = args.get(i).ok_or("--port requires a number")?;
                port = val.parse().map_err(|_| format!("invalid port: {val}"))?;
            }
            _ if a.starts_with("--host=") => {
                host = a["--host=".len()..].to_string();
            }
            _ if a.starts_with("--port=") => {
                let val = &a["--port=".len()..];
                port = val.parse().map_err(|_| format!("invalid port: {val}"))?;
            }
            _ => return Err(format!("unknown argument: {a}\n{USAGE}")),
        }
        i += 1;
    }
    Ok((host, port, healthcheck))
}

#[tokio::main]
async fn main() -> ExitCode {
    let (host, port, healthcheck) = match parse_args() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };

    if healthcheck {
        return if healthcheck_http(&host, port).await {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    let trust_proxy = std::env::var("TRUST_PROXY").as_deref() == Ok("1");
    let origins: Vec<String> = std::env::var("ALLOWED_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .map(ToString::to_string)
        .collect();

    let relay = Relay::new(trust_proxy, origins);
    let _srv = match start(&host, port, Some(Arc::clone(&relay))).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error binding {host}:{port}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let sweep_relay = Arc::clone(&relay);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        interval.tick().await;
        loop {
            interval.tick().await;
            sweep_relay.sweep(None);
        }
    });

    std::future::pending::<()>().await;
    ExitCode::SUCCESS
}

