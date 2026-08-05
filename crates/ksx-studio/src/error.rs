use std::net::SocketAddr;

/// Everything [`crate::serve`] can refuse or fail on.
#[derive(Debug, thiserror::Error)]
pub enum StudioError {
    /// Studio is localhost-only until the LAN story ships with a pairing
    /// token (docs/ENHANCEMENTS.md E7); refusing here means no code path can
    /// expose the listener by accident.
    #[error(
        "refusing to bind {bind}: ksx Studio serves 127.0.0.1 only \
         (LAN access arrives with the pairing token, not before)"
    )]
    NonLoopbackBind { bind: SocketAddr },
    /// The committed studio-ui build output is missing or unreadable —
    /// regenerate it (see the crate docs).
    #[error("embedded UI asset problem: {0}")]
    Asset(String),
    /// The embedded FMIR did not parse or is not the version forma-server
    /// 0.1.4 renders (v2) — regenerate with a compatible @getforma/compiler.
    #[error("embedded FMIR rejected: {0}")]
    Ir(String),
    #[error("failed to start the studio tokio runtime")]
    Runtime(#[source] std::io::Error),
    #[error("failed to bind {bind}")]
    Bind {
        bind: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("studio server terminated unexpectedly")]
    Serve(#[source] std::io::Error),
}
