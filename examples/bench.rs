//! Time the port: whole duels under a policy, no trace kept.
//!
//! ```text
//! cargo run --release --example bench -- [--policy random|play|pass] [--games N]
//!     [--deck pool|vanilla] [--deck-size N] [--seed S] [--max-steps N]
//!     [--clone] [--no-log] [--game [--keys]] [--determinize]
//! ```
//!
//! Reports games, `process` steps and questions per second, and the
//! microseconds each step and each question cost, over `--games` duels.
//! `--deck pool` deals a seeded shuffle of [`ygo_engine::cards::POOL`] to
//! both players (vanilla filler past the pool's fifty), reshuffled per
//! game from `--seed + game`; `random` reseeds its policy the same way,
//! so every game is a different duel and the numbers are an average over
//! the pool's play, not one line of it.
//!
//! The measurement is the driver loop as the differential harness runs
//! it minus the trace, so it is the engine's own cost plus the policy's
//! (a few draws per question). `--clone` times `Field::clone` at a
//! mid-game point instead. `--game` plays through the game-tree API,
//! [`ygo_engine::game::Game`], choosing uniformly among the legal actions
//! and chance outcomes, and reports nodes per second as a solver would
//! see them — forced nodes skipped — with `--keys` adding the cost of an
//! information-set key at every player node.

use std::collections::BTreeMap;
use std::time::Instant;

use ygo_engine::cards::{card_data, POOL};
use ygo_engine::driver::{Driver, PassPolicy, PlayPolicy, Policy, RandomPolicy, Stop};

struct Args {
    policy: String,
    games: u64,
    deck: String,
    deck_size: u32,
    seed: u64,
    max_steps: u32,
    /// Time `Field::clone` instead of play: each game runs to a mid-game
    /// point and is then copied repeatedly.
    clone: bool,
    /// Drop the message log after every step, as a search would.
    no_log: bool,
    /// Play through `Game` instead of the driver.
    game: bool,
    /// With `--game`: compute the actor's information-set key at every
    /// player node.
    keys: bool,
    /// Time `Game::determinize` at a mid-game node instead of play.
    determinize: bool,
}

fn parse() -> Args {
    let mut a = Args {
        policy: "random".into(),
        games: 1000,
        deck: "pool".into(),
        deck_size: 40,
        seed: 1,
        max_steps: 500_000,
        clone: false,
        no_log: false,
        game: false,
        keys: false,
        determinize: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        if flag == "--clone" {
            a.clone = true;
            continue;
        }
        if flag == "--game" {
            a.game = true;
            continue;
        }
        if flag == "--keys" {
            a.keys = true;
            continue;
        }
        if flag == "--determinize" {
            a.determinize = true;
            continue;
        }
        if flag == "--no-log" {
            a.no_log = true;
            continue;
        }
        let value = it
            .next()
            .unwrap_or_else(|| usage(&format!("{flag} needs a value")));
        match flag.as_str() {
            "--policy" => a.policy = value,
            "--games" => a.games = value.parse().unwrap_or_else(|_| usage("--games N")),
            "--deck" => a.deck = value,
            "--deck-size" => {
                a.deck_size = value.parse().unwrap_or_else(|_| usage("--deck-size N"));
            }
            "--seed" => a.seed = value.parse().unwrap_or_else(|_| usage("--seed N")),
            "--max-steps" => {
                a.max_steps = value.parse().unwrap_or_else(|_| usage("--max-steps N"));
            }
            other => usage(&format!("unknown flag {other}")),
        }
    }
    if !matches!(a.policy.as_str(), "random" | "play" | "pass") {
        usage("--policy random|play|pass");
    }
    if !matches!(a.deck.as_str(), "pool" | "vanilla") {
        usage("--deck pool|vanilla");
    }
    a
}

fn usage(why: &str) -> ! {
    eprintln!("bench: {why}");
    eprintln!(
        "usage: bench [--policy random|play|pass] [--games N] [--deck pool|vanilla] \
         [--deck-size N] [--seed S] [--max-steps N] [--clone] [--no-log] [--game [--keys]] \
         [--determinize]"
    );
    std::process::exit(2)
}

/// One player's decklist for game `game`: the pool, shuffled by a
/// SplitMix64 seeded from `seed + game`, cut to `deck_size` with vanilla
/// filler underneath; or all vanilla.
fn deck(args: &Args, game: u64) -> Vec<ygo_engine::card::CardData> {
    let vanilla = Driver::<PassPolicy>::vanilla_data();
    if args.deck == "vanilla" {
        return (0..args.deck_size).map(|_| vanilla.clone()).collect();
    }
    let mut rng = RandomPolicy::new(args.seed.wrapping_add(game));
    let mut codes = POOL.to_vec();
    for i in (1..codes.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        codes.swap(i, j);
    }
    codes.truncate(args.deck_size as usize);
    let filler = args.deck_size as usize - codes.len();
    let mut out: Vec<_> = (0..filler).map(|_| vanilla.clone()).collect();
    out.extend(
        codes
            .iter()
            .map(|&c| card_data(c).expect("a pool card has data")),
    );
    out
}

struct Totals {
    steps: u64,
    questions: u64,
    messages: u64,
    stops: BTreeMap<String, u64>,
}

fn play<P: Policy>(policy: P, args: &Args, game: u64, t: &mut Totals) {
    let d = deck(args, game);
    let mut driver = Driver::with_decks_and_seed(policy, [d.clone(), d], [1, 2, 3, 4]);
    if args.no_log {
        driver = driver.discarding_messages();
    }
    let stop = driver.run(args.max_steps);
    t.steps += driver.steps;
    t.questions += driver.questions;
    t.messages += driver.field.messages.len() as u64;
    let key = match stop {
        Stop::Win { .. } => "win".to_string(),
        Stop::Finished => "finished".into(),
        Stop::OutOfSteps => "out of steps".into(),
        Stop::Stuck => "stuck".into(),
    };
    *t.stops.entry(key).or_default() += 1;
}

/// `--clone`: the cost of copying a mid-game duel, which is what a search
/// pays at every node. Each game is run to 8,000 steps (or its end), then
/// cloned 200 times; the mean is reported.
fn clone_bench(args: &Args) {
    let (mut clones, mut nanos, mut bare_nanos, mut cards, mut effects, mut messages) =
        (0u64, 0u128, 0u128, 0usize, 0usize, 0usize);
    for game in 0..args.games {
        let d = deck(args, game);
        let mut driver = Driver::with_decks_and_seed(
            RandomPolicy::new(args.seed.wrapping_add(game)),
            [d.clone(), d],
            [1, 2, 3, 4],
        );
        driver.run(8_000);
        cards += driver.field.cards.len();
        effects += driver.field.effects.len();
        messages += driver.field.messages.len();
        let start = Instant::now();
        for _ in 0..200 {
            let copy = driver.field.clone();
            std::hint::black_box(&copy);
        }
        nanos += start.elapsed().as_nanos();
        clones += 200;
        // The same state without its message log — what a search would copy.
        driver.field.messages.clear();
        let start = Instant::now();
        for _ in 0..200 {
            let copy = driver.field.clone();
            std::hint::black_box(&copy);
        }
        bare_nanos += start.elapsed().as_nanos();
    }
    let g = args.games as f64;
    println!(
        "clone: {:.2} us per Field::clone, {:.2} us without the message log ({} clones over {} games; mean state {:.0} cards, {:.0} effects, {:.0} messages)",
        nanos as f64 / clones as f64 / 1e3,
        bare_nanos as f64 / clones as f64 / 1e3,
        clones,
        args.games,
        cards as f64 / g,
        effects as f64 / g,
        messages as f64 / g
    );
}

/// `--game`: the game-tree API. Each game is played through `Game`,
/// choosing uniformly among the legal actions at a player node and the
/// outcomes at a chance node, forced nodes skipped, until it ends or
/// `--max-steps` processor steps have run. With `--keys` the actor's
/// information-set key is computed at every player node, which is what a
/// search pays per visit on top of the moves.
fn game_bench(args: &Args) {
    use ygo_engine::game::{Actor, Game};
    let (mut nodes, mut chance, mut applies, mut actions, mut steps) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut key_nanos = 0u128;
    let mut stops: BTreeMap<String, u64> = BTreeMap::new();
    let start = Instant::now();
    for game in 0..args.games {
        let d = deck(args, game);
        let mut g = Game::new([d.clone(), d], [1, 2, 3, 4]);
        let mut rng = RandomPolicy::new(args.seed.wrapping_add(game));
        loop {
            if g.steps >= u64::from(args.max_steps) {
                *stops.entry("out of steps".into()).or_default() += 1;
                break;
            }
            match g.player_to_act() {
                Actor::Terminal => {
                    let key = match g.result().map(|o| o.cause) {
                        Some(ygo_engine::game::Cause::Win { .. }) => "win",
                        Some(ygo_engine::game::Cause::Draw { .. }) => "draw",
                        Some(ygo_engine::game::Cause::Finished) => "finished",
                        _ => "runaway",
                    };
                    *stops.entry(key.into()).or_default() += 1;
                    break;
                }
                Actor::Chance => {
                    let outcomes = g.chance_outcomes();
                    let pick = outcomes[(rng.next_u64() % outcomes.len() as u64) as usize]
                        .0
                        .clone();
                    chance += 1;
                    applies += 1;
                    g.apply(&pick).expect("a listed outcome");
                }
                Actor::Player(p) => {
                    let legal = g.legal_actions();
                    if args.keys {
                        let t = Instant::now();
                        std::hint::black_box(g.infoset_key(p));
                        key_nanos += t.elapsed().as_nanos();
                    }
                    let a = legal[(rng.next_u64() % legal.len() as u64) as usize].clone();
                    nodes += 1;
                    actions += legal.len() as u64;
                    applies += 1;
                    g.apply(&a).expect("a listed action");
                }
            }
        }
        steps += g.steps;
        if args.games >= 10 && (game + 1) % (args.games / 10) == 0 {
            eprintln!(
                "[{}/{}] {:.1}s",
                game + 1,
                args.games,
                start.elapsed().as_secs_f64()
            );
        }
    }
    let secs = start.elapsed().as_secs_f64();
    let key_secs = key_nanos as f64 / 1e9;
    let play_secs = secs - key_secs;
    println!(
        "game: deck={} deck_size={} games={} seed={}{}",
        args.deck,
        args.deck_size,
        args.games,
        args.seed,
        if args.keys { " keys" } else { "" }
    );
    println!(
        "{:.2}s: {:.0} games/s, {:.0} player nodes/s, {:.0} applies/s (chance nodes included), {:.0} steps/s",
        secs,
        args.games as f64 / secs,
        nodes as f64 / play_secs,
        applies as f64 / play_secs,
        steps as f64 / play_secs
    );
    println!(
        "{:.2} us/apply, {:.1} legal actions per player node, {:.0} player nodes and {:.0} chance nodes per game",
        play_secs * 1e6 / applies as f64,
        actions as f64 / nodes as f64,
        nodes as f64 / args.games as f64,
        chance as f64 / args.games as f64
    );
    if args.keys {
        println!(
            "keys: {:.2} us per infoset_key ({} keys)",
            key_secs * 1e6 / nodes as f64,
            nodes
        );
    }
    let stops: Vec<String> = stops.iter().map(|(k, v)| format!("{k}={v}")).collect();
    println!("stops: {}", stops.join(" "));
}

/// `--determinize`: the cost of sampling a world, which is what a search
/// with beliefs pays per world. Each game is played through `Game` with
/// random moves to its fortieth player node (games ending sooner are
/// skipped), then the actor samples 200 worlds; the mean is reported,
/// with how many cards a sample gives a new identity.
fn determinize_bench(args: &Args) {
    use ygo_engine::game::{Actor, Game};
    let (mut samples, mut nanos, mut changed, mut hidden, mut positions) =
        (0u64, 0u128, 0u64, 0u64, 0u64);
    for game in 0..args.games {
        let d = deck(args, game);
        let mut g = Game::new([d.clone(), d], [1, 2, 3, 4]);
        let mut rng = RandomPolicy::new(args.seed.wrapping_add(game));
        let mut nodes = 0;
        let viewer = loop {
            match g.player_to_act() {
                Actor::Terminal => break None,
                Actor::Chance => g.sample_chance().expect("a chance node samples"),
                Actor::Player(p) => {
                    if nodes == 40 {
                        break Some(p);
                    }
                    nodes += 1;
                    let legal = g.legal_actions();
                    let a = legal[(rng.next_u64() % legal.len() as u64) as usize].clone();
                    g.apply(&a).expect("a listed action");
                }
            }
        };
        let Some(viewer) = viewer else { continue };
        positions += 1;
        let codes: Vec<u32> = g.field().cards.iter().map(|c| c.data.code).collect();
        hidden += g
            .field()
            .cards
            .iter()
            .enumerate()
            .filter(|(i, c)| c.current.controller != viewer && !g.knowledge().knows(viewer, *i))
            .count() as u64;
        let start = Instant::now();
        for k in 0..200u64 {
            let w = g.determinize(viewer, [1, 2, 3, k]);
            std::hint::black_box(&w);
            changed += w
                .field()
                .cards
                .iter()
                .zip(&codes)
                .filter(|(c, &code)| c.data.code != code)
                .count() as u64;
        }
        nanos += start.elapsed().as_nanos();
        samples += 200;
    }
    println!(
        "determinize: {:.2} us per Game::determinize ({} samples over {} positions; mean {:.1} hidden cards, {:.1} re-identified per sample)",
        nanos as f64 / samples as f64 / 1e3,
        samples,
        positions,
        hidden as f64 / positions as f64,
        changed as f64 / samples as f64
    );
}

fn main() {
    let args = parse();
    if args.clone {
        clone_bench(&args);
        return;
    }
    if args.determinize {
        determinize_bench(&args);
        return;
    }
    if args.game {
        game_bench(&args);
        return;
    }
    let mut t = Totals {
        steps: 0,
        questions: 0,
        messages: 0,
        stops: BTreeMap::new(),
    };
    let start = Instant::now();
    for game in 0..args.games {
        match args.policy.as_str() {
            "random" => play(
                RandomPolicy::new(args.seed.wrapping_add(game)),
                &args,
                game,
                &mut t,
            ),
            "play" => play(PlayPolicy, &args, game, &mut t),
            _ => play(PassPolicy, &args, game, &mut t),
        }
        if args.games >= 10 && (game + 1) % (args.games / 10) == 0 {
            eprintln!(
                "[{}/{}] {:.1}s",
                game + 1,
                args.games,
                start.elapsed().as_secs_f64()
            );
        }
    }
    let secs = start.elapsed().as_secs_f64();
    let games = args.games as f64;
    println!(
        "policy={} deck={} deck_size={} games={} seed={}{}",
        args.policy,
        args.deck,
        args.deck_size,
        args.games,
        args.seed,
        if args.no_log { " no-log" } else { "" }
    );
    println!(
        "{:.2}s: {:.0} games/s, {:.0} steps/s, {:.0} questions/s",
        secs,
        games / secs,
        t.steps as f64 / secs,
        t.questions as f64 / secs
    );
    println!(
        "{:.2} us/step, {:.2} us/question, {:.0} us/game",
        secs * 1e6 / t.steps as f64,
        secs * 1e6 / t.questions as f64,
        secs * 1e6 / games
    );
    println!(
        "per game: {:.0} steps, {:.0} questions, {:.0} messages",
        t.steps as f64 / games,
        t.questions as f64 / games,
        t.messages as f64 / games
    );
    let stops: Vec<String> = t.stops.iter().map(|(k, v)| format!("{k}={v}")).collect();
    println!("stops: {}", stops.join(" "));
}
