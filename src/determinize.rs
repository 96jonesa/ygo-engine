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
//! A caller that weighs worlds itself (a belief over the hidden cards that
//! is not uniform) builds them with [`Game::determinize_with`]: given an
//! identity for each card [`Game::hidden`] lists, it checks the assignment
//! is a completion (the hidden multiset, each seat's code legal for it) and
//! builds that world. [`Game::determinize`] is the same construction with
//! the assignment drawn uniformly.
//!
//! Two legality constraints on the permutation, because a world the engine
//! could not have reached is not a completion: a face-down card in the
//! monster zone is a monster, any monster (a level-five-or-more one is a
//! tribute set, or a card that sets without tributing: whether the history
//! makes it plausible is a belief's question, not the rules'), and a set
//! spell or trap is a spell or trap. Hands and decks take anything left.
//! Within those constraints the sample is uniform. A face-down card the viewer
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

/// The cards a viewer cannot identify, and the identities they hold: what
/// a world may permute. The other player's hidden cards not known to the
/// viewer, by the seat class that constrains them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hidden {
    /// Face-down in the monster zone: each must take a monster.
    pub monster_seats: Vec<CardId>,
    /// Set in the spell and trap zone: each must take a spell or trap.
    pub spell_trap_seats: Vec<CardId>,
    /// In the hand or the deck: anything.
    pub free: Vec<CardId>,
    /// The identities those cards hold between them, as printed codes,
    /// sorted: what any completion must hand out, each exactly once.
    pub codes: Vec<u32>,
}

impl Hidden {
    /// Every card in the permutation, in seat-class order.
    pub fn cards(&self) -> impl Iterator<Item = CardId> + '_ {
        self.monster_seats
            .iter()
            .chain(&self.spell_trap_seats)
            .chain(&self.free)
            .copied()
    }
}

impl Game {
    /// What `viewer` cannot identify: the cards a world may re-identify,
    /// and the identities to share among them.
    pub fn hidden(&self, viewer: u8) -> Hidden {
        use crate::board::{location, position};
        let other = 1 - viewer;
        let (mut monster_seats, mut spell_trap_seats, mut free) =
            (Vec::new(), Vec::new(), Vec::new());
        let f = self.field();
        for (c, card) in f.cards.iter().enumerate() {
            let cur = &card.current;
            if cur.controller != other || self.knowledge().knows(viewer, c) {
                continue;
            }
            match cur.location {
                location::HAND | location::DECK => free.push(c),
                location::MZONE if cur.position & position::FACEDOWN != 0 => monster_seats.push(c),
                location::SZONE if cur.position & position::FACEDOWN != 0 => {
                    spell_trap_seats.push(c)
                }
                _ => {}
            }
        }
        let mut codes: Vec<u32> = monster_seats
            .iter()
            .chain(&spell_trap_seats)
            .chain(&free)
            .map(|&c| f.cards[c].data.code)
            .collect();
        codes.sort_unstable();
        Hidden {
            monster_seats,
            spell_trap_seats,
            free,
            codes,
        }
    }

    /// A world the viewer cannot tell from this one — see the module note.
    /// The returned game is at the same node, with the same question
    /// pending, the same observation and legal actions for `viewer`, and a
    /// generator seeded from `seed`; a search plays it out from here.
    ///
    /// `viewer` must be the player the pending question asks, or there must
    /// be no player's question pending (a chance node, the game's end). The
    /// world keeps the pending question as it is, and another player's
    /// question was built from that player's real cards: their menu would
    /// name identities this world has changed.
    ///
    /// `seed` is the generator's raw state, so it should be well mixed
    /// ([`crate::rng::expand_seed`] of a counter is): xoshiro's first draws
    /// depend on few of its words, and raw seeds that differ in one word
    /// give correlated samples.
    pub fn determinize(&self, viewer: u8, seed: [u64; 4]) -> Game {
        debug_assert!(
            self.asker().is_none_or(|a| a == viewer),
            "determinize for a viewer the pending question does not ask"
        );
        let mut rng = Xoshiro256StarStar::new(seed);
        let hidden = self.hidden(viewer);
        let f = self.field();

        // The identities, as a pool, partitioned by what each seat class
        // may take.
        let mut monsters: Vec<CardData> = Vec::new();
        let mut spells_traps: Vec<CardData> = Vec::new();
        let mut rest: Vec<CardData> = Vec::new();
        for c in hidden.cards() {
            let d = f.cards[c].data.clone();
            if is_monster(&d) {
                monsters.push(d);
            } else if is_spell_or_trap(&d) {
                spells_traps.push(d);
            } else {
                rest.push(d);
            }
        }
        shuffle(&mut monsters, &mut rng);
        shuffle(&mut spells_traps, &mut rng);
        let mut assignment: Vec<(CardId, CardData)> = Vec::new();
        // A seat class whose own identities run out takes from the rest of
        // the pool rather than failing. No card in the pool can be set
        // anywhere but its own class's seat (`no_pool_card_sets_out_of_class`
        // pins that), so the fallback never fires today; a card that can
        // (a monster set in a spell and trap zone) makes it fire, and its
        // seat rules belong here first.
        for &c in &hidden.monster_seats {
            let d = monsters
                .pop()
                .or_else(|| spells_traps.pop())
                .or_else(|| rest.pop())
                .expect("an identity for every hidden card");
            assignment.push((c, d));
        }
        for &c in &hidden.spell_trap_seats {
            let d = spells_traps
                .pop()
                .or_else(|| rest.pop())
                .or_else(|| monsters.pop())
                .expect("an identity for every hidden card");
            assignment.push((c, d));
        }
        let mut pool: Vec<CardData> = monsters;
        pool.append(&mut spells_traps);
        pool.append(&mut rest);
        shuffle(&mut pool, &mut rng);
        for &c in &hidden.free {
            assignment.push((c, pool.pop().expect("an identity for every hidden card")));
        }
        debug_assert!(pool.is_empty());
        self.world(viewer, assignment, &mut rng)
    }

    /// The world in which the viewer's unidentified cards (`hidden(viewer)`)
    /// hold the given identities: `assignment` names a printed code for
    /// every one of those cards, the codes being exactly the hidden
    /// multiset, each seat's code legal for it (a monster face-down in the
    /// monster zone, a spell or trap set in the spell and trap zone). The
    /// viewer's own deck order and the generator are drawn from `seed`, as
    /// [`Game::determinize`] draws them. For a search that weighs worlds
    /// itself rather than sampling them uniformly.
    ///
    /// The check is what the rules guarantee and no more: the seat class.
    /// Any monster may sit face-down in the monster zone, a level-five-or-
    /// more one included (a tribute set, a card that sets without
    /// tributing, an effect that sets it there). Whether an identity is
    /// *plausible* given the history (no tribute seen, say) is the
    /// caller's belief to weigh; [`Game::determinize`] is uniform over the
    /// legal completions for the same reason.
    pub fn determinize_with(
        &self,
        viewer: u8,
        assignment: &[(CardId, u32)],
        seed: [u64; 4],
    ) -> Result<Game, String> {
        if let Some(a) = self.asker().filter(|&a| a != viewer) {
            return Err(format!(
                "the pending question asks player {a}: a world for player {viewer} would keep it as built from the real cards"
            ));
        }
        let hidden = self.hidden(viewer);
        let f = self.field();
        let mut by_card: std::collections::HashMap<CardId, u32> = std::collections::HashMap::new();
        for &(c, code) in assignment {
            if by_card.insert(c, code).is_some() {
                return Err(format!("card {c} is assigned twice"));
            }
        }
        let mut given: Vec<u32> = Vec::with_capacity(assignment.len());
        let mut out: Vec<(CardId, CardData)> = Vec::with_capacity(assignment.len());
        // An identity's data, from a hidden card that holds that code now.
        let data_of = |code: u32| -> Option<CardData> {
            hidden
                .cards()
                .find(|&c| f.cards[c].data.code == code)
                .map(|c| f.cards[c].data.clone())
        };
        for c in hidden.cards() {
            let code = *by_card
                .get(&c)
                .ok_or_else(|| format!("card {c} is hidden from the viewer but not assigned"))?;
            let d = data_of(code)
                .ok_or_else(|| format!("code {code} is not among the hidden identities"))?;
            if hidden.monster_seats.contains(&c) && !is_monster(&d) {
                return Err(format!(
                    "card {c} is a face-down monster; {code} is not a monster"
                ));
            }
            if hidden.spell_trap_seats.contains(&c) && !is_spell_or_trap(&d) {
                return Err(format!(
                    "card {c} is a set spell or trap; {code} is neither"
                ));
            }
            given.push(code);
            out.push((c, d));
        }
        if out.len() != by_card.len() {
            return Err("the assignment names a card the viewer can identify".into());
        }
        given.sort_unstable();
        if given != hidden.codes {
            return Err("the assignment's codes are not the hidden multiset".into());
        }
        let mut rng = Xoshiro256StarStar::new(seed);
        Ok(self.world(viewer, out, &mut rng))
    }

    /// The player the pending question asks, if a player's question is
    /// pending.
    fn asker(&self) -> Option<u8> {
        self.question().and_then(crate::game::asked_player)
    }

    /// Apply an assignment of identities, reshuffle the viewer's own deck
    /// and reseed the generator, all from `rng`.
    fn world(
        &self,
        viewer: u8,
        assignment: Vec<(CardId, CardData)>,
        rng: &mut Xoshiro256StarStar,
    ) -> Game {
        let mut g = self.clone();
        for (c, d) in assignment {
            if g.field().cards[c].data.code != d.code {
                g.field_mut().reidentify(c, d);
            }
        }

        // The viewer's own deck: its contents are the viewer's, its order
        // is not.
        let mut order = g.field().players[usize::from(viewer)].main.clone();
        shuffle(&mut order, rng);
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
    use crate::game::{Action, Actor, Config};
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

    /// Positions from random pool games where the player to act faces
    /// hidden cards: `(game, viewer)`.
    fn positions() -> Vec<(Game, u8)> {
        let mut out = Vec::new();
        for game in 0..6u64 {
            let deck = pool_deck(game);
            for &depth in &[3usize, 40, 120] {
                let mut g = Game::new([deck.clone(), deck.clone()], [1, 2, 3, game]);
                let mut rng = RandomPolicy::new(400 + game);
                if !at_node(&mut g, &mut rng, depth) {
                    continue;
                }
                if let Actor::Player(p) = g.player_to_act() {
                    out.push((g, p));
                }
            }
        }
        assert!(out.len() >= 10, "{} positions", out.len());
        out
    }

    /// The refusal, where a world was expected not to be built.
    fn refused(r: Result<Game, String>) -> String {
        match r {
            Ok(_) => panic!("the assignment was accepted"),
            Err(e) => e,
        }
    }

    /// The assignment a world holds: each hidden card's code there.
    fn assignment_in(world: &Game, hidden: &Hidden) -> Vec<(CardId, u32)> {
        hidden
            .cards()
            .map(|c| (c, world.field().cards[c].data.code))
            .collect()
    }

    mod hidden {
        use super::*;

        /// **The hidden cards are the other player's unknown ones, and
        /// their codes are the hidden multiset.**
        #[test]
        fn lists_the_cards_and_codes_a_world_may_permute() {
            for (g, p) in positions() {
                let h = g.hidden(p);
                assert_eq!(h.cards().count(), h.codes.len());
                let mut from_cards: BTreeMap<u32, usize> = BTreeMap::new();
                for &code in &h.codes {
                    *from_cards.entry(code).or_default() += 1;
                }
                assert_eq!(from_cards, hidden_codes(&g, p));
                for c in h.cards() {
                    assert_eq!(g.field().cards[c].current.controller, 1 - p);
                    assert!(!g.knowledge().knows(p, c));
                }
            }
        }
    }

    /// **No pool card can be set outside its own class's seat.** A
    /// monster may be set in a spell and trap zone only under
    /// `EFFECT_MONSTER_SSET` (`Field::is_setable_szone`). `determinize`
    /// and `determinize_with` assume no hidden card does that: a set spell
    /// or trap seat holds a spell or trap. Every pool card, initialised in
    /// a hand, is checked for the effect. A card that grants it fails this
    /// test, and the seat rules in this module must be extended for it
    /// (with that card to test against) before the test is relaxed.
    #[test]
    fn no_pool_card_sets_out_of_class() {
        let mut f = Field::new(8000);
        let mut granting = Vec::new();
        for &code in POOL.iter() {
            let mut c = crate::card::Card::with_data(card_data(code).expect("pool data"), 0);
            c.current.controller = 0;
            let id = f.new_card(c);
            f.add_card(0, id, location::HAND, 0, false);
            f.initialize_card(id);
            if f.is_affected_by_effect(id, crate::event::code::MONSTER_SSET)
                .is_some()
            {
                granting.push(code);
            }
        }
        // The detection is not blind: a monster given the effect, as a
        // script grants it, is seen.
        let control = f.new_card(crate::card::Card::with_data(
            card_data(POOL[0]).expect("pool data"),
            0,
        ));
        f.add_card(0, control, location::HAND, 0, false);
        let mut e = crate::effect::Effect::new(
            crate::effect::effect_type::SINGLE,
            crate::event::code::MONSTER_SSET,
        );
        e.owner = Some(control);
        e.handler = Some(control);
        let e = f.new_effect(e);
        f.cards[control]
            .single_effect
            .insert(crate::event::code::MONSTER_SSET, e);
        f.cards[control].indexer.insert(e);
        assert!(
            f.is_affected_by_effect(control, crate::event::code::MONSTER_SSET)
                .is_some(),
            "the check must see a granted MONSTER_SSET"
        );
        assert!(
            granting.is_empty(),
            "{granting:?} may be set in a spell and trap zone as a monster: extend determinize's \
             seat rules for it first"
        );
    }

    mod determinize_with {
        use super::*;

        /// **The world holds exactly the assignment, and the viewer cannot
        /// tell it from the original.** The assignment is read off a
        /// sampled world, so it is a legal completion.
        #[test]
        fn gives_each_hidden_card_its_assigned_identity() {
            let mut checked = 0;
            for (mut g, p) in positions() {
                let h = g.hidden(p);
                let sample = g.determinize(p, [9, 9, 9, checked]);
                let a = assignment_in(&sample, &h);
                let mut w = g
                    .determinize_with(p, &a, [1, 2, 3, 4])
                    .expect("a legal completion");
                assert_eq!(assignment_in(&w, &h), a);
                assert_eq!(w.infoset_key(p), g.infoset_key(p), "the viewer's key");
                assert_eq!(w.public_key(), g.public_key());
                assert_eq!(w.legal_actions(), g.legal_actions());
                assert_eq!(hidden_codes(&w, p), hidden_codes(&g, p));
                checked += 1;
            }
            assert!(checked >= 10);
        }

        /// **The original identities are a completion too**, and give
        /// back the original hidden cards.
        #[test]
        fn accepts_the_true_assignment() {
            for (g, p) in positions() {
                let h = g.hidden(p);
                let truth = assignment_in(&g, &h);
                let w = g.determinize_with(p, &truth, [5, 5, 5, 5]).unwrap();
                assert_eq!(assignment_in(&w, &h), truth);
            }
        }

        /// **A world is built only for the player the question asks.** For
        /// the other player the pending question would keep the asker's real
        /// offers; the call is refused.
        #[test]
        fn refuses_a_viewer_the_question_does_not_ask() {
            for (g, p) in positions() {
                let other = 1 - p;
                let truth: Vec<(CardId, u32)> = g
                    .hidden(other)
                    .cards()
                    .map(|c| (c, g.field().cards[c].data.code))
                    .collect();
                let e = refused(g.determinize_with(other, &truth, [2, 2, 2, 2]));
                assert!(e.contains("pending question asks"), "{e}");
            }
        }

        /// **Anything but a legal completion is refused**: a card left out,
        /// a card named twice, a card the viewer can identify, a code
        /// outside the hidden multiset, and a seat given an identity it
        /// cannot hold.
        #[test]
        fn refuses_an_assignment_that_is_not_a_completion() {
            let mut seats_tested = 0;
            for (g, p) in positions() {
                let h = g.hidden(p);
                let truth = assignment_in(&g, &h);
                if truth.len() < 2 {
                    continue;
                }
                let seed = [1, 1, 1, 1];
                assert!(refused(g.determinize_with(p, &truth[1..], seed)).contains("not assigned"));
                let mut twice = truth.clone();
                twice.push(truth[0]);
                assert!(refused(g.determinize_with(p, &twice, seed)).contains("twice"));
                let own = g
                    .field()
                    .cards
                    .iter()
                    .position(|c| c.current.controller == p)
                    .expect("the viewer has cards");
                let mut foreign = truth.clone();
                foreign.push((own, truth[0].1));
                assert!(refused(g.determinize_with(p, &foreign, seed)).contains("can identify"));
                let mut alien = truth.clone();
                alien[0].1 = u32::MAX;
                assert!(refused(g.determinize_with(p, &alien, seed)).contains("not among"));
                // Two codes swapped between a face-down monster and a free
                // card holding a spell or trap: the multiset holds, the
                // seat does not.
                if let Some(&m) = h.monster_seats.first() {
                    let st = h
                        .free
                        .iter()
                        .copied()
                        .find(|&c| is_spell_or_trap(&g.field().cards[c].data));
                    if let Some(st) = st {
                        let mut bad = truth.clone();
                        let (im, is) = (
                            bad.iter().position(|x| x.0 == m).unwrap(),
                            bad.iter().position(|x| x.0 == st).unwrap(),
                        );
                        let (cm, cs) = (bad[im].1, bad[is].1);
                        bad[im].1 = cs;
                        bad[is].1 = cm;
                        assert!(
                            refused(g.determinize_with(p, &bad, seed)).contains("not a monster")
                        );
                        seats_tested += 1;
                    }
                }
                // A code repeated in place of another: every card is named
                // and every code is hidden, but the multiset is wrong.
                if let Some(j) = truth.iter().position(|x| x.1 != truth[0].1) {
                    let mut skewed = truth.clone();
                    skewed[j].1 = truth[0].1;
                    let e = refused(g.determinize_with(p, &skewed, seed));
                    assert!(
                        e.contains("multiset")
                            || e.contains("not a monster")
                            || e.contains("neither"),
                        "{e}"
                    );
                }
            }
            assert!(
                seats_tested >= 1,
                "no position had a face-down monster to test"
            );
        }
    }

    /// **Whether the other player had a response, and declined, leaves no
    /// trace.** A world sampled for the player to act differs from the real
    /// game only in what that player cannot see, so the other player may
    /// hold a response in one and not the other. The same move is made in
    /// both; then, while the other player is asked, they decline every
    /// response window offered (in the game without the response the
    /// window is never asked: a window with nothing to do is answered for
    /// them). When both reach the first player's next decision, that
    /// player's key is the same: being asked, and declining, is not seen.
    #[test]
    fn a_declined_window_leaves_no_trace() {
        // Advance to `viewer`'s next decision, declining every window the
        // other player is offered. None if anything else intervenes.
        fn to_next_decision(g: &mut Game, viewer: u8) -> Option<usize> {
            let mut declined = 0;
            for _ in 0..64 {
                match g.player_to_act() {
                    Actor::Player(p) if p == viewer => return Some(declined),
                    Actor::Player(_) if g.legal_actions().contains(&Action::Decline) => {
                        g.apply(&Action::Decline).ok()?;
                        declined += 1;
                    }
                    _ => return None,
                }
            }
            None
        }
        let (mut compared, mut asymmetric) = (0, 0);
        for game in 0..12u64 {
            let deck = pool_deck(game);
            let mut g = Game::new([deck.clone(), deck.clone()], [5, 1, 9, game]);
            let mut rng = RandomPolicy::new(600 + game);
            for _ in 0..400 {
                match g.player_to_act() {
                    Actor::Terminal => break,
                    Actor::Chance => g.sample_chance().unwrap(),
                    Actor::Player(v) => {
                        let l = g.legal_actions();
                        let a = l[rng.below(l.len())].clone();
                        let mut w = g
                            .determinize(v, crate::rng::expand_seed(game * 1000 + compared as u64));
                        // Only the other player's cards differ: the viewer's
                        // own deck keeps its real order, so the viewer draws
                        // the same cards in both.
                        let order = g.field().players[usize::from(v)].main.clone();
                        assert!(w.field_mut().set_deck_order(v, &order));
                        // And the generator: a hand shuffled after a reveal
                        // rolls it, and the viewer sees their own hand's order.
                        w.field_mut().rng = g.field().rng.clone();
                        let mut real = g.clone();
                        if real.apply(&a).is_ok() && w.apply(&a).is_ok() {
                            if let (Some(dr), Some(dw)) =
                                (to_next_decision(&mut real, v), to_next_decision(&mut w, v))
                            {
                                // A move can reveal the other player's cards
                                // (a discard, a look at the hand), and those
                                // differ between the worlds: a visible
                                // difference, not a trace. Compare only where
                                // the same identities were revealed, so the
                                // hidden multisets still agree.
                                if real.hidden(v).codes != w.hidden(v).codes {
                                    g.apply(&a).unwrap();
                                    continue;
                                }
                                assert_eq!(
                                    real.infoset_key(v),
                                    w.infoset_key(v),
                                    "game {game}: the windows declined ({dr} real, {dw} sampled) are seen after {a:?}"
                                );
                                compared += 1;
                                if dr != dw {
                                    asymmetric += 1;
                                }
                            }
                        }
                        g.apply(&a).unwrap();
                    }
                }
            }
        }
        assert!(compared >= 200, "{compared} comparisons");
        assert!(
            asymmetric >= 1,
            "no case where one game asked and the other did not"
        );
        eprintln!("compared {compared}, asymmetric {asymmetric}");
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

    /// **A face-down monster seat is uniform over the hidden monsters**,
    /// big ones included: no level preference. At positions where a hidden
    /// face-down monster shares the pool with monsters of level five or
    /// more, the share of samples putting a big one there matches their
    /// share of the hidden monsters.
    #[test]
    fn a_face_down_monster_seat_is_uniform_over_the_monsters() {
        let mut tested = 0;
        for (g, p) in positions() {
            let h = g.hidden(p);
            let Some(&seat) = h.monster_seats.first() else {
                continue;
            };
            let monsters: Vec<u32> = h
                .cards()
                .map(|c| &g.field().cards[c].data)
                .filter(|d| is_monster(d))
                .map(|d| d.level)
                .collect();
            let big = monsters.iter().filter(|&&l| l >= 5).count();
            if big == 0 || big == monsters.len() {
                continue;
            }
            let expected = big as f64 / monsters.len() as f64;
            let n = 4000u64;
            let hits = (0..n)
                .filter(|&k| {
                    // Seeded as callers seed it: `expand_seed` mixes all four
                    // words (raw seeds differing in one word give the
                    // generator correlated first draws).
                    g.determinize(p, crate::rng::expand_seed(k)).field().cards[seat]
                        .data
                        .level
                        >= 5
                })
                .count() as f64;
            let share = hits / n as f64;
            let sd = (expected * (1.0 - expected) / n as f64).sqrt();
            assert!(
                (share - expected).abs() < 5.0 * sd,
                "big monsters in the seat {share:.3}, expected {expected:.3}"
            );
            tested += 1;
        }
        assert!(
            tested >= 1,
            "no position had a face-down monster with big and small monsters hidden"
        );
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
