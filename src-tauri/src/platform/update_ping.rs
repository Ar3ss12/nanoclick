//! Radar ping — tray-aware update telemetry via Cloudflare Worker.
//!
//! Why a backend ping and not the frontend checker: the page's 30-minute
//! check is `everyVisible`-gated (hidden page = no ping, deep sleep = no
//! page at all), so a tray-dwelling gamer would NEVER appear in the stats.
//! This module runs on a plain background thread owned by Tauri `setup()`:
//! first ping shortly after cold start, then one ping every 6 h for as long
//! as the process lives — window or no window.
//!
//! Privacy: the ping carries NO identifiers — only `?v=<semver>` for the
//! dashboard version split. Counting (requests / unique IPs / country) is
//! done by Cloudflare's edge, not by us. Failures (429 over-limit, DNS down,
//! Worker offline) are silent by design: the update flow itself always goes
//! through the plugin's direct GitHub endpoint, never through the Worker.

/// The Worker proxy in front of `latest.json`. Query shape:
/// `GET /check-update?v=1.2.0` → 200 + upstream manifest bytes.
pub const RADAR_PING_URL: &str = "https://nanoclick-update.aarik6131.workers.dev/check-update";

/// First ping delay after cold start (the app is still building windows).
pub const RADAR_FIRST_PING_DELAY_MS: u64 = 60_000;
/// Steady cadence afterwards. 4 pings/day/user → 100k free req/day ≈ 25k DAU.
pub const RADAR_PING_INTERVAL_MS: u64 = 6 * 60 * 60 * 1000;

/// Fire-and-forget ping through the platform HTTP fetcher (WinHTTP on
/// Windows). Silent on every failure — telemetry must never surface.
#[cfg(target_os = "windows")]
fn radar_ping_once() {
    let url = radar_ping_url(env!("CARGO_PKG_VERSION"));
    // Short and small: we only need the status, the body is discarded.
    // A 5 s local timeout is enforced by the fetcher itself.
    let _ = crate::platform::windows::uipi::winhttp_get(&url, &|_, _| {});
}

/// Tray-aware ping loop. Own thread (`nanoclick-radar`), first ping 60 s
/// after cold start, then every 6 h until the process dies. No window, no
/// hook, no scheduler touch — SHUTTING_DOWN is the only coupling.
pub fn start_radar_ping() {
    std::thread::Builder::new()
        .name("nanoclick-radar".into())
        .spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(RADAR_FIRST_PING_DELAY_MS));
            loop {
                #[cfg(target_os = "windows")]
                radar_ping_once();
                // Sleep in 1 s slices so a shutdown does not wait out 6 h.
                for _ in 0..(RADAR_PING_INTERVAL_MS / 1000) {
                    if crate::SHUTTING_DOWN.load(std::sync::atomic::Ordering::Acquire) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
        })
        .ok();
}

/// Pure: build the ping URL for a version. No network, unit-tested.
pub fn radar_ping_url(version: &str) -> String {
    format!("{RADAR_PING_URL}?v={version}")
}

/// Pure: should this HTTP outcome fall back to direct GitHub?
/// The Worker is analytics-only: 429 (over daily limit), 5xx, network
/// errors — all mean "check GitHub directly, bother nobody".
pub fn radar_needs_github_fallback(status: Option<u16>, transport_ok: bool) -> bool {
    if !transport_ok {
        return true;
    }
    match status {
        Some(200..=299) => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_url_carries_only_the_version() {
        let u = radar_ping_url("1.2.0");
        assert!(u.starts_with(RADAR_PING_URL));
        assert!(u.contains("v=1.2.0"));
        assert!(!u.contains("machine") && !u.contains("user") && !u.contains("id="));
    }

    #[test]
    fn fallback_covers_limit_and_outage() {
        assert!(!radar_needs_github_fallback(Some(200), true));
        assert!(radar_needs_github_fallback(Some(429), true));
        assert!(radar_needs_github_fallback(Some(500), true));
        assert!(radar_needs_github_fallback(None, false));
    }

    #[test]
    fn cadence_math_holds_the_free_tier() {
        // 4 pings/day/user against 100k req/day → 25k DAU headroom.
        let pings_per_day = 24 * 60 * 60 * 1000 / RADAR_PING_INTERVAL_MS + 1;
        assert!(pings_per_day <= 5, "radar must stay whisper-quiet");
    }
}
