//! The game-tree canary at scale: random games through `Game`, checking
//! the two invariants a solver relies on.
//!
//! ```text
//! cargo run --release --example canary -- [--games N] [--seed S]
//!     [--deck pool|vanilla] [--spec CODE:N,CODE:N,...] [--deck-size N]
//!     [--max-applies N] [--probe N] [--threads N]
//! ```
//!
//! Per node: the engine never refuses a listed action (`Rejected` is a
//! defect in the layer's encoding), a player node is never empty and,
//! with the default config, never forced, and a chance node's
//! probabilities sum to one. Across every game: **equal keys offer equal
//! actions** — every repeat of an actor's information-set key must offer
//! the same legal actions, else the observation left out a state fact
//! (conflating two positions) or the action list depends on something
//! the viewer cannot see. Either is a bug in `observation.rs` or
//! `game.rs`, not in this canary, and the first is printed with the
//! question and both lists.
//!
//! Repeats of a key are rare — a key spans the whole hand and board, so
//! only the first nodes of games with the same deal coincide — so the
//! canary also probes the key directly: at every `--probe`-th player
//! node, a clone has the opponent's deck reversed, the generator
//! advanced, and the identity of one of their face-down cards exchanged
//! with a hand or deck card of theirs where the actor knows neither, and
//! the actor's key and legal actions must not move. That is the
//! invariance the key is built on, checked without waiting for a repeat.
//!
//! `--deck pool` deals a seeded shuffle of the pool to both players, as
//! `bench` does; `--spec` deals the given multiset of codes to both
//! (Extra Deck cards included), the Goat mirror being the use. `--threads`
//! shards the games across threads (`Field` is `Send`), each with its own
//! key map, so a repeat is only found within a shard; the probes, which
//! carry the canary, are per node and unaffected. 100,000 pool games are
//! ~30 M nodes and about a gigabyte of map in one shard.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use ygo_engine::board::position;
use ygo_engine::card::CardData;
use ygo_engine::cards::{card_data, POOL};
use ygo_engine::driver::{Driver, PassPolicy, RandomPolicy};
use ygo_engine::fxhash::FxHasher;
use ygo_engine::game::{Action, Actor, Game, GameError};

struct Args {
    games: u64,
    seed: u64,
    deck: String,
    spec: String,
    deck_size: u32,
    max_applies: u32,
    /// Probe the key's invariance at every N-th player node (0 = never).
    probe: u64,
    /// Shard the games across this many threads.
    threads: u64,
}

fn usage(why: &str) -> ! {
    eprintln!("canary: {why}");
    eprintln!(
        "usage: canary [--games N] [--seed S] [--deck pool|vanilla] [--spec CODE:N,...] \
         [--deck-size N] [--max-applies N] [--probe N] [--threads N]"
    );
    std::process::exit(2)
}

fn parse() -> Args {
    let mut a = Args {
        games: 1000,
        seed: 1,
        deck: "pool".into(),
        spec: String::new(),
        deck_size: 40,
        max_applies: 20_000,
        probe: 10,
        threads: 1,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it
            .next()
            .unwrap_or_else(|| usage(&format!("{flag} needs a value")));
        match flag.as_str() {
            "--games" => a.games = value.parse().unwrap_or_else(|_| usage("--games N")),
            "--seed" => a.seed = value.parse().unwrap_or_else(|_| usage("--seed N")),
            "--deck" => a.deck = value,
            "--spec" => a.spec = value,
            "--deck-size" => {
                a.deck_size = value.parse().unwrap_or_else(|_| usage("--deck-size N"));
            }
            "--max-applies" => {
                a.max_applies = value.parse().unwrap_or_else(|_| usage("--max-applies N"));
            }
            "--probe" => a.probe = value.parse().unwrap_or_else(|_| usage("--probe N")),
            "--threads" => a.threads = value.parse().unwrap_or_else(|_| usage("--threads N")),
            _ => usage(&format!("unknown flag {flag}")),
        }
    }
    if !matches!(a.deck.as_str(), "pool" | "vanilla") {
        usage("--deck pool|vanilla");
    }
    a
}

/// A deck for game `game`: the `--spec` multiset if given, else a seeded
/// shuffle of the pool (vanilla filler underneath), else all vanilla.
fn deck(args: &Args, game: u64) -> Vec<CardData> {
    let vanilla = Driver::<PassPolicy>::vanilla_data();
    if !args.spec.is_empty() {
        let mut out = Vec::new();
        for entry in args.spec.split(',') {
            let (code, n) = entry
                .split_once(':')
                .unwrap_or_else(|| usage("--spec CODE:N,CODE:N,..."));
            let code: u32 = code.parse().unwrap_or_else(|_| usage("--spec: a code"));
            let n: usize = n.parse().unwrap_or_else(|_| usage("--spec: a count"));
            let data = card_data(code)
                .unwrap_or_else(|| usage(&format!("--spec: {code} is not in the pool")));
            out.extend(std::iter::repeat_n(data, n));
        }
        return out;
    }
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

/// The legal actions, hashed: what the map holds per key.
fn fingerprint(actions: &[Action]) -> u64 {
    let mut h = FxHasher::default();
    actions.hash(&mut h);
    h.finish()
}

struct Tally {
    games: u64,
    ended: u64,
    player_nodes: u64,
    chance_nodes: u64,
    keys: u64,
    repeats: u64,
    probes: u64,
    swaps: u64,
    violations: u64,
}

impl Tally {
    fn new() -> Self {
        Self {
            games: 0,
            ended: 0,
            player_nodes: 0,
            chance_nodes: 0,
            keys: 0,
            repeats: 0,
            probes: 0,
            swaps: 0,
            violations: 0,
        }
    }

    fn add(&mut self, o: &Tally) {
        self.games += o.games;
        self.ended += o.ended;
        self.player_nodes += o.player_nodes;
        self.chance_nodes += o.chance_nodes;
        self.keys += o.keys;
        self.repeats += o.repeats;
        self.probes += o.probes;
        self.swaps += o.swaps;
        self.violations += o.violations;
    }
}

/// Play games `first..last` and check them; `report` is whether this shard
/// prints progress.
fn shard(args: &Args, first: u64, last: u64, report: bool, start: Instant) -> Tally {
    let mut seen: HashMap<u64, u64> = HashMap::new();
    let mut t = Tally::new();
    for game in first..last {
        let d = deck(args, game);
        let mut g = Game::new([d.clone(), d], [1, 2, 3, 4 + game]);
        let mut rng = RandomPolicy::new(args.seed.wrapping_add(1_000_003 * game));
        t.games += 1;
        for _ in 0..args.max_applies {
            match g.player_to_act() {
                Actor::Terminal => {
                    t.ended += 1;
                    break;
                }
                Actor::Chance => {
                    let outcomes = g.chance_outcomes();
                    let total: f64 = outcomes.iter().map(|(_, p)| p).sum();
                    if outcomes.len() < 2 || (total - 1.0).abs() > 1e-9 {
                        t.violations += 1;
                        eprintln!(
                            "game {game}: chance node with {} outcomes summing to {total}: {:?}",
                            outcomes.len(),
                            g.question()
                        );
                    }
                    let pick = outcomes[rng.below(outcomes.len())].0.clone();
                    t.chance_nodes += 1;
                    if let Err(e) = g.apply(&pick) {
                        t.violations += 1;
                        eprintln!("game {game}: outcome {pick:?} refused: {e:?}");
                        break;
                    }
                }
                Actor::Player(p) => {
                    let actions = g.legal_actions();
                    if actions.len() < 2 {
                        t.violations += 1;
                        eprintln!(
                            "game {game}: player node with {} actions: {:?}",
                            actions.len(),
                            g.question()
                        );
                        if actions.is_empty() {
                            break;
                        }
                    }
                    let key = g.infoset_key(p);
                    let fp = fingerprint(&actions);
                    t.player_nodes += 1;
                    if args.probe > 0 && t.player_nodes.is_multiple_of(args.probe) {
                        // The hidden state reordered: the other deck reversed,
                        // the generator advanced. Neither is visible.
                        let mut c = g.clone();
                        let other = usize::from(1 - p);
                        let mut order = c.field().players[other].main.clone();
                        order.reverse();
                        c.field_mut().set_deck_order(1 - p, &order);
                        let _ = c.field_mut().rng.next_integer(0, 1000);
                        // And the identity of one of their face-down cards
                        // exchanged with one of their hand or deck cards,
                        // where the actor knows neither: what a card *is*
                        // is hidden; what its seat has been through (set
                        // this turn, attacked) is not, and stays put.
                        let hidden_seat = {
                            let f = c.field();
                            f.players[other]
                                .mzone
                                .iter()
                                .chain(f.players[other].szone.iter())
                                .flatten()
                                .copied()
                                .find(|&x| {
                                    f.cards[x].current.position & position::FACEDOWN != 0
                                        && !c.knowledge().knows(p, x)
                                })
                        };
                        let hidden_partner = {
                            let f = c.field();
                            f.players[other]
                                .hand
                                .iter()
                                .chain(f.players[other].main.iter())
                                .copied()
                                .find(|&y| !c.knowledge().knows(p, y))
                        };
                        if let (Some(x), Some(y)) = (hidden_seat, hidden_partner) {
                            let f = c.field_mut();
                            let (dx, cx) = (f.cards[x].data.clone(), f.cards[x].code);
                            let (dy, cy) = (f.cards[y].data.clone(), f.cards[y].code);
                            f.cards[x].data = dy;
                            f.cards[x].code = cy;
                            f.cards[y].data = dx;
                            f.cards[y].code = cx;
                            t.swaps += 1;
                        }
                        t.probes += 1;
                        if c.infoset_key(p) != key || c.legal_actions() != actions {
                            t.violations += 1;
                            eprintln!(
                                "game {game}: the key or the actions moved with the hidden state at {:?}",
                                g.question()
                            );
                        }
                    }
                    match seen.get(&key) {
                        Some(&prev) => {
                            t.repeats += 1;
                            if prev != fp {
                                t.violations += 1;
                                eprintln!(
                                    "game {game}: key {key:#x} seen with different actions; now {actions:?} at {:?}",
                                    g.question()
                                );
                            }
                        }
                        None => {
                            seen.insert(key, fp);
                            t.keys += 1;
                        }
                    }
                    let a = actions[rng.below(actions.len())].clone();
                    match g.apply(&a) {
                        Ok(()) => {}
                        Err(GameError::Rejected(a)) => {
                            t.violations += 1;
                            eprintln!(
                                "game {game}: the engine refused {a:?} at {:?}",
                                g.question()
                            );
                            break;
                        }
                        Err(e) => {
                            t.violations += 1;
                            eprintln!("game {game}: {a:?}: {e:?}");
                            break;
                        }
                    }
                }
            }
        }
        let span = last - first;
        if report && span >= 10 && (game + 1 - first).is_multiple_of(span / 10) {
            eprintln!(
                "[{}/{} in this shard] {:.0}s nodes={} keys={} repeats={} violations={}",
                game + 1 - first,
                span,
                start.elapsed().as_secs_f64(),
                t.player_nodes,
                t.keys,
                t.repeats,
                t.violations
            );
        }
    }
    t
}

fn main() {
    let args = parse();
    let start = Instant::now();
    let threads = args.threads.max(1).min(args.games.max(1));
    let per = args.games.div_ceil(threads);
    let t = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                let args = &args;
                let first = i * per;
                let last = ((i + 1) * per).min(args.games);
                scope.spawn(move || shard(args, first, last, i == 0, start))
            })
            .collect();
        let mut total = Tally::new();
        for h in handles {
            total.add(&h.join().expect("a shard finished"));
        }
        total
    });
    let secs = start.elapsed().as_secs_f64();
    println!(
        "canary: deck={}{} games={} seed={} threads={}",
        args.deck,
        if args.spec.is_empty() {
            String::new()
        } else {
            " (spec)".into()
        },
        args.games,
        args.seed,
        threads
    );
    println!(
        "{:.1}s: {} games ({} ended), {} player nodes, {} chance nodes, {} distinct keys, {} repeats, {} probes ({} with a swap)",
        secs, t.games, t.ended, t.player_nodes, t.chance_nodes, t.keys, t.repeats, t.probes, t.swaps
    );
    println!("violations: {}", t.violations);
    if t.violations > 0 {
        std::process::exit(1);
    }
}
