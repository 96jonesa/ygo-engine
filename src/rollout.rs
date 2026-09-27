//! The playout: a game played out on a copy, for a search to evaluate.
//!
//! A search — PIMC, IS-MCTS, anything that expands a tree a node at a
//! time — needs a value for the nodes at its frontier, and the classical
//! estimate is a playout: play the position out by a cheap policy and take
//! the result. This module is that playout, and only that: uniform random
//! moves, chance sampled, to the end of the game or a cap on the moves.
//! What a search does with a playout cut short at the cap — score the
//! position with a heuristic, or play on — is the search's choice, not the
//! engine's, so the cut-off game is handed back rather than judged.
//!
//! [`Game::rollout`] plays a *copy*, so the game it is called on is
//! untouched. The copy's generator is reseeded from the playout's seed
//! before the first move, and so is the move policy, so two playouts from
//! one state with different seeds see different coins and random
//! selections as well as different moves, and the same seed gives the same
//! playout, which is what makes a search reproducible given its seed. The
//! order of the decks is not reseeded, and should not be: a shuffle rolls
//! it and a look at the top reveals it (`docs/processor-loop.md`, "Solver
//! mode"), so the order is part of the state. A playout from a world
//! sampled by [`Game::determinize`] follows that world's order, as it
//! should — the sample is one definite completion, and its draws are part
//! of it.

use crate::game::{Actor, Game, GameError, Outcome};
use crate::rng::Xoshiro256StarStar;

/// A finished playout: the game as it was left, how it ended if it did,
/// and the player moves made.
#[derive(Clone)]
pub struct Playout {
    /// The copy, at the end of the game or at the cap.
    pub game: Game,
    /// The game's result, or `None` when the cap stopped it first.
    pub outcome: Option<Outcome>,
    /// Player moves made (chance nodes are not counted).
    pub moves: u32,
}

/// The playout itself, on `g` in place: random moves from `rng`, chance
/// sampled from the game's own generator, until the end or `max_moves`
/// player moves. Returns the result, if the game ended, and the moves
/// made.
pub(crate) fn play_out(
    g: &mut Game,
    rng: &mut Xoshiro256StarStar,
    max_moves: u32,
) -> (Option<Outcome>, u32) {
    let mut moves = 0;
    loop {
        match g.player_to_act() {
            Actor::Terminal => {
                return (
                    Some(g.result().expect("a terminal game has a result")),
                    moves,
                );
            }
            Actor::Chance => match g.sample_chance() {
                // A runaway leaves the game terminal; the next turn of the
                // loop returns its result.
                Ok(()) | Err(GameError::Runaway) => {}
                Err(e) => panic!("sampling a chance node: {e:?}"),
            },
            Actor::Player(_) => {
                if moves >= max_moves {
                    return (None, moves);
                }
                let legal = g.legal_actions();
                let a = legal[(rng.next() % legal.len() as u64) as usize].clone();
                match g.apply(&a) {
                    Ok(()) | Err(GameError::Runaway) => {}
                    Err(e) => panic!("applying a listed action {a:?}: {e:?}"),
                }
                moves += 1;
            }
        }
    }
}

impl Game {
    /// A random playout from here on a copy, to the end or to `max_moves`
    /// player moves — see the module note. The copy's generator (coins,
    /// random selections; the deck order is the state's) and the move
    /// policy are seeded from `seed`; `self` is untouched.
    pub fn rollout(&self, max_moves: u32, seed: u64) -> Playout {
        let mut game = self.clone();
        let mut rng = Xoshiro256StarStar::from_u64(seed);
        game.field_mut().rng =
            Xoshiro256StarStar::new([rng.next(), rng.next(), rng.next(), rng.next()]);
        let (outcome, moves) = play_out(&mut game, &mut rng, max_moves);
        Playout {
            game,
            outcome,
            moves,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::CardData;
    use crate::cards::{card_data, POOL};
    use crate::driver::RandomPolicy;

    fn pool_deck(game: u64) -> Vec<CardData> {
        let mut rng = RandomPolicy::new(7_000 + game);
        let mut codes = POOL.to_vec();
        for i in (1..codes.len()).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            codes.swap(i, j);
        }
        codes.truncate(40);
        codes
            .iter()
            .map(|&c| card_data(c).expect("pool data"))
            .collect()
    }

    /// Random play to the `n`-th player node, chance sampled.
    fn at_node(g: &mut Game, rng: &mut RandomPolicy, n: usize) -> bool {
        let mut seen = 0;
        for _ in 0..20_000 {
            match g.player_to_act() {
                Actor::Terminal => return false,
                Actor::Chance => g.sample_chance().unwrap(),
                Actor::Player(_) => {
                    seen += 1;
                    if seen >= n {
                        return true;
                    }
                    let l = g.legal_actions();
                    let a = l[rng.below(l.len())].clone();
                    g.apply(&a).unwrap();
                }
            }
        }
        false
    }

    fn mid_game(game: u64, depth: usize) -> Game {
        let deck = pool_deck(game);
        let mut g = Game::new([deck.clone(), deck], [1, 2, 3, game]);
        let mut rng = RandomPolicy::new(500 + game);
        assert!(at_node(&mut g, &mut rng, depth));
        g
    }

    /// What a playout reached, as something comparable.
    fn fingerprint(p: &mut Playout) -> (Option<[u64; 2]>, u32, u64) {
        let outcome = p.outcome.map(|o| o.returns.map(f64::to_bits));
        (outcome, p.moves, p.game.public_key())
    }

    /// **A playout is a function of its seed, and not constant.** From one
    /// mid-game node, the same seed twice reaches the same place; eight
    /// seeds do not all agree.
    #[test]
    fn playouts_are_seeded_and_vary() {
        let g = mid_game(1, 30);
        let reached: Vec<_> = (0..8).map(|s| fingerprint(&mut g.rollout(40, s))).collect();
        for (s, r) in reached.iter().enumerate() {
            assert_eq!(
                *r,
                fingerprint(&mut g.rollout(40, s as u64)),
                "seed {s} again"
            );
        }
        assert!(reached.iter().any(|r| *r != reached[0]), "{reached:?}");
    }

    /// **A playout leaves the state as it was.** Keys, the legal actions
    /// and the generator are those before.
    #[test]
    fn a_playout_is_pure() {
        let mut g = mid_game(2, 25);
        let key = g.infoset_key(0);
        let public = g.public_key();
        let legal = g.legal_actions();
        let rng = g.field().rng.clone();
        for s in 0..4 {
            g.rollout(60, s);
        }
        assert_eq!(g.infoset_key(0), key);
        assert_eq!(g.public_key(), public);
        assert_eq!(g.legal_actions(), legal);
        assert_eq!(g.field().rng, rng);
    }

    /// **The cap counts player moves, and a capped playout hands back the
    /// cut-off game.** No moves allowed from a player node returns that
    /// node untouched; five allowed makes five and stops at a player node.
    #[test]
    fn the_cap_counts_player_moves() {
        let mut g = mid_game(3, 20);
        let mut none = g.rollout(0, 9);
        assert_eq!((none.outcome, none.moves), (None, 0));
        assert_eq!(none.game.public_key(), g.public_key());
        let five = g.rollout(5, 9);
        assert_eq!(five.moves, 5);
        assert!(five.outcome.is_none());
        assert!(matches!(five.game.player_to_act(), Actor::Player(_)));
    }

    /// **A finished game returns its result.** Played to the end, a
    /// playout from the terminal is the game's outcome whatever the seed
    /// and cap; and an uncapped playout from mid-game always ends.
    #[test]
    fn a_finished_game_returns_its_result() {
        let mut g = mid_game(4, 10);
        let mut rng = Xoshiro256StarStar::from_u64(1);
        let (outcome, _) = play_out(&mut g, &mut rng, u32::MAX);
        assert!(g.is_terminal());
        assert_eq!(outcome, g.result());
        assert_eq!(g.rollout(0, 5).outcome, outcome);
        assert_eq!(g.rollout(100, 6).outcome, outcome);
        let p = mid_game(5, 10).rollout(u32::MAX, 3);
        let returns = p.outcome.expect("an uncapped playout ends").returns;
        assert!(
            [[1.0, -1.0], [-1.0, 1.0], [0.0, 0.0]].contains(&returns),
            "{returns:?}"
        );
    }

    /// **A playout follows the world's deck order.** From the root chance
    /// node — the first player's opening draw — every seed draws the same
    /// card, because the order is the state's; a different world (the
    /// seam's, for the other player, whose sample permutes this player's
    /// deck) draws differently.
    #[test]
    fn a_playout_follows_the_worlds_deck_order() {
        let deck = pool_deck(6);
        let g = Game::new([deck.clone(), deck], [1, 2, 3, 4]);
        assert_eq!(g.player_to_act(), Actor::Chance);
        let after_draw = |g: &Game, seed: u64| g.rollout(0, seed).game.infoset_key(0);
        let first = after_draw(&g, 0);
        assert!((1..12u64).all(|s| after_draw(&g, s) == first));
        let worlds: std::collections::BTreeSet<u64> = (0..8u64)
            .map(|s| after_draw(&g.determinize(1, [5, 6, 7, s]), 0))
            .collect();
        assert!(worlds.len() > 1, "every world drew the same card");
    }
}
