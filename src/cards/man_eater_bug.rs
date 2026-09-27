//! Man-Eater Bug — `c54652250.lua`.
//!
//! The first **flip** effect: `EFFECT_TYPE_SINGLE + EFFECT_TYPE_FLIP`,
//! which fires when the monster is turned face-up rather than when it is
//! summoned or activated. Nothing in the pool had reached that path
//! before, because the play policy summoned every monster face-up and so
//! never produced a face-down to flip — see `processor-loop.md`, "The
//! play policy Sets monsters now".
//!
//! ## It needs no new API
//!
//! `Duel.SelectTarget`, `Duel.Hint`, `Duel.GetFirstTarget` and
//! `Card.IsRelateToEffect` all arrived with Mystical Space Typhoon. The
//! only thing this card adds is `Card.IsLocation`, for its `chkc`
//! branch. What it exercises that nothing else did is the **flip trigger
//! itself**.
//!
//! ## Its `chk == 0` is unconditional
//!
//! `return true`, with no check at all — where Mystical Space Typhoon
//! asks `IsExistingTarget` first. That is the script's, and it is not an
//! oversight: a flip effect has already happened by the time it is asked,
//! so there is nothing to refuse. If the field is empty the selection
//! simply finds nobody.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, CardId};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 54_652_250;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // flip
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::DESTROY);
    api::set_property(f, e1, flag::CARD_TARGET, 0);
    api::set_type(f, e1, effect_type::SINGLE | effect_type::FLIP);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, operation);
    api::register_effect(f, c, e1, false);
}

const MZONE: u32 = location::MZONE as u32;

fn target(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if let Some(chkc) = chkc {
        return api::yes(api::is_location(f, chkc, u16::from(location::MZONE)));
    }
    if !chk {
        return api::yes(true);
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::DESTROY);
    api::select_target(
        f,
        tp,
        Some(&api::always),
        tp,
        MZONE,
        MZONE,
        1,
        1,
        api::Except::None,
    );
    api::suspend(move |f, _| {
        if let Some(g) = api::selected_targets(f) {
            let n = g.len() as u8;
            api::set_operation_info(f, 0, category::DESTROY, Some(g), n, 0, 0);
        }
        api::yes(true)
    })
}

fn operation(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if api::is_relate_to_effect(f, tc, ctx.reason_effect) {
        api::destroy(f, vec![tc], reason::EFFECT);
    }
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::duel::phases;
    use crate::event::{code, Event};
    use crate::field::Message;
    use crate::position;
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

    fn vanilla(code_: u32, atk: i32) -> CardData {
        CardData {
            code: code_,
            type_: card_type::MONSTER | card_type::NORMAL,
            level: 4,
            attack: atk,
            defense: atk,
            ..Default::default()
        }
    }

    /// Player 1's Battle Step. Man-Eater Bug sits face-down in player 0's
    /// Monster Zone, and player 1 has `theirs` monsters, the first of
    /// which will attack it.
    fn field(theirs: usize) -> (Field, CardId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 4;
        f.infos.turn_player = 1;
        f.infos.phase = phases::BATTLE_STEP;
        let attackers = (0..theirs)
            .map(|i| {
                let m = put(
                    &mut f,
                    1,
                    vanilla(2000 + i as u32, 1800),
                    location::MZONE,
                    i as u32,
                );
                f.cards[m].current.position = position::FACEUP_ATTACK;
                m
            })
            .collect();
        let meb = put(
            &mut f,
            0,
            crate::cards::card_data(CODE).expect("printed data"),
            location::MZONE,
            0,
        );
        f.cards[meb].current.position = position::FACEDOWN_DEFENSE;
        f.initialize_card(meb);
        (f, meb, attackers)
    }

    fn effect_of(f: &Field, meb: CardId) -> crate::event::EffectId {
        f.cards[meb].single_effect.equal_range(code::FLIP)[0]
    }

    /// **One printed flip effect that targets.** `EFFECT_TYPE_FLIP` is
    /// what makes it fire on being turned face-up rather than on a
    /// summon.
    #[test]
    fn the_script_registers_a_flip_effect() {
        let (f, meb, _) = field(0);
        let ids: Vec<_> = f.cards[meb].single_effect.equal_range(code::FLIP).to_vec();
        assert_eq!(ids.len(), 1, "one flip effect");
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::SINGLE));
        assert!(e.is_type(effect_type::FLIP), "a flip, not a trigger");
        assert!(e.is_flag(flag::CARD_TARGET));
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert_eq!(e.category, category::DESTROY);
        assert_eq!(e.description, api::stringid(CODE, 0));
    }

    /// **The third question wants the Monster Zone**, and nothing else.
    #[test]
    fn the_third_question_wants_a_monster_zone() {
        let (mut f, meb, attackers) = field(1);
        let spell = put(
            &mut f,
            1,
            CardData {
                code: 9_999,
                type_: card_type::TRAP,
                ..Default::default()
            },
            location::SZONE,
            0,
        );
        let e = effect_of(&f, meb);
        let ev = Event::new(code::FLIP);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: Some(meb),
            args: &[],
        };
        let asks = |f: &mut Field, c: Option<CardId>| {
            target(f, &ctx, false, c).finished().unwrap_or(0) != 0
        };
        assert!(asks(&mut f, Some(attackers[0])), "a monster");
        assert!(!asks(&mut f, Some(spell)), "not a Spell/Trap row card");
    }

    /// **`chk == 0` is unconditional.** Unlike a targeting Spell, a flip
    /// effect has already happened by the time it is asked, so there is
    /// nothing to refuse — even on an empty field.
    #[test]
    fn the_activation_question_is_unconditional() {
        let (mut f, meb, _) = field(0);
        let e = effect_of(&f, meb);
        let ev = Event::new(code::FLIP);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: Some(meb),
            args: &[],
        };
        assert!(
            target(&mut f, &ctx, false, None).finished().unwrap_or(0) != 0,
            "yes, with nothing on the field at all"
        );
    }

    /// **Attacked face-down, it flips and destroys the chosen monster.**
    /// End to end: the attack turns it face-up, the flip effect fires,
    /// the player is asked, and the monster they name goes.
    #[test]
    fn being_attacked_flips_it_and_destroys_the_chosen_monster() {
        let (mut f, meb, attackers) = field(2);
        f.emplace(Kind::BattleCommand {
            state: Box::default(),
        });
        let mut attacked = false;
        let mut asked_of_zero = 0;
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectBattleCmd { attackable, .. }) if !attacked => {
                        assert!(!attackable.is_empty());
                        f.core.returns.set(1);
                        attacked = true;
                    }
                    Some(Message::SelectBattleCmd { .. }) => break,
                    Some(Message::SelectChain { chains, .. }) => {
                        f.core.returns.set(if chains.is_empty() { -1 } else { 0 });
                    }
                    Some(Message::SelectCard {
                        player, cards, min, ..
                    }) => {
                        if *player == 0 {
                            asked_of_zero += 1;
                        }
                        let min = usize::from(*min);
                        let _ = cards;
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, i as i32);
                        }
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                _ => break,
            }
        }
        assert!(attacked, "the attack was declared");
        assert!(
            asked_of_zero > 0,
            "the flip effect asked its controller to choose"
        );
        assert!(
            f.cards[meb].current.is_faceup(),
            "and the attack turned it face-up"
        );
        let gone = attackers
            .iter()
            .filter(|&&a| f.cards[a].current.location == location::GRAVE)
            .count();
        assert!(gone >= 1, "a monster of the attacker's was destroyed");
    }

    /// **Choosing records the operation.** Driven through an
    /// `ExecuteTarget`, so the selection and the recording both happen
    /// the way they do in a duel.
    #[test]
    fn choosing_records_the_destroy() {
        let (mut f, meb, attackers) = field(2);
        let e = effect_of(&f, meb);
        let mut ch = crate::chain::Chain::new(e, Event::new(code::FLIP));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 9;
        f.core.current_chain.push(ch);
        f.core.sub_solving_event.push_back(Event::new(code::FLIP));
        f.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: e,
            player: 0,
            subject: Some(meb),
            args: Vec::new(),
            was_disabled: false,
        });
        let mut offered = 0;
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() {
                        break;
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectCard { cards, .. }) => {
                        offered = cards.len();
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, 1);
                        f.core.returns.set_i32(2, 1); // the second offer
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        assert!(offered >= 2, "both sides' monsters are on offer: {offered}");
        let chosen = f.core.current_chain[0].target_cards.clone();
        assert_eq!(chosen.len(), 1, "exactly one named");
        let recorded = f.core.current_chain[0].opinfos.get(&category::DESTROY);
        assert_eq!(
            recorded.and_then(|t| t.cards.clone()),
            Some(chosen.clone()),
            "and the destroy records the one that was named"
        );
        assert_eq!(recorded.map(|t| t.count), Some(1));
        assert!(
            attackers.contains(&chosen[0]) || chosen[0] == meb,
            "from the monsters on the field"
        );
    }

    /// **A target that is no longer the one chosen is spared** — the
    /// re-read at resolution every targeting card does.
    #[test]
    fn a_target_that_lost_its_relation_is_spared() {
        let (mut f, meb, attackers) = field(1);
        let e = effect_of(&f, meb);
        let mut ch = crate::chain::Chain::new(e, Event::new(code::FLIP));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.target_cards = vec![attackers[0]];
        f.core.current_chain.push(ch);
        let ev = Event::new(code::FLIP);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: Some(meb),
            args: &[],
        };
        f.core.subunits.clear();
        operation(&mut f, &ctx);
        assert!(f.core.subunits.is_empty(), "no relation, nothing destroyed");
        f.cards[attackers[0]].relate_effect_insert_for_test(e);
        operation(&mut f, &ctx);
        assert_eq!(f.core.subunits.len(), 1, "and with one, it is");
    }
}
