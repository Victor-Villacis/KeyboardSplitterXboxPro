//! `ksx session` — control a RUNNING daemon over its named pipe.
//!
//! Thin by contract (docs/CONTROL-SURFACE.md): each verb is one pipe request,
//! and the pipe enqueues the same [`crate::daemon::DaemonCommand`] the tray
//! menu produces. Nothing here validates profiles, resolves plans or touches
//! drivers — the daemon does all of that on its own threads, exactly as if
//! the tray had been clicked.
//!
//! Exit codes: 0 = done, 1 = error (the daemon refused, or the pipe broke
//! mid-conversation), 2 = no daemon control channel (no daemon is running, or
//! the one that is predates `ksx session`).

use crate::daemon::pipe::{client, PIPE_NAME};

pub const EXIT_ERROR: i32 = 1;
pub const EXIT_DAEMON_NOT_RUNNING: i32 = 2;

pub enum Verb {
    Status,
    Start { game: Option<String> },
    Stop,
    Reload,
}

impl Verb {
    fn request(&self) -> serde_json::Value {
        match self {
            Self::Status => serde_json::json!({ "verb": "status" }),
            Self::Start { game: None } => serde_json::json!({ "verb": "start" }),
            Self::Start { game: Some(game) } => {
                serde_json::json!({ "verb": "start", "profile": game })
            }
            Self::Stop => serde_json::json!({ "verb": "stop" }),
            Self::Reload => serde_json::json!({ "verb": "reload" }),
        }
    }
}

pub fn run(verb: Verb, json: bool) -> anyhow::Result<()> {
    match client::request(PIPE_NAME, &verb.request()) {
        Ok(response) => report(&verb, &response, json),
        Err(client::ClientError::NotRunning) => {
            let message = client::ClientError::NotRunning.to_string();
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "code": "daemon-not-running",
                        "error": message,
                    })
                );
            } else {
                eprintln!("{message}");
            }
            std::process::exit(EXIT_DAEMON_NOT_RUNNING);
        }
        Err(err) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "code": "pipe-error",
                        "error": err.to_string(),
                    })
                );
            } else {
                eprintln!("{err}");
            }
            std::process::exit(EXIT_ERROR);
        }
    }
}

/// Render the daemon's answer. `--json` prints the response verbatim — the
/// pipe protocol IS the stable machine interface, so re-shaping it here would
/// just create a second contract to keep.
fn report(verb: &Verb, response: &serde_json::Value, json: bool) -> anyhow::Result<()> {
    let ok = response["ok"].as_bool().unwrap_or(false);
    if json {
        println!("{response}");
    } else if ok {
        match verb {
            Verb::Status => print_status(response),
            _ => println!(
                "{}",
                response["message"].as_str().unwrap_or("done").trim_end()
            ),
        }
    } else {
        eprintln!(
            "{}",
            response["error"].as_str().unwrap_or("the daemon refused")
        );
    }
    if !ok {
        std::process::exit(EXIT_ERROR);
    }
    Ok(())
}

fn print_status(response: &serde_json::Value) {
    // The tooltip is the daemon's own one-line self-description (state, game,
    // worst health note) — reuse it rather than re-deriving a fourth wording.
    if let Some(tooltip) = response["tooltip"].as_str() {
        println!("{tooltip}");
    }
    match response["profiles"].as_array() {
        Some(rows) if !rows.is_empty() => {
            println!("profiles:");
            for row in rows {
                println!(
                    "  {} — {}",
                    row["title"].as_str().unwrap_or("?"),
                    row["detail"].as_str().unwrap_or("")
                );
            }
        }
        _ => println!("profiles: none in games.toml"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_verb_builds_the_documented_request_line() {
        assert_eq!(Verb::Status.request().to_string(), r#"{"verb":"status"}"#);
        assert_eq!(
            Verb::Start { game: None }.request().to_string(),
            r#"{"verb":"start"}"#
        );
        assert_eq!(
            Verb::Start {
                game: Some("Street Fighter".into())
            }
            .request(),
            serde_json::json!({ "verb": "start", "profile": "Street Fighter" })
        );
        assert_eq!(Verb::Stop.request().to_string(), r#"{"verb":"stop"}"#);
        assert_eq!(Verb::Reload.request().to_string(), r#"{"verb":"reload"}"#);
    }
}
