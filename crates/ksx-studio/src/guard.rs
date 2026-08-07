//! The browser boundary: what stops a page you are merely *visiting* from
//! driving your cabinet.
//!
//! # Why a loopback bind is not a security boundary
//!
//! Studio binds `127.0.0.1` and sends no CORS headers, and it is tempting to
//! read that as "only this machine, and only this page, can talk to it". That
//! is true of *reads* — a cross-origin `fetch` cannot see the response body
//! without CORS, so the poller's JSON stays private.
//!
//! It is not true of *writes*, and writes are the whole mutating surface.
//!
//! A cross-origin `<form method="post">` submission is a CORS **simple
//! request**: no preflight, no permission asked, and the browser sends it
//! happily. The attacking page never sees the response — and does not need to,
//! because the side effect has already happened. Every form route Studio serves
//! is reachable this way, from any tab, on a port that is not a secret:
//!
//! ```text
//! <form action="http://127.0.0.1:4460/map/preset/clear-all" method="post">
//! <script>document.forms[0].submit()</script>
//! ```
//!
//! That wipes a preset. `/map/session/stop` ends a session mid-game. Nothing in
//! the request distinguishes it from the real page's own submit.
//!
//! # The check, and why `Origin` is the right one
//!
//! A browser sets `Origin` on every request whose method is not GET or HEAD —
//! `fetch`, `sendBeacon`, and form submissions alike — and a page **cannot
//! suppress or forge it**. So for mutating methods:
//!
//! - `Origin` present and matching this bind → the real page. Allow.
//! - `Origin` present and NOT matching → a cross-site write. Refuse.
//! - `Origin` absent → not a browser-initiated cross-site request at all. Allow.
//!
//! That last arm is deliberate, not a gap. No browser mechanism produces a
//! cross-origin POST without `Origin`; a request without one came from `curl`,
//! a script, or the CLI — a local process already running as the user, which
//! could call the daemon's pipe directly and needs no help from Studio. Refusing
//! it would break automation to prevent nothing.
//!
//! # And `Host`, for the rebinding case
//!
//! DNS rebinding defeats a loopback bind without ever crossing the network:
//! `evil.com` resolves to `127.0.0.1` on its second lookup, so the browser
//! treats `http://evil.com:4460/api/map` as **same-origin with the attacker**
//! and hands over the response body. The `Origin` check above stops the writes;
//! this stops the reads, by requiring that the `Host` the client asked for is
//! actually a loopback name. A rebound request carries `Host: evil.com` and is
//! refused before it reaches a handler.

use std::net::SocketAddr;

use axum::extract::Request;
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Is the `Host` the client asked for a name **this server** answers to?
///
/// **Name only — the port is deliberately not checked here.** What rebinding
/// changes is the *name*: the attacker's page is served from `evil.com`, which
/// resolves to `127.0.0.1` on its second lookup, and the request arrives
/// carrying `Host: evil.com`. The port is whatever ksx is listening on either
/// way, so requiring it would reject honest clients (an HTTP/1.1 request may
/// omit `:80`, and plenty of tools do) while catching nothing extra.
///
/// Port strictness belongs on [`is_own_origin`] instead, where it is load
/// bearing: `http://127.0.0.1:4461` is a *different origin*, so a second local
/// web server must not be able to drive this one.
fn is_own_host(host: &str, bind: SocketAddr) -> bool {
    is_own_name(split_host_port(host).0, bind)
}

/// The same question about a name that has **already** been split off its port.
///
/// # Split once, ask once
///
/// Feeding an already-split name back through [`split_host_port`] is not
/// harmless: a bare IPv6 address has colons in it, so `::1` re-split becomes
/// `::` and stops being a loopback name. That was a real bug on the `Origin`
/// arm — `http://[::1]:4460` was refused as cross-site on a dual-stack machine,
/// which is exactly the "looks like a bug rather than a defence" failure the
/// three-spelling rule below exists to avoid.
///
/// # Two ways to be this server
///
/// A loopback name first: a user types `localhost`, follows a link to
/// `127.0.0.1`, and on a dual-stack machine may land on `[::1]`. All three are
/// this server and refusing any of them looks like a bug.
///
/// Then the literal address ksx is bound to. `docs/SURFACES.md` §7 warns that
/// "our own guard becomes the bug report" when LAN access arrives, and named
/// one check — there are two, and a change-set that fixes only the `Host`
/// allow-list ships a Studio that **renders on the phone and refuses every
/// button**, a 403 from the `Origin` arm on every form. So both learn the bound
/// address here, together, while the change is inert: the bind is loopback-only
/// today (`error.rs` returns `NonLoopbackBind` for anything else), so
/// `bind.ip()` is already a loopback address and nothing new is admitted. What
/// is removed is the trap.
///
/// This never widens the rebinding defence. [`is_bound_address`] compares
/// *parsed IP literals*: `evil.example` resolving to the bound address does not
/// parse as one, so a rebound request is refused exactly as before.
fn is_own_name(name: &str, bind: SocketAddr) -> bool {
    name.eq_ignore_ascii_case("localhost")
        || name == "127.0.0.1"
        || name == "::1"
        || is_bound_address(name, bind)
}

/// Is `name` the IP literal this server is bound to?
///
/// An IP **literal**, deliberately: a name is only ever this server if it is
/// spelled as the address, never because it resolves to it. That distinction is
/// the whole DNS-rebinding defence.
fn is_bound_address(name: &str, bind: SocketAddr) -> bool {
    name.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip == bind.ip())
}

/// Split `host:port`, tolerating the bracketed IPv6 literal form `[::1]:4460`.
fn split_host_port(host: &str) -> (&str, Option<u16>) {
    if let Some(rest) = host.strip_prefix('[') {
        // `[::1]:4460` — the colons inside the brackets are the address.
        return match rest.split_once(']') {
            Some((addr, tail)) => (addr, tail.strip_prefix(':').and_then(|p| p.parse().ok())),
            None => (host, None),
        };
    }
    match host.rsplit_once(':') {
        Some((name, port)) => (name, port.parse().ok()),
        None => (host, None),
    }
}

/// Is this `Origin` value this server?
fn is_own_origin(origin: &str, bind: SocketAddr) -> bool {
    // `Origin: null` is what a sandboxed iframe or a `file://` page sends. It is
    // not this server, and treating it as absent would hand exactly those
    // contexts a bypass.
    let Some(rest) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    // An origin has no path, but be strict rather than trusting that.
    let authority = rest.split('/').next().unwrap_or(rest);
    let (name, port) = split_host_port(authority);
    // Here the port IS the point: another server on another local port is a
    // different origin, and must not be able to post to this one.
    port == Some(bind.port()) && is_own_name(name, bind)
}

/// Refuse cross-site writes and rebound reads, before any handler runs.
///
/// Applied as one layer over the whole router rather than per route, because
/// the failure mode being prevented is *forgetting a route* — and the mapper
/// alone grew eight form endpoints in three milestones.
pub async fn same_origin(bind: SocketAddr, request: Request, next: Next) -> Response {
    let headers = request.headers();

    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !is_own_host(host, bind) {
        tracing::warn!(
            %host,
            "refused: Host is not a name this bind answers to (DNS rebinding looks exactly \
             like this)"
        );
        return (
            StatusCode::MISDIRECTED_REQUEST,
            "ksx Studio answers to localhost only. Open it as http://127.0.0.1 — a request \
             arriving under another host name is a DNS-rebinding attempt, not a configuration \
             problem.\n",
        )
            .into_response();
    }

    // GET/HEAD change nothing, and every mutating route is a POST.
    let mutating = !matches!(request.method(), &Method::GET | &Method::HEAD);
    if mutating {
        if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
            if !is_own_origin(origin, bind) {
                tracing::warn!(
                    %origin,
                    path = %request.uri().path(),
                    "refused a cross-site write — a page you are visiting tried to drive ksx"
                );
                return (
                    StatusCode::FORBIDDEN,
                    "ksx Studio refused a request from another site.\n\nThis endpoint changes \
                     your cabinet's configuration, so it only accepts requests from ksx \
                     Studio's own pages. If you meant to do this, use the Studio tab itself \
                     or the `ksx` command line.\n",
                )
                    .into_response();
            }
        }
        // No `Origin` at all: not a browser cross-site request. See module docs.
    }

    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bind() -> SocketAddr {
        "127.0.0.1:4460".parse().unwrap()
    }

    #[test]
    fn the_three_loopback_spellings_are_all_this_server() {
        for host in ["127.0.0.1:4460", "localhost:4460", "LocalHost:4460"] {
            assert!(is_own_host(host, bind()), "{host} must be allowed");
        }
        assert!(is_own_host("[::1]:4460", bind()), "IPv6 literal");
    }

    #[test]
    fn a_rebound_host_is_refused_even_though_it_resolves_to_loopback() {
        // This is the whole DNS-rebinding trick: the packet genuinely arrives on
        // 127.0.0.1, so the bind cannot tell. Only the name it asked for can.
        assert!(!is_own_host("evil.example:4460", bind()));
        assert!(!is_own_host("ksx.attacker.test:4460", bind()));
        assert!(
            !is_own_host("127.0.0.1.evil.example:4460", bind()),
            "a name that merely begins with a loopback address is not one"
        );
    }

    #[test]
    fn a_host_without_a_port_is_still_judged_by_its_name() {
        // HTTP/1.1 lets a client omit the port, and tools do. The name is what
        // rebinding changes, so the name is what is checked.
        assert!(is_own_host("127.0.0.1", bind()));
        assert!(!is_own_host("evil.example", bind()));
    }

    #[test]
    fn our_own_pages_origin_is_recognised() {
        assert!(is_own_origin("http://127.0.0.1:4460", bind()));
        assert!(is_own_origin("http://localhost:4460", bind()));
        // The third loopback spelling, which the `Host` arm has always accepted
        // and this arm did NOT: an already-split bare IPv6 name was being fed
        // back through the port splitter, so `::1` became `::` and our own page
        // on a dual-stack machine 403'd every form it submitted.
        assert!(
            is_own_origin("http://[::1]:4460", "[::1]:4460".parse().unwrap()),
            "all three loopback spellings must agree across both checks"
        );
    }

    #[test]
    fn another_sites_origin_is_not() {
        assert!(!is_own_origin("https://evil.example", bind()));
        assert!(
            !is_own_origin("http://127.0.0.1:4461", bind()),
            "a different port is a different origin"
        );
        assert!(
            !is_own_origin("http://127.0.0.1.evil.example:4460", bind()),
            "a suffix that merely starts with our address is not our address"
        );
    }

    /// **The trap `docs/SURFACES.md` §7 half-described, as a test.**
    ///
    /// The doc named one check a LAN bind must clear (the `Host` allow-list)
    /// and there are two. A change-set that fixes only that one produces a
    /// Studio that renders on the phone and 403s every form, because
    /// `is_own_origin` ended in *"and the host is a loopback name"* — so a page
    /// served from `http://192.168.1.47:4460` was not its own origin.
    ///
    /// Breaks against that version: both assertions below were false when the
    /// only accepted names were `localhost`, `127.0.0.1` and `::1`.
    #[test]
    fn the_address_ksx_is_bound_to_is_a_name_for_ksx_on_both_checks() {
        let lan: SocketAddr = "192.168.1.47:4460".parse().unwrap();
        assert!(
            is_own_host("192.168.1.47:4460", lan),
            "a request to the address we are serving on is not a rebinding attempt"
        );
        assert!(
            is_own_origin("http://192.168.1.47:4460", lan),
            "and our own page, served from that address, is our own origin — \
             this is the one that turns the guard into the bug report"
        );
        // IPv6, spelled the way a browser sends it.
        let v6: SocketAddr = "[fd00::1]:4460".parse().unwrap();
        assert!(is_own_host("[fd00::1]:4460", v6));
        assert!(is_own_origin("http://[fd00::1]:4460", v6));
    }

    /// The half that must NOT move when the other half does.
    ///
    /// Learning the bound address may not become "trust any address". The
    /// comparison is against a parsed IP literal and the bind we are actually
    /// on, so a rebound name still fails and a LAN page still cannot drive a
    /// loopback Studio.
    #[test]
    fn learning_the_bound_address_admits_nothing_else() {
        let lan: SocketAddr = "192.168.1.47:4460".parse().unwrap();
        let loopback = bind();

        assert!(
            !is_own_origin("http://192.168.1.47:4460", loopback),
            "a page on the LAN address is not the origin of a loopback bind"
        );
        assert!(!is_own_host("192.168.1.47:4460", loopback));
        assert!(
            !is_own_host("evil.example:4460", lan),
            "a NAME that resolves to the bound address is still rebinding — \
             only the literal counts"
        );
        assert!(
            !is_own_host("192.168.1.48:4460", lan),
            "the neighbour's address is not ours"
        );
        assert!(
            !is_own_origin("http://192.168.1.47:4461", lan),
            "and the port still decides: another server on this host is another \
             origin"
        );
    }

    #[test]
    fn null_origin_is_not_absent_origin() {
        // A sandboxed iframe posts with `Origin: null`. Absent means "no
        // browser"; null means "a browser that will not say which page" — the
        // opposite of trustworthy.
        assert!(!is_own_origin("null", bind()));
    }
}
