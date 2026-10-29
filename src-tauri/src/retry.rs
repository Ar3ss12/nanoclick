//! Retry delays: the crate's deterministic spread, armed per caller.
//!
//! A refused admission that a caller retries on a FIXED delay wakes every
//! waiter at the same millisecond, which is the herd the backpressure advice
//! exists to avoid. [`snubber::jitter_ms`] moves the delay inside a band from
//! a caller-supplied salt, and the crate owns no entropy source on purpose:
//! the same salt always yields the same millisecond, so the spread is
//! testable without a timing window.
//!
//! The config-busy retry is NOT armed here: the page serializes saves through
//! one in-flight promise and retries once, so it is a herd of one and a spread
//! would only add latency.

/// Band around the base delay: ±20 %.
pub const SPREAD_PERCENT: u8 = 20;

/// Delay before one retry: `base_ms` moved inside ±`SPREAD_PERCENT` by `salt`.
///
/// Pass a salt that identifies the waiter (a label, a label plus an attempt
/// counter). Two waiters that share a salt retry in lockstep, which is the
/// behaviour this replaces.
pub fn retry_delay_ms(base_ms: u64, salt: u64) -> u64 {
    snubber::jitter_ms(base_ms, SPREAD_PERCENT, salt)
}

/// Stable salt for a named caller (FNV-1a over its name).
///
/// `hud` and `overlay` are toggled by one page action, so they must not hash
/// to the same offset.
pub fn retry_salt(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic spread: every named caller stays inside the band, the
    /// same salt never moves, and two labels never share an offset — the
    /// property that desynchronizes a hud+overlay retry.
    #[test]
    fn test_retry_delay_spreads_without_leaving_the_band() {
        let hud = retry_salt("hud");
        let overlay = retry_salt("overlay");
        assert_ne!(hud, overlay, "names must hash apart");

        for (name, salt) in [("hud", hud), ("overlay", overlay)] {
            let delay = retry_delay_ms(90, salt);
            assert!(
                (72..=108).contains(&delay),
                "{name} outside the ±20 % band around 90 ms: {delay}"
            );
            assert_eq!(
                delay,
                retry_delay_ms(90, salt),
                "{name}: the same salt must yield the same delay"
            );
        }
        assert_ne!(
            retry_delay_ms(90, hud),
            retry_delay_ms(90, overlay),
            "a hud+overlay retry must not wake on the same millisecond"
        );

        // The attempt counter separates repeated spawn failures of ONE label.
        let first = retry_delay_ms(500, retry_salt("hud") ^ 1);
        let second = retry_delay_ms(500, retry_salt("hud") ^ 2);
        assert!((400..=600).contains(&first) && (400..=600).contains(&second));
        assert_ne!(first, second, "consecutive attempts must spread");

        // No salt, no motion: a zero base (or a zero spread) is the advice.
        assert_eq!(retry_delay_ms(0, hud), 0);
    }
}
