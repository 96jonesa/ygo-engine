//! The determinization seam: a world the viewer cannot tell from this one.
//!
//! A search solver — PIMC, IS-MCTS — does not know the hidden state, so it
//! samples worlds consistent with what the player knows and searches each
//! as if it were the truth. This module is where a world comes from.
//!
//! ## The belief model, version one
//!
//! Uniform over consistent completions. What the viewer does not know, in the port's
//! own terms (`observation::Knowledge`), is the identity of the other
//! player's hidden cards not marked known to the viewer — hand, deck,
//! face-down on the field — and the order of both decks. A sampled world
//! permutes those identities uniformly among those cards, reshuffles the
//! viewer's own deck, and reseeds the generator. Everything the viewer can
//! see is untouched, so the viewer's observation, key and legal actions
//! are the same in every sample — the tests pin that — and the other
//! player's cards keep their multiset, since a permutation moves nothing
//! in or out. Possibility sets ("the set card is one of these three",
//! from the ambiguous-departure work) would narrow the permutation later;
//! the interface leaves room.
//!
//! Two legality constraints on the permutation, because a world the engine
//! could not have reached is not a completion: a face-down card in the
//! monster zone is a monster, by preference one that could have been set
//! without tributes (level four or less; a level-five-or-more monster is
//! placed there only when nothing smaller is left in the pool, which is
//! the tribute-set world and rare), and a set spell or trap is a spell or
//! trap. Hands and decks take anything left. A face-down card the viewer
//! knows — one they watched turn over — is not in the permutation at all.
//!
//! ## Identity, not position
//!
//! The permutation moves *identities* between cards, never cards between
//! places. What a card is — its printed data and its script's effects — is
//! hidden; what its seat has been through — its place, its status word,
//! the turn it arrived, the effects granted to it by others, its counters —
//! is public and stays put. That is the lesson the canary's probe taught
//! (`docs/processor-loop.md`, "Ambiguous departures"): the two live in one
//! struct and only one of them may move. [`Field::reidentify`] is the
//! operation: strip the card's own printed effects, swap the data, run the
//! new script as at the deal. Effect ids come out fresh, which no
//! observation can see.

use crate::card::{card_type, CardData};
use crate::effect::flag;
use crate::event::{CardId, EffectId};
use crate::field::Field;
use crate::game::Game;
use crate::rng::Xoshiro256StarStar;

impl Field {
    /// Give `card` the printed identity `data`. Its script's effects — the
    /// `INITIAL` ones it owns — come off through the card's own removal
    /// path, and the new script registers its effects as the deal does;
    /// everything positional stays. The fields a script writes at
    /// initialisation besides effects (`fusion_materials`, the unique
    /// marks) are cleared for the new script to set.
    pub fn reidentify(&mut self, card: CardId, data: CardData) {
        let own: Vec<EffectId> = self.cards[card]
            .indexer
            .iter()
            .copied()
            .filter(|&e| {
                self.effects
                    .get(e)
                    .is_some_and(|x| x.is_flag(flag::INITIAL) && x.owner == Some(card))
            })
            .collect();
        for e in own {
            self.remove_card_effect(card, e);
        }
        let c = &mut self.cards[card];
        c.code = data.code;
        c.data = data;
        c.fusion_materials.clear();
        c.unique_code = 0;
        c.unique_location = 0;
        c.unique_pos = [0, 0];
        c.unique_fieldid = 0;
        c.unique_effect = None;
        c.unique_filter = None;
        self.initialize_card(card);
    }
}

/// A monster that could have been set without tributes: the preferred
/// occupant of a hidden face-down monster seat.
fn may_sit_face_down_in_mzone(d: &CardData) -> bool {
    is_monster(d) && d.level <= 4
}

fn is_monster(d: &CardData) -> bool {
    d.type_ & card_type::MONSTER != 0 && d.type_ & card_type::FUSION == 0
}

fn is_spell_or_trap(d: &CardData) -> bool {
    d.type_ & (card_type::SPELL | card_type::TRAP) != 0
}

/// Fisher–Yates with the duel's own generator type, so a sample is a
/// function of its seed alone.
fn shuffle<T>(v: &mut [T], rng: &mut Xoshiro256StarStar) {
    for i in (1..v.len()).rev() {
        let j = (rng.next() % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
}

impl Game {
    /// A world the viewer cannot tell from this one — see the module note.
    /// The returned game is at the same node, with the same question
    /// pending, the same observation and legal actions for `viewer`, and a
    /// generator seeded from `seed`; a search plays it out from here.
    pub fn determinize(&self, viewer: u8, seed: [u64; 4]) -> Game {
        use crate::board::{location, position};
        let mut g = self.clone();
        let other = 1 - viewer;
        let mut rng = Xoshiro256StarStar::new(seed);

        // The other player's hidden cards the viewer does not know, by the
        // seat they must be legal in.
        let (mut mzone, mut szone, mut free): (Vec<CardId>, Vec<CardId>, Vec<CardId>) =
            (Vec::new(), Vec::new(), Vec::new());
        {
            let f = g.field();
            for (c, card) in f.cards.iter().enumerate() {
                let cur = &card.current;
                if cur.controller != other || self.knowledge().knows(viewer, c) {
                    continue;
                }
                match cur.location {
                    location::HAND | location::DECK => free.push(c),
                    location::MZONE if cur.position & position::FACEDOWN != 0 => mzone.push(c),
                    location::SZONE if cur.position & position::FACEDOWN != 0 => szone.push(c),
                    _ => {}
                }
            }
        }
        // The identities those cards hold, as a pool, partitioned by what
        // each seat class may take.
        let mut monsters: Vec<CardData> = Vec::new();
        let mut big_monsters: Vec<CardData> = Vec::new();
        let mut spells_traps: Vec<CardData> = Vec::new();
        let mut rest: Vec<CardData> = Vec::new();
        for &c in mzone.iter().chain(szone.iter()).chain(free.iter()) {
            let d = g.field().cards[c].data.clone();
            if may_sit_face_down_in_mzone(&d) {
                monsters.push(d);
            } else if is_monster(&d) {
                big_monsters.push(d);
            } else if is_spell_or_trap(&d) {
                spells_traps.push(d);
            } else {
                rest.push(d);
            }
        }
        shuffle(&mut monsters, &mut rng);
        shuffle(&mut big_monsters, &mut rng);
        shuffle(&mut spells_traps, &mut rng);
        let mut assignment: Vec<(CardId, CardData)> = Vec::new();
        for &c in &mzone {
            // The world where a hidden face-down monster was set with
            // tributes is sampled only when nothing smaller is left.
            let d = monsters
                .pop()
                .or_else(|| big_monsters.pop())
                .expect("a monster for every face-down monster");
            assignment.push((c, d));
        }
        for &c in &szone {
            assignment.push((
                c,
                spells_traps
                    .pop()
                    .expect("a spell or trap for every set spell or trap"),
            ));
        }
        let mut pool: Vec<CardData> = monsters;
        pool.append(&mut big_monsters);
        pool.append(&mut spells_traps);
        pool.append(&mut rest);
        shuffle(&mut pool, &mut rng);
        for &c in &free {
            assignment.push((c, pool.pop().expect("an identity for every hidden card")));
        }
        debug_assert!(pool.is_empty());
        for (c, d) in assignment {
            if g.field().cards[c].data.code != d.code {
                g.field_mut().reidentify(c, d);
            }
        }

        // The viewer's own deck: its contents are the viewer's, its order
        // is not.
        let mut order = g.field().players[usize::from(viewer)].main.clone();
        shuffle(&mut order, &mut rng);
        let imposed = g.field_mut().set_deck_order(viewer, &order);
        debug_assert!(imposed);

        // Fresh rolls from here: a different world rolls differently.
        g.field_mut().rng =
            Xoshiro256StarStar::new([rng.next(), rng.next(), rng.next(), rng.next()]);
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{location, position};
    use crate::cards::{card_data, POOL};
    use crate::driver::RandomPolicy;
    use crate::game::{Actor, Config};
    use std::collections::BTreeMap;

    fn pool_deck(game: u64) -> Vec<CardData> {
        let mut rng = RandomPolicy::new(9_000 + game);
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

    fn hidden_codes(g: &Game, viewer: u8) -> BTreeMap<u32, usize> {
        let other = 1 - viewer;
        let mut m = BTreeMap::new();
        for (c, card) in g.field().cards.iter().enumerate() {
            let cur = &card.current;
            let hidden = cur.location == location::HAND
                || cur.location == location::DECK
                || (cur.location & location::ONFIELD != 0
                    && cur.position & position::FACEDOWN != 0);
            if cur.controller == other && hidden && !g.knowledge().knows(viewer, c) {
                *m.entry(card.data.code).or_default() += 1;
            }
        }
        m
    }

    /// **A sampled world is indistinguishable to the viewer.** At random
    /// nodes of random pool games, the viewer's key, the public key and
    /// the legal actions are the sample's too; the viewer's own cards and
    /// the cards they know keep their identities; the other's hidden cards
    /// keep their multiset; and the seats stay legal.
    #[test]
    fn a_sampled_world_is_indistinguishable_to_the_viewer() {
        let mut samples = 0;
        for game in 0..6u64 {
            let deck = pool_deck(game);
            for &depth in &[3usize, 40, 120] {
                let mut g = Game::new([deck.clone(), deck.clone()], [1, 2, 3, game]);
                let mut rng = RandomPolicy::new(400 + game);
                if !at_node(&mut g, &mut rng, depth) {
                    continue;
                }
                let Actor::Player(p) = g.player_to_act() else {
                    continue;
                };
                let before_key = g.infoset_key(p);
                let before_public = g.public_key();
                let before_actions = g.legal_actions();
                let before_hidden = hidden_codes(&g, p);
                let mine: Vec<u32> = g
                    .field()
                    .cards
                    .iter()
                    .filter(|c| c.current.controller == p)
                    .map(|c| c.data.code)
                    .collect();
                for k in 0..3u64 {
                    let mut d = g.determinize(p, [5, 6, 7, 100 * game + 10 * depth as u64 + k]);
                    samples += 1;
                    assert_eq!(
                        d.infoset_key(p),
                        before_key,
                        "game {game} depth {depth}: the key"
                    );
                    assert_eq!(d.public_key(), before_public, "the public key");
                    assert_eq!(d.legal_actions(), before_actions, "the actions");
                    assert_eq!(hidden_codes(&d, p), before_hidden, "the multiset");
                    let still_mine: Vec<u32> = d
                        .field()
                        .cards
                        .iter()
                        .filter(|c| c.current.controller == p)
                        .map(|c| c.data.code)
                        .collect();
                    assert_eq!(still_mine, mine, "the viewer's own cards");
                    // The seats the permutation may touch stay legal. (A
                    // face-down card the viewer knows — one turned over in
                    // play — may be anything, and is not touched.)
                    for (c, card) in d.field().cards.iter().enumerate() {
                        let cur = &card.current;
                        if cur.controller != 1 - p || d.knowledge().knows(p, c) {
                            continue;
                        }
                        if cur.location == location::MZONE && cur.position & position::FACEDOWN != 0
                        {
                            assert!(is_monster(&card.data), "{}", card.data.code);
                        }
                        if cur.location == location::SZONE && cur.position & position::FACEDOWN != 0
                        {
                            assert!(is_spell_or_trap(&card.data), "{}", card.data.code);
                        }
                    }
                    // A known card of the other's is not touched.
                    for (c, card) in g.field().cards.iter().enumerate() {
                        if card.current.controller == 1 - p && g.knowledge().knows(p, c) {
                            assert_eq!(d.field().cards[c].data.code, card.data.code);
                        }
                    }
                }
            }
        }
        assert!(samples >= 20, "{samples}");
    }

    /// **Play continues on a sampled world.** The re-registered scripts
    /// are real: random play from several samples runs to the end with
    /// every listed action accepted.
    #[test]
    fn play_continues_on_a_sampled_world() {
        let deck = pool_deck(7);
        let mut ended = 0;
        for k in 0..4u64 {
            let mut g = Game::new([deck.clone(), deck.clone()], [1, 2, 3, 7]);
            let mut rng = RandomPolicy::new(700 + k);
            assert!(at_node(&mut g, &mut rng, 20));
            let Actor::Player(p) = g.player_to_act() else {
                panic!()
            };
            let mut d = g.determinize(p, [9, 9, 9, k]);
            for _ in 0..6_000 {
                match d.player_to_act() {
                    Actor::Terminal => {
                        ended += 1;
                        break;
                    }
                    Actor::Chance => d.sample_chance().unwrap(),
                    Actor::Player(_) => {
                        let l = d.legal_actions();
                        assert!(!l.is_empty(), "{:?}", d.question());
                        let a = l[rng.below(l.len())].clone();
                        d.apply(&a).unwrap_or_else(|e| panic!("{a:?}: {e:?}"));
                    }
                }
            }
        }
        assert!(ended >= 3, "{ended}");
    }

    /// **The sample is uniform over the unknown.** At the first player
    /// node the other's whole deck and hand are unknown to the viewer: the
    /// identity their first hand card takes, over many samples, is each
    /// code of the decklist in proportion to its copies.
    #[test]
    fn sampling_is_uniform_over_the_unknown() {
        let deck = pool_deck(11);
        let mut g = Game::new([deck.clone(), deck.clone()], [4, 4, 4, 4]);
        let mut rng = RandomPolicy::new(1);
        assert!(at_node(&mut g, &mut rng, 1));
        let Actor::Player(p) = g.player_to_act() else {
            panic!()
        };
        let other = usize::from(1 - p);
        let slot = g.field().players[other].hand[0];
        // The pool: every code among the other's hidden cards, with copies.
        let pool = hidden_codes(&g, p);
        let n = 600u64;
        let mut counts: BTreeMap<u32, usize> = BTreeMap::new();
        for k in 0..n {
            let d = g.determinize(p, [1, 2, 3, k]);
            *counts.entry(d.field().cards[slot].data.code).or_default() += 1;
        }
        let total: usize = pool.values().sum();
        for (&code, &copies) in &pool {
            let expected = n as f64 * copies as f64 / total as f64;
            let got = *counts.get(&code).unwrap_or(&0) as f64;
            assert!(
                got > 0.35 * expected && got < 2.2 * expected,
                "code {code}: {got} of {expected:.1} expected"
            );
        }
    }

    /// **A re-identified card carries the new script.** Given another
    /// printed identity, a card's own effects are the new card's — the
    /// same set a fresh card of that identity registers — and the old
    /// ones are gone; its place and status word are untouched.
    #[test]
    fn a_reidentified_card_carries_the_new_script() {
        let sangan = card_data(26_202_165).expect("Sangan");
        let serpent = card_data(8_131_171).expect("Sinister Serpent");
        let mut f = Field::new(8000);
        let c = f.new_card_nowhere(crate::card::Card::with_data(sangan.clone(), 1));
        f.add_card(1, c, location::HAND, 0, false);
        f.initialize_card(c);
        let status_before = f.cards[c].status;
        let effect_codes = |f: &Field, c: CardId| -> Vec<u32> {
            let mut v: Vec<u32> = f.cards[c]
                .indexer
                .iter()
                .filter_map(|&e| f.effects.get(e))
                .filter(|e| e.is_flag(flag::INITIAL))
                .map(|e| e.code)
                .collect();
            v.sort_unstable();
            v
        };
        let as_sangan = effect_codes(&f, c);
        f.reidentify(c, serpent.clone());
        assert_eq!(f.cards[c].data.code, serpent.code);
        assert_eq!(f.cards[c].code, serpent.code);
        assert_eq!(f.cards[c].current.location, location::HAND, "in place");
        assert_eq!(f.cards[c].status, status_before, "the status word");
        // What a fresh Serpent registers.
        let mut fresh = Field::new(8000);
        let s = fresh.new_card_nowhere(crate::card::Card::with_data(serpent, 1));
        fresh.add_card(1, s, location::HAND, 0, false);
        fresh.initialize_card(s);
        assert_eq!(
            effect_codes(&f, c),
            effect_codes(&fresh, s),
            "the new script"
        );
        assert_ne!(effect_codes(&f, c), as_sangan, "and not the old one");
        // Back again, and the field effects index holds nothing of the Serpent's.
        f.reidentify(c, sangan);
        assert_eq!(effect_codes(&f, c), as_sangan);
        for &e in f.field_effects.indexer.iter() {
            let x = f.effects.get(e).unwrap();
            assert!(
                !(x.owner == Some(c) && x.is_flag(flag::INITIAL) && x.code == 0x1200),
                "a Serpent effect left behind"
            );
        }
    }

    /// Determinizing with the forced-node skip off is the same world.
    #[test]
    fn the_sample_keeps_the_configuration() {
        let deck = pool_deck(2);
        let g = Game::configured(
            [deck.clone(), deck],
            [1, 2, 3, 4],
            Config {
                skip_forced: false,
                ..Config::default()
            },
        );
        let d = g.determinize(0, [1, 1, 1, 1]);
        assert!(!d.config().skip_forced);
        assert_eq!(d.player_to_act(), g.player_to_act());
    }
}
