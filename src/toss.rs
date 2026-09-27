//! Tossing coins.
//!
//! `field::process(Processors::TossCoin&)`. Three cases — and they are
//! numbered 0, 1 and **3**.
//!
//! ## The jump table, because reading it off the code is a trap
//!
//! There is no case 2. Case 0 can end the unit, fall through to case 1, or
//! jump to case 3; case 3 goes back to case 1.
//!
//! | from | `arg.step` | runs next | when |
//! |---|---|---|---|
//! | 0 | — (returns true) | nothing | a `TOSS_COIN_REPLACE` effect took over |
//! | 0 | `2` | **case 3** | a `TOSS_COIN_CHOOSE` effect set the results |
//! | 0 | — (returns false) | case 1 | nothing intervened: the coins were rolled |
//! | 3 | `0` | **case 1** | always |
//!
//! Both jumps are the increment rule: the processor adds one after a
//! handler returns false, so `arg.step = n` means case `n + 1` runs. Case
//! 0 writing `2` therefore reaches case 3, and case 3 writing `0` reaches
//! case 1 — the number written is never the case that runs.
//!
//! ## Three ways to get a result, and only one of them rolls
//!
//! `EFFECT_TOSS_COIN_REPLACE` takes the toss over entirely: the unit ends
//! and the replacing effect is responsible for everything, including any
//! message. `EFFECT_TOSS_COIN_CHOOSE` lets an effect *write the results*
//! and then rejoins at case 3, which announces them as if they had been
//! rolled. Only with neither does the unit call the RNG.
//!
//! Both look for the **first activateable** effect and stop; and both size
//! `coin_results` to `count` before solving, so the effect has somewhere to
//! write.
//!
//! ## `EVENT_TOSS_COIN_NEGATE` fires before the result is known
//!
//! The negate window opens in case 0 (and again in case 3) — *after* the
//! coins are shown but *before* case 1 tallies them and raises
//! `EVENT_TOSS_COIN`. Something that replaces the result does so in
//! between. Collapsing the two cases into one would close that window.
//!
//! ## The tally is packed into one integer
//!
//! `EVENT_TOSS_COIN`'s value is `(tails << 16) | (heads << 8) | count`.
//! Three numbers in one word, which is the reference's convention for
//! event values and worth reading twice before assuming a byte order.

use crate::event::{code, EffectId};
use crate::field::{Field, Message};
use crate::processor::Kind;

/// `COIN_HEADS` / `COIN_TAILS` (`common.h`). Heads is **1**.
pub mod coin {
    pub const TAILS: u8 = 0;
    pub const HEADS: u8 = 1;
}

/// `EVENT_TOSS_COIN`'s value: `(tails << 16) | (heads << 8) | count`.
///
/// Three numbers in one word, which is the reference's convention for
/// event values. A free function because the packing is the whole of what
/// case 1 decides, and it is worth being able to test it without driving a
/// duel to the point where the event is raised and immediately drained.
///
/// `count` is what was *asked for*; heads and tails are what is actually
/// in `coin_results`, which a choosing effect may have resized.
pub fn toss_event_value(results: &[bool], count: u8) -> u32 {
    let heads = results.iter().filter(|&&r| r).count() as u32;
    let tails = results.len() as u32 - heads;
    (tails << 16) | (heads << 8) | u32::from(count)
}

impl Field {
    /// Queue a coin toss.
    pub fn toss_coin(
        &mut self,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        playerid: u8,
        count: u8,
    ) {
        self.emplace(Kind::TossCoin {
            reason_effect,
            reason_player,
            playerid,
            count,
        });
    }

    /// One step of `TossCoin`.
    pub(crate) fn toss_coin_step(
        &mut self,
        step: u16,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        playerid: u8,
        count: u8,
    ) -> bool {
        match step {
            0 => self.toss_coin_roll(reason_effect, reason_player, playerid, count),
            1 => self.toss_coin_tally(reason_effect, reason_player, playerid, count),
            3 => self.toss_coin_announce_chosen(reason_effect, reason_player, playerid, count),
            4 => self.toss_coin_host_result(reason_effect, reason_player, playerid, count),
            // There is no case 2, and nothing past 3. The reference's
            // switch falls through to `return TRUE`.
            _ => true,
        }
    }

    /// The faithful mode's faces: one roll per coin, `HEADS` on a 1.
    /// Shared with [`Field::sample_chance`], which rolls the same for the
    /// solver mode's question.
    pub(crate) fn roll_coins(&mut self, count: u8) -> Vec<bool> {
        (0..count)
            .map(|_| self.rng.next_integer(0, 1) == i32::from(coin::HEADS))
            .collect()
    }

    /// Case 0: let an effect take over, or roll.
    fn toss_coin_roll(
        &mut self,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        playerid: u8,
        count: u8,
    ) -> bool {
        let ev = {
            let mut e = crate::event::Event::new(0);
            e.event_cards.clear();
            e.event_player = playerid;
            e.event_value = u32::from(count);
            e.reason = 0;
            e.reason_effect = reason_effect;
            e.reason_player = reason_player;
            e
        };
        self.core.coin_results.clear();

        // A replacement ends the unit outright: the effect owns the whole
        // toss, message included.
        if let Some(e) = self.first_activateable_toss(code::TOSS_COIN_REPLACE, &ev) {
            self.core.coin_results.resize(count as usize, false);
            let p = self.effect_handler_player(e);
            self.solve_continuous(p, e, ev.clone());
            return true;
        }
        // A chooser writes the results and rejoins at case 3, which
        // announces them as if they had been rolled.
        if let Some(e) = self.first_activateable_toss(code::TOSS_COIN_CHOOSE, &ev) {
            self.core.coin_results.resize(count as usize, false);
            let p = self.effect_handler_player(e);
            self.solve_continuous(p, e, ev.clone());
            // 2, not 3: the increment lands on case 3. See the module note.
            self.set_step(2);
            return false;
        }

        // Solver mode: the faces are the host's to give. Ask, and read them
        // back at step 4 — the message, the tally and the negation window
        // are then exactly the generator's path.
        if self.core.chance_mode {
            self.emplace(Kind::SelectCoin {
                player: playerid,
                count,
            });
            self.set_step(3);
            return false;
        }

        let results = self.roll_coins(count);
        self.core.coin_results.clone_from(&results);
        self.messages.push(Message::TossCoin {
            player: playerid,
            results,
        });
        self.raise_toss_negate(reason_effect, reason_player, playerid, count);
        false
    }

    /// Step 4, solver mode: the host's bitmask becomes the results — bit
    /// `i` is coin `i`, set for heads — and the toss continues as rolled.
    fn toss_coin_host_result(
        &mut self,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        playerid: u8,
        count: u8,
    ) -> bool {
        let mask = self.core.returns.get() as u32;
        let results: Vec<bool> = (0..count).map(|i| mask & (1 << i) != 0).collect();
        self.core.coin_results.clone_from(&results);
        self.messages.push(Message::TossCoin {
            player: playerid,
            results,
        });
        self.raise_toss_negate(reason_effect, reason_player, playerid, count);
        self.set_step(0);
        false
    }

    /// Case 3: announce results an effect chose, then go to case 1.
    fn toss_coin_announce_chosen(
        &mut self,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        playerid: u8,
        count: u8,
    ) -> bool {
        // 0, not 1: the increment lands on case 1.
        self.set_step(0);
        let results: Vec<bool> = self
            .core
            .coin_results
            .iter()
            .take(count as usize)
            .copied()
            .collect();
        self.messages.push(Message::TossCoin {
            player: playerid,
            results,
        });
        self.raise_toss_negate(reason_effect, reason_player, playerid, count);
        false
    }

    /// Case 1: count them up and raise the event everything else listens
    /// for.
    fn toss_coin_tally(
        &mut self,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        playerid: u8,
        count: u8,
    ) -> bool {
        let value = toss_event_value(&self.core.coin_results, count);
        self.raise_event(
            None,
            code::TOSS_COIN,
            reason_effect,
            0,
            reason_player,
            playerid,
            value,
        );
        self.process_instant_event();
        true
    }

    /// The window between showing the coins and acting on them.
    fn raise_toss_negate(
        &mut self,
        reason_effect: Option<EffectId>,
        reason_player: u8,
        playerid: u8,
        count: u8,
    ) {
        self.raise_event(
            None,
            code::TOSS_COIN_NEGATE,
            reason_effect,
            0,
            reason_player,
            playerid,
            u32::from(count),
        );
        self.process_instant_event();
    }

    /// The first continuous effect of `which` that can activate on `ev`.
    fn first_activateable_toss(
        &mut self,
        which: u32,
        ev: &crate::event::Event,
    ) -> Option<EffectId> {
        for e in self.field_effects.continuous.equal_range(which).to_vec() {
            let p = self.effect_handler_player(e);
            if self.is_activateable(e, p, ev, false, false, false, false, false) {
                return Some(e);
            }
        }
        None
    }

    fn effect_handler_player(&self, e: EffectId) -> u8 {
        self.effects.get(e).map_or(crate::event::PLAYER_NONE, |x| {
            x.get_handler_player(&self.cards)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **In solver mode the toss asks, then reads the answer bit by bit.**
    /// Bit `i` of the host's mask is coin `i`; the results message, the
    /// tally and the negation window follow exactly as after a roll.
    #[test]
    fn in_chance_mode_the_toss_asks_for_its_faces() {
        let mut f = Field::new(8000);
        f.set_chance_mode(true);
        f.toss_coin(None, 0, 0, 3);
        let mut asked = None;
        for _ in 0..64 {
            match f.process() {
                Status::Awaiting => {
                    asked = f.messages.last().cloned();
                    break;
                }
                Status::Continue => {}
                Status::End => break,
            }
        }
        assert!(
            matches!(
                asked,
                Some(Message::SelectCoin {
                    player: 0,
                    count: 3
                })
            ),
            "the coin question, for three coins: {asked:?}"
        );
        f.core.returns.set(0b101);
        for _ in 0..64 {
            if f.messages
                .iter()
                .any(|m| matches!(m, Message::TossCoin { .. }))
            {
                break;
            }
            assert!(
                !matches!(f.process(), Status::Awaiting),
                "no second question"
            );
        }
        let results = f
            .messages
            .iter()
            .find_map(|m| match m {
                Message::TossCoin { results, .. } => Some(results.clone()),
                _ => None,
            })
            .expect("the toss was announced");
        assert_eq!(
            results,
            vec![true, false, true],
            "heads, tails, heads — bit i is coin i"
        );
        assert_eq!(f.core.coin_results, vec![true, false, true]);
    }
    use crate::board::{location, position};
    use crate::card::{card_type, status, Card, CardData};
    use crate::duel::phases;
    use crate::effect::{effect_type, Effect};
    use crate::event::CardId;
    use crate::processor::Status;

    fn field() -> Field {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        f
    }

    fn on_field(f: &mut Field, controller: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 60000,
                type_: card_type::MONSTER,
                level: 4,
                ..Default::default()
            },
            controller,
        );
        c.current.controller = controller;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        let seat = f.players[controller as usize]
            .mzone
            .iter()
            .position(Option::is_none)
            .expect("a free seat") as u32;
        f.add_card(controller, id, location::MZONE, seat, false);
        f.cards[id].current.position = position::FACEUP_ATTACK;
        id
    }

    /// A registered continuous effect of `code_` belonging to `controller`.
    fn toss_effect(f: &mut Field, controller: u8, code_: u32) -> crate::event::EffectId {
        let source = on_field(f, controller);
        let mut e = Effect::new(effect_type::CONTINUOUS | effect_type::ACTIONS, code_);
        e.owner = Some(source);
        e.handler = Some(source);
        e.effect_owner = controller;
        e.range = u16::from(location::MZONE);
        let id = f.new_effect(e);
        f.field_effects.continuous.insert(code_, id);
        f.field_effects.indexer.insert(id);
        id
    }

    fn run(f: &mut Field) -> Status {
        for _ in 0..512 {
            match f.process() {
                Status::Continue => continue,
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn unit_step(f: &Field) -> Option<u16> {
        f.core
            .units
            .iter()
            .chain(f.core.subunits.iter())
            .find_map(|u| match u.kind {
                Kind::TossCoin { .. } => Some(u.step),
                _ => None,
            })
    }

    fn coin_message(f: &Field) -> Option<(u8, Vec<bool>)> {
        f.messages.iter().find_map(|m| match m {
            Message::TossCoin { player, results } => Some((*player, results.clone())),
            _ => None,
        })
    }

    /// **The coins are rolled and recorded.** `coin_results` is poisoned
    /// with the wrong length first, so a unit that never wrote it would
    /// fail rather than agree with an empty vector.
    #[test]
    fn it_rolls_the_asked_for_number_of_coins() {
        let mut f = field();
        f.core.coin_results = vec![true; 9];
        f.toss_coin(None, 0, 0, 3);
        run(&mut f);
        assert_eq!(f.core.coin_results.len(), 3);
        let (player, results) = coin_message(&f).expect("a toss message");
        assert_eq!(player, 0);
        assert_eq!(
            results, f.core.coin_results,
            "the message shows what was recorded"
        );
    }

    /// **The exact coins a known seed produces.**
    ///
    /// Derived independently — xoshiro256** plus the reference's rejection
    /// loop, worked through from `duel.cpp` rather than read back out of
    /// this crate — so it fails if the generator, the `[0, 1]` range, the
    /// `n <= lim` rejection, or the heads/tails constants drift. A
    /// self-consistency test catches none of those: both runs move
    /// together.
    ///
    /// `[1, 2, 3, 4]` is the seed the differential harness sets on both
    /// engines.
    #[test]
    fn a_known_seed_rolls_a_known_sequence() {
        let mut f =
            Field::with_flags_and_seed(8000, crate::duel::REFERENCE_CONFIGURATION, [1, 2, 3, 4]);
        f.infos.turn_id = 3;
        f.toss_coin(None, 0, 0, 24);
        run(&mut f);
        // Twenty-four, not eight. Eight is not enough to separate this
        // from a generator rolling over `[0, 3]` and testing `== 1`: that
        // mutant agrees on the first thirteen coins by chance and only
        // then diverges. Length here is the whole point.
        assert_eq!(
            f.core.coin_results,
            vec![
                false, false, false, false, false, true, false, true, false, false, false, false,
                false, true, false, false, false, true, false, true, true, false, false, false,
            ],
            "the reference's generator, range and rejection loop"
        );
        // Six heads in twenty-four looks biased and is not: it is this
        // seed's prefix. The generator runs 0.4985 heads over 100k draws.
    }

    /// **The same seed rolls the same coins.** Determinism is the property
    /// a differential run rests on, so it is asserted rather than assumed.
    #[test]
    fn the_same_seed_gives_the_same_coins() {
        let roll = || {
            let mut f = Field::with_flags_and_seed(
                8000,
                crate::duel::REFERENCE_CONFIGURATION,
                [1, 2, 3, 4],
            );
            f.infos.turn_id = 3;
            f.toss_coin(None, 0, 0, 8);
            run(&mut f);
            f.core.coin_results.clone()
        };
        let (a, b) = (roll(), roll());
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
        // Eight coins that all came out the same way would make every
        // other assertion here vacuous.
        assert!(
            a.iter().any(|&h| h) && a.iter().any(|&h| !h),
            "the generator produced both faces: {a:?}"
        );
    }

    /// **A different seed rolls different coins**, which is what makes the
    /// harness's seed alignment matter.
    #[test]
    fn a_different_seed_gives_different_coins() {
        let roll = |seed: [u64; 4]| {
            let mut f =
                Field::with_flags_and_seed(8000, crate::duel::REFERENCE_CONFIGURATION, seed);
            f.infos.turn_id = 3;
            f.toss_coin(None, 0, 0, 16);
            run(&mut f);
            f.core.coin_results.clone()
        };
        assert_ne!(
            roll([1, 2, 3, 4]),
            roll(Field::default_seed()),
            "the port's default seed and the harness's are not interchangeable"
        );
    }

    /// **The tally packs three numbers into one word**:
    /// `(tails << 16) | (heads << 8) | count`.
    ///
    /// Deliberately three *different* numbers — 4, 3 and 1 — so a swapped
    /// shift or a heads/tails mix-up cannot pass.
    #[test]
    fn the_event_value_packs_tails_heads_and_count() {
        let value = toss_event_value(&[true, true, true, false], 4);
        assert_eq!(value & 0xff, 4, "count");
        assert_eq!((value >> 8) & 0xff, 3, "heads");
        assert_eq!((value >> 16) & 0xff, 1, "tails");
        assert_eq!(value, (1 << 16) | (3 << 8) | 4);
    }

    /// Heads is `true`, and the count is what was *asked for* rather than
    /// what was rolled — a choosing effect can leave them different.
    #[test]
    fn the_tally_counts_heads_as_true_and_trusts_the_asked_for_count() {
        assert_eq!(toss_event_value(&[false, false], 2), (2 << 16) | 2);
        assert_eq!(toss_event_value(&[], 5), 5, "no coins, still five asked");
    }

    /// **A replacing effect ends the unit.** No roll, no message: the
    /// effect owns the whole toss.
    #[test]
    fn a_replacing_effect_takes_the_toss_over() {
        let mut f = field();
        toss_effect(&mut f, 0, code::TOSS_COIN_REPLACE);
        f.toss_coin(None, 0, 0, 3);
        assert_eq!(f.process(), Status::Continue);
        assert!(
            coin_message(&f).is_none(),
            "nothing was announced by this unit"
        );
        assert!(unit_step(&f).is_none(), "and the unit is finished");
        // Sized for the effect to write into — and **cleared first**.
        // Without the clear a stale vector is merely resized, so the
        // effect reads last toss's results as if they were this one's.
        assert_eq!(f.core.coin_results, vec![false, false, false]);
    }

    /// The choosing path clears the stale results too, for the same
    /// reason. Poisoned with a *longer* run of the opposite value, so a
    /// bare `resize` would leave them visible.
    #[test]
    fn a_choosing_effect_starts_from_a_cleared_vector() {
        let mut f = field();
        f.core.coin_results = vec![true; 6];
        toss_effect(&mut f, 0, code::TOSS_COIN_CHOOSE);
        f.toss_coin(None, 0, 0, 2);
        assert_eq!(f.process(), Status::Continue);
        assert_eq!(f.core.coin_results, vec![false, false]);
    }

    /// **A choosing effect jumps to case 3, not case 2.** `arg.step = 2`
    /// plus the processor's increment lands on 3; there is no case 2 at
    /// all. Case 3 then writes 0 and lands on case 1.
    #[test]
    fn a_choosing_effect_jumps_to_the_announcing_case() {
        let mut f = field();
        toss_effect(&mut f, 0, code::TOSS_COIN_CHOOSE);
        f.toss_coin(None, 0, 0, 2);
        assert_eq!(f.process(), Status::Continue);
        assert_eq!(
            unit_step(&f),
            Some(3),
            "case 3 runs next — `set_step(2)` plus the increment"
        );
        assert!(
            coin_message(&f).is_none(),
            "case 0 announced nothing; case 3 does that"
        );
    }

    /// **Case 3 announces only `count` results**, even when the vector
    /// holds more. Given four results and a count of three, the fourth is
    /// not shown — so the vector here is deliberately longer than the
    /// count, which an equal-length one could never demonstrate.
    #[test]
    fn the_announcing_case_shows_only_the_asked_for_count() {
        let mut f = field();
        f.core.coin_results = vec![true, false, true, true];
        f.emplace_at(
            Kind::TossCoin {
                reason_effect: None,
                reason_player: 0,
                playerid: 1,
                count: 3,
            },
            3,
        );
        assert_eq!(f.process(), Status::Continue);
        let (_, results) = coin_message(&f).expect("a toss message");
        assert_eq!(results, vec![true, false, true], "the fourth is not shown");
    }

    /// And case 3 announces whatever the effect wrote, then hands on to
    /// the tally.
    #[test]
    fn the_announcing_case_shows_the_chosen_results_then_tallies() {
        let mut f = field();
        f.core.coin_results = vec![true, false, true];
        f.emplace_at(
            Kind::TossCoin {
                reason_effect: None,
                reason_player: 0,
                playerid: 1,
                count: 3,
            },
            3,
        );
        assert_eq!(f.process(), Status::Continue);
        let (player, results) = coin_message(&f).expect("a toss message");
        assert_eq!(player, 1);
        assert_eq!(results, vec![true, false, true], "the chosen results");
        assert_eq!(unit_step(&f), Some(1), "case 1 runs next, not case 4");
    }
}
