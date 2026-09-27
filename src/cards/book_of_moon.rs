//! Book of Moon — `c14087893.lua`.
//!
//! Turns one face-up monster face-down. The first card to **change a
//! position** rather than destroy, draw or banish, and the first with a
//! hint timing on **both** sides — it is a Quick-Play, so the script says
//! when its controller may be prompted and when the opponent may.
//!
//! ## `IsCanTurnSet` is asked for the reason player
//!
//! Not for the card's controller. `card::is_capable_turn_set(playerid)`
//! takes the player doing it, and the export passes
//! `core.reason_player` — so whether a monster can be turned face-down
//! depends on who is turning it. Reading the controller there would be a
//! plausible and wrong translation.
//!
//! ## The re-check is a different question
//!
//! `chk == 0` and `chkc` ask `IsCanTurnSet`; `activate` asks
//! `IsMonster` instead. That is the script's, and it is not
//! interchangeable: between activation and resolution a monster can stop
//! being one, and what the card may not do is turn a Spell or Trap
//! face-down. Whether it could still be *set* is no longer the question
//! once the target is fixed.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::{location, position};
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::{timing, Field};
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 14_087_893;

/// `TIMINGS_CHECK_MONSTER_E` (`constant.lua:902`).
const TIMINGS_CHECK_MONSTER_E: u32 = 0x1e0;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Change 1 face-up monster on the field to face-down Defense Position
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::POSITION | category::SET);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_property(f, e1, flag::CARD_TARGET, 0);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::set_hint_timing(
        f,
        e1,
        timing::END_PHASE,
        timing::STANDBY_PHASE
            | timing::MAIN_END
            | timing::BATTLE_START
            | timing::BATTLE_PHASE
            | TIMINGS_CHECK_MONSTER_E,
    );
    api::register_effect(f, c, e1, false);
}

const MZONE: u32 = location::MZONE as u32;

fn filter(f: &mut Field, c: CardId) -> bool {
    api::is_can_turn_set(f, c)
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if let Some(chkc) = chkc {
        return api::yes(
            api::is_location(f, chkc, u16::from(location::MZONE)) && api::is_can_turn_set(f, chkc),
        );
    }
    if !chk {
        return api::yes(api::is_existing_target(
            f,
            Some(&filter),
            tp,
            MZONE,
            MZONE,
            1,
            api::Except::None,
        ));
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::TARGET);
    api::select_target(
        f,
        tp,
        Some(&filter),
        tp,
        MZONE,
        MZONE,
        1,
        1,
        api::Except::None,
    );
    api::suspend(move |f, _| {
        if let Some(g) = api::selected_targets(f) {
            api::set_operation_info(
                f,
                0,
                category::POSITION,
                Some(g),
                1,
                tp,
                i32::from(position::FACEDOWN_DEFENSE),
            );
        }
        api::yes(true)
    })
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if api::is_relate_to_effect(f, tc, ctx.reason_effect) && api::is_monster(f, tc) {
        api::change_position(f, vec![tc], position::FACEDOWN_DEFENSE);
    }
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::Event;
    use crate::field::Message;
    use crate::processor::{Kind, Status};

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

    fn monster(f: &mut Field, owner: u8, seq: u32) -> CardId {
        let m = put(
            f,
            owner,
            CardData {
                code: 3_000 + seq + u32::from(owner) * 10,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1500,
                defense: 1000,
                ..Default::default()
            },
            location::MZONE,
            seq,
        );
        f.cards[m].current.position = position::FACEUP_ATTACK;
        m
    }

    /// Player 0 in Main Phase 1 with Book of Moon face-up in its row and
    /// a monster on each side.
    fn field() -> (Field, CardId, CardId, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        let bom = put(
            &mut f,
            0,
            crate::cards::card_data(CODE).expect("printed data"),
            location::SZONE,
            0,
        );
        f.cards[bom].current.position = position::FACEUP;
        f.initialize_card(bom);
        let mine = monster(&mut f, 0, 0);
        let theirs = monster(&mut f, 1, 0);
        (f, bom, mine, theirs)
    }

    fn effect_of(f: &Field, bom: CardId) -> crate::event::EffectId {
        f.cards[bom].field_effect.equal_range(code::FREE_CHAIN)[0]
    }

    fn asks(f: &mut Field, e: crate::event::EffectId, chk: bool, chkc: Option<CardId>) -> bool {
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        f.core.reason_effect = Some(e);
        f.core.reason_player = 0;
        let answer = target(f, &ctx, chk, chkc).finished().unwrap_or(0) != 0;
        f.core.reason_effect = None;
        answer
    }

    /// **One printed activate effect**, with both categories and hint
    /// timings on **both** sides — a Quick-Play says when its controller
    /// may be prompted and when the opponent may.
    #[test]
    fn the_script_registers_a_two_sided_hint_timing() {
        let (f, bom, _, _) = field();
        let ids = f.cards[bom].field_effect.equal_range(code::FREE_CHAIN);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(e.is_flag(flag::CARD_TARGET));
        assert_eq!(e.category, category::POSITION | category::SET);
        assert_eq!(
            e.hint_timing,
            [
                timing::END_PHASE,
                timing::STANDBY_PHASE
                    | timing::MAIN_END
                    | timing::BATTLE_START
                    | timing::BATTLE_PHASE
                    | TIMINGS_CHECK_MONSTER_E
            ],
            "its own side and the opponent's differ"
        );
    }

    /// **The third question wants a Monster Zone card that can be turned
    /// face-down.** Both clauses, each refused on its own.
    #[test]
    fn the_third_question_wants_a_turnable_monster() {
        let (mut f, bom, mine, theirs) = field();
        let e = effect_of(&f, bom);
        assert!(asks(&mut f, e, false, Some(mine)), "its own side counts");
        assert!(asks(&mut f, e, false, Some(theirs)), "and the opponent's");
        assert!(!asks(&mut f, e, false, Some(bom)), "not a Spell in the row");
        // A token cannot be turned face-down.
        f.cards[mine].data.type_ |= card_type::TOKEN;
        assert!(!asks(&mut f, e, false, Some(mine)), "and not a token");
    }

    /// **It is offered only while something can be turned.** With every
    /// monster a token there is nothing to aim at.
    #[test]
    fn it_is_offered_only_when_something_can_be_turned() {
        let (mut f, bom, mine, theirs) = field();
        let e = effect_of(&f, bom);
        assert!(asks(&mut f, e, false, None));
        for m in [mine, theirs] {
            f.cards[m].data.type_ |= card_type::TOKEN;
        }
        assert!(!asks(&mut f, e, false, None), "nothing can be turned");
    }

    /// **Choosing records a position change to face-down defence**, and
    /// resolving performs it.
    #[test]
    fn it_turns_the_chosen_monster_face_down() {
        let (mut f, bom, _, _) = field();
        let e = effect_of(&f, bom);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 3;
        f.core.current_chain.push(ch);
        f.core
            .sub_solving_event
            .push_back(Event::new(code::FREE_CHAIN));
        f.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: e,
            player: 0,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() {
                        break;
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectCard { .. }) => {
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, 1);
                        f.core.returns.set_i32(2, 0);
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        let chosen = f.core.current_chain[0].target_cards.clone();
        assert_eq!(chosen.len(), 1);
        let recorded = f.core.current_chain[0].opinfos.get(&category::POSITION);
        assert_eq!(
            recorded.map(|t| t.param),
            Some(i32::from(position::FACEDOWN_DEFENSE)),
            "the position it will put the card in"
        );

        let ev = Event::new(code::FREE_CHAIN);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        activate(&mut f, &ctx);
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
        assert_eq!(
            f.cards[chosen[0]].current.position,
            position::FACEDOWN_DEFENSE,
            "and it is face-down now"
        );
    }

    /// **The re-check at resolution is `IsMonster`, not
    /// `IsCanTurnSet`.** A target that stopped being a monster is left
    /// alone; one that merely stopped being *turnable* is not the
    /// question any more.
    #[test]
    fn resolution_asks_whether_it_is_still_a_monster() {
        let (mut f, bom, mine, _) = field();
        let e = effect_of(&f, bom);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.target_cards = vec![mine];
        f.core.current_chain.push(ch);
        f.cards[mine].relate_effect_insert_for_test(e);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        // No longer a monster: left alone.
        f.cards[mine].data.type_ = card_type::SPELL;
        f.core.subunits.clear();
        activate(&mut f, &ctx);
        assert!(f.core.subunits.is_empty(), "not a monster, so untouched");
        f.cards[mine].data.type_ = card_type::MONSTER | card_type::NORMAL;
        activate(&mut f, &ctx);
        assert_eq!(f.core.subunits.len(), 1, "and a monster is turned");
        // And the other half of the pair: still a monster, but no longer
        // the card that was targeted.
        f.cards[mine].clear_relate_effect();
        f.core.subunits.clear();
        activate(&mut f, &ctx);
        assert!(f.core.subunits.is_empty(), "no relation, so untouched");
    }

    /// **A monster already in defence is turned too.** The export
    /// defaults its three later position arguments to the first, so
    /// "face-down defence" means that whatever the card is now — and a
    /// target that is not in face-up *attack* is the only thing that can
    /// show the defaulting happened.
    #[test]
    fn a_monster_in_defence_is_turned_as_well() {
        let (mut f, bom, mine, _) = field();
        f.cards[mine].current.position = position::FACEUP_DEFENSE;
        let e = effect_of(&f, bom);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.target_cards = vec![mine];
        f.core.current_chain.push(ch);
        f.cards[mine].relate_effect_insert_for_test(e);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        activate(&mut f, &ctx);
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
        assert_eq!(
            f.cards[mine].current.position,
            position::FACEDOWN_DEFENSE,
            "from face-up defence, not only from face-up attack"
        );
    }
}
