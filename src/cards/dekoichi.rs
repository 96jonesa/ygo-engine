//! Dekoichi the Battlechanted Locomotive — `c87621407.lua`.
//!
//! A second flip effect, and the first card whose **draw count is
//! computed** rather than written down: one, plus one for each face-up
//! copy of card `8715625` its controller has on the field.
//!
//! ## The count is right even though it is always one here
//!
//! `8715625` is not in the Goat pool, so `ct` is zero in every duel this
//! port will play and the card draws exactly one. That is precisely why
//! the arithmetic is worth a test rather than a shrug: nothing in a
//! harness run can distinguish `ct + 1` from `1`, and a card written as
//! "draw one" would be identical on every trace and wrong the moment the
//! pool grew.
//!
//! ## What is asked when
//!
//! `chk == 0` is unconditional, as a flip effect's is — it has already
//! happened. `chk == 1` counts, records the player and the count, and
//! files the operation. The operation reads both back off the chain and
//! draws, which is Pot of Greed's shape exactly.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, CardId};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 87_621_407;

/// `s.listed_names` — the card each face-up copy of which adds a draw.
const LISTED: u32 = 8_715_625;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // flip
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::DRAW);
    api::set_type(f, e1, effect_type::SINGLE | effect_type::FLIP);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, operation);
    api::register_effect(f, c, e1, false);
}

fn filter(f: &mut Field, c: CardId) -> bool {
    api::is_faceup(f, c) && api::is_code(f, c, LISTED)
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        return api::yes(true);
    }
    // Its **own** side only: the second mask is zero.
    let ct = api::get_matching_group_count(
        f,
        Some(&filter),
        tp,
        u32::from(location::ONFIELD),
        0,
        api::Except::None,
    );
    let draws = ct as i32 + 1;
    api::set_target_player(f, tp);
    api::set_target_param(f, draws);
    api::set_operation_info(f, 0, category::DRAW, None, 0, tp, draws);
    api::yes(true)
}

fn operation(f: &mut Field, _ctx: &Ctx) -> Yield {
    let (p, d) = api::get_chain_target_player_param(f, 0);
    api::draw(f, p, d as u32, reason::EFFECT);
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::{code, Event};
    use crate::field::Message;
    use crate::position;
    use crate::processor::Status;

    fn put(f: &mut Field, owner: u8, data: CardData, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(data, owner);
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        let fid = f.next_field_id_raw();
        f.cards[id].fieldid = fid;
        f.cards[id].fieldid_r = fid;
        id
    }

    /// Player 0 with Dekoichi face-down and a deck to draw from.
    fn field() -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 4;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        for i in 0..8u32 {
            put(
                &mut f,
                0,
                CardData {
                    code: 5_053_103,
                    type_: card_type::MONSTER,
                    ..Default::default()
                },
                location::DECK,
                i,
            );
        }
        let dek = put(
            &mut f,
            0,
            crate::cards::card_data(CODE).expect("printed data"),
            location::MZONE,
            0,
        );
        f.cards[dek].current.position = position::FACEDOWN_DEFENSE;
        f.initialize_card(dek);
        (f, dek)
    }

    fn effect_of(f: &Field, dek: CardId) -> crate::event::EffectId {
        f.cards[dek].single_effect.equal_range(code::FLIP)[0]
    }

    /// Run `target(chk = 1)` with a chain link, and report what it
    /// recorded.
    fn records(f: &mut Field, dek: CardId) -> (u8, i32) {
        let e = effect_of(f, dek);
        let mut ch = Chain::new(e, Event::new(code::FLIP));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        f.core.current_chain.push(ch);
        let ev = Event::new(code::FLIP);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: Some(dek),
            args: &[],
        };
        assert!(target(f, &ctx, true, None).finished().unwrap_or(0) != 0);
        api::get_chain_target_player_param(f, 0)
    }

    /// **One printed flip effect that draws.**
    #[test]
    fn the_script_registers_a_flip_effect() {
        let (f, dek) = field();
        let ids = f.cards[dek].single_effect.equal_range(code::FLIP);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::SINGLE) && e.is_type(effect_type::FLIP));
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert_eq!(e.category, category::DRAW);
        assert_eq!(e.description, api::stringid(CODE, 0));
    }

    /// **`chk == 0` is unconditional**, as a flip effect's is: it has
    /// already happened by the time it is asked.
    #[test]
    fn the_activation_question_is_unconditional() {
        let (mut f, dek) = field();
        let e = effect_of(&f, dek);
        let ev = Event::new(code::FLIP);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: Some(dek),
            args: &[],
        };
        assert!(target(&mut f, &ctx, false, None).finished().unwrap_or(0) != 0);
    }

    /// **One draw with none of the listed card on the field**, which is
    /// every duel in the Goat pool.
    #[test]
    fn it_draws_one_by_default() {
        let (mut f, dek) = field();
        assert_eq!(records(&mut f, dek), (0, 1));
        let recorded = f.core.current_chain[0].opinfos.get(&category::DRAW);
        assert_eq!(recorded.map(|t| t.param), Some(1));
        assert_eq!(recorded.map(|t| t.player), Some(0));
    }

    /// **Each face-up copy of the listed card adds one.** No duel this
    /// port plays contains that card, so nothing in a harness run can
    /// tell `ct + 1` from `1` — which is exactly why the arithmetic is
    /// tested here rather than trusted.
    #[test]
    fn each_face_up_listed_card_adds_a_draw() {
        for (copies, want) in [(0u32, 1i32), (1, 2), (3, 4)] {
            let (mut f, dek) = field();
            for i in 0..copies {
                let c = put(
                    &mut f,
                    0,
                    CardData {
                        code: LISTED,
                        type_: card_type::MONSTER,
                        ..Default::default()
                    },
                    location::MZONE,
                    i + 1,
                );
                f.cards[c].current.position = position::FACEUP_ATTACK;
            }
            assert_eq!(records(&mut f, dek), (0, want), "{copies} copies");
        }
    }

    /// **Face-down copies do not count, and neither does the
    /// opponent's.** The filter asks `IsFaceup`, and the scan passes zero
    /// as the opponent's mask.
    #[test]
    fn face_down_and_the_opponents_copies_do_not_count() {
        let (mut f, dek) = field();
        // A face-up monster that is *not* the listed card. Without it,
        // dropping the code check from the filter changes nothing — the
        // only other monster is Dekoichi, and it is face-down.
        let other = put(
            &mut f,
            0,
            CardData {
                code: 4_242,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            location::MZONE,
            2,
        );
        f.cards[other].current.position = position::FACEUP_ATTACK;
        let hidden = put(
            &mut f,
            0,
            CardData {
                code: LISTED,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            location::MZONE,
            1,
        );
        f.cards[hidden].current.position = position::FACEDOWN_DEFENSE;
        let theirs = put(
            &mut f,
            1,
            CardData {
                code: LISTED,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            location::MZONE,
            0,
        );
        f.cards[theirs].current.position = position::FACEUP_ATTACK;
        assert_eq!(
            records(&mut f, dek),
            (0, 1),
            "neither the face-down one, nor the opponent's, nor another card"
        );
    }

    /// **Flipping it draws.** End to end: the operation reads the player
    /// and count back off the chain, as Pot of Greed's does.
    #[test]
    fn resolving_it_draws_what_was_recorded() {
        let (mut f, dek) = field();
        let deck_before = f.players[0].main.len();
        records(&mut f, dek);
        let e = effect_of(&f, dek);
        let ev = Event::new(code::FLIP);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: Some(dek),
            args: &[],
        };
        operation(&mut f, &ctx);
        for _ in 0..512 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        break;
                    }
                }
                other => panic!("stopped with {other:?}"),
            }
        }
        assert_eq!(f.players[0].main.len(), deck_before - 1, "one drawn");
        let drew: usize = f
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::Draw { player: 0, codes } => Some(codes.len()),
                _ => None,
            })
            .sum();
        assert_eq!(drew, 1);
    }
}
