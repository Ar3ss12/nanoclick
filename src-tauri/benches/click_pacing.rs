//! Click-pacing corridor (P2b): the per-click COMPUTE the click worker does
//! before it sleeps — `calc_single_timing` (Bates B3 jitter + fatigue
//! stretch + hesitation outlier + dynamic hold model), measured across the
//! CPS range with snubber's `ProfileHarness` (deterministic xorshift seed,
//! min/median/max/jitter/bursts).
//!
//! Run: `cargo bench --bench click_pacing`
//!
//! Why this exists: `click_run_node` carries an `ExecutionEnvelope` with a
//! 1-byte payload floor and wall bounds 0. Wall bounds stay 0 BY MEASUREMENT:
//! any ceiling tight enough to catch a wedged run would have to sit below a
//! legal 1-second sleep between clicks, while the per-click compute measured
//! here is microseconds. The corridor pins the numbers instead of a guess.

use nanoclick::calc_single_timing;
use rand::rngs::StdRng;
use rand::SeedableRng;
use snubber::ProfileHarness;

/// Worst-case humanization corridor: full jitter (±35%, the slider promise),
/// max hesitation probability (0.10) and low CPS (bio_factor = 1.0 keeps
/// every branch alive). High CPS fades the humanization out, so the low-CPS
/// corridor is the expensive one.
///
/// One sample = 1000 `calc_single_timing` calls: the per-click compute is
/// below the harness's 1 µs resolution (a single-call corridor measures 0
/// everywhere), so 1000 calls per sample give three digits of per-click
/// µs after the division printed below.
const CALLS_PER_SAMPLE: u64 = 1000;

fn corridor(name: &str, cps: u32, interval_ms: f64) {
    let harness = ProfileHarness::new(256, 16, 3);
    let mut rng = StdRng::seed_from_u64(0xC1C1_C1C1);
    let report = harness.run(0xC1C1, |_| {
        for _ in 0..CALLS_PER_SAMPLE {
            let _ = calc_single_timing(&mut rng, interval_ms, 35.0, 0.10);
        }
    });
    println!(
        "[{name}] cps={cps} interval_ms={interval_ms} x{CALLS_PER_SAMPLE} {}",
        report.summary()
    );
    println!(
        "[{name}] per click: min={}ns median={}ns max={}ns (us/{}*1000)",
        report.min_us * 1000 / CALLS_PER_SAMPLE,
        report.median_us * 1000 / CALLS_PER_SAMPLE,
        report.max_us * 1000 / CALLS_PER_SAMPLE,
        CALLS_PER_SAMPLE
    );
    let envelope = report.envelope(30);
    println!("[{name}] sample envelope(30% headroom): {envelope:?}");
}

fn main() {
    println!("click-pacing corridor: calc_single_timing (jitter=35%, outlier=0.10)");
    corridor("slow", 1, 1000.0);
    corridor("mid", 30, 1000.0 / 30.0);
    corridor("overdrive", 160, 1000.0 / 160.0);
}
