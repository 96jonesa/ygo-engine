//! Print a duel's trace, for comparison against ocgcore.
//!
//! ```text
//! cargo run --example trace -- [policy] [deck_size] [max_steps] [seed] [top]
//! ```
//!
//! `policy` is `pass`, `play` or `random:N` (the fuzzing policy, seeded
//! with `N`). `seed` is four comma-separated `u64`s.
//! `top` is a `CODE:N,CODE:N` list of translated cards replacing vanillas
//! at the **top** of each deck, laid down bottom-to-top so the last group
//! is drawn first — the same list `tools/differential.py --top` gives
//! ocgcore.
//!
//! ## Why the seed is an argument
//!
//! Under `DUEL_PSEUDO_SHUFFLE` nothing in a vanilla duel draws from the
//! generator, so the two engines agreeing says nothing about their seeds —
//! they are in fact **different** by default (this crate's
//! `[0x2545_F491_4F6C_DD1D, 1, 2, 3]` against the harness's `[1, 2, 3, 4]`).
//! The moment a coin is tossed that stops being harmless, and the
//! divergence would read as an engine bug rather than a configuration one.
//! So the harness names the seed explicitly on both sides.
//!
//! Kept as an example rather than a test because the differential harness
//! drives it as a subprocess: the port has no Python binding and is not
//! getting one, so a line-based trace on stdout is the seam.

use ygo_engine::driver::{Driver, PassPolicy, PlayPolicy, Policy, RandomPolicy, Stop};

fn main() {
    let mut args = std::env::args().skip(1);
    let policy = args.next().unwrap_or_else(|| "pass".into());
    let deck_size: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(40);
    let max_steps: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(500_000);
    let seed = args.next().map_or([1, 2, 3, 4], |s| {
        let mut it = s.split(',').filter_map(|v| v.trim().parse::<u64>().ok());
        [
            it.next().unwrap_or(1),
            it.next().unwrap_or(2),
            it.next().unwrap_or(3),
            it.next().unwrap_or(4),
        ]
    });

    let top = parse_top(&args.next().unwrap_or_default());

    let stop = match policy.as_str() {
        "pass" => run(PassPolicy, deck_size, max_steps, seed, &top),
        "play" => run(PlayPolicy, deck_size, max_steps, seed, &top),
        // `random:N` — the fuzzing policy, seeded with N; the harness
        // hands ocgcore's mirror the same N.
        other => match other
            .strip_prefix("random:")
            .and_then(|n| n.parse::<u64>().ok())
        {
            Some(n) => run(RandomPolicy::new(n), deck_size, max_steps, seed, &top),
            None => {
                eprintln!("unknown policy {other:?}: expected `pass`, `play` or `random:N`");
                std::process::exit(2);
            }
        },
    };
    // The stop reason is part of the comparison: two engines that play the
    // same duel should also end it the same way.
    match stop {
        Stop::Win { player, reason } => println!("evt stop win p{player} r{reason}"),
        Stop::Finished => println!("evt stop finished"),
        Stop::OutOfSteps => println!("evt stop outofsteps"),
        Stop::Stuck => println!("evt stop stuck"),
    }
}

/// `CODE:N,CODE:N` → `[(code, n), ...]`; the empty string is no cards.
fn parse_top(spec: &str) -> Vec<(u32, u32)> {
    spec.split(',')
        .filter(|s| !s.is_empty())
        .map(|item| {
            let (code, n) = item.split_once(':').unwrap_or((item, "1"));
            let parse = |v: &str| {
                v.trim().parse::<u32>().unwrap_or_else(|_| {
                    eprintln!("bad top spec {spec:?}: expected CODE:N,CODE:N");
                    std::process::exit(2);
                })
            };
            (parse(code), parse(n))
        })
        .collect()
}

/// Play one duel and print its trace, returning why it stopped.
fn run<P: Policy>(
    policy: P,
    deck_size: u32,
    max_steps: u32,
    seed: [u64; 4],
    top: &[(u32, u32)],
) -> Stop {
    let vanilla = Driver::<P>::vanilla_data();
    let placed: u32 = top.iter().map(|&(_, n)| n).sum();
    if placed > deck_size {
        eprintln!("top places {placed} cards in a deck of {deck_size}");
        std::process::exit(2);
    }
    // Decklist order: the last listed is the top, so the groups go last.
    let mut deck: Vec<_> = (0..deck_size - placed).map(|_| vanilla.clone()).collect();
    for &(code, n) in top {
        let data = ygo_engine::cards::card_data(code).unwrap_or_else(|| {
            eprintln!("no translated card has code {code}");
            std::process::exit(2);
        });
        deck.extend((0..n).map(|_| data.clone()));
    }
    let mut d = Driver::with_decks_and_seed(policy, [deck.clone(), deck], seed).tracing();
    let stop = d.run(max_steps);
    println!("{}", d.rendered_trace());
    stop
}
