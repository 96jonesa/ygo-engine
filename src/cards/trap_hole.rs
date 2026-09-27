//! Trap Hole — `c4206964.lua`.
//!
//! The first card that **targets**, and the first to be written from one
//! effect and cloned into a second.
//!
//! ## What it targets is not chosen
//!
//! `EFFECT_FLAG_CARD_TARGET` is set, but nothing ever asks a player to
//! pick: the target is the monster that was just summoned, read off the
//! **event** (`eg:GetFirst()`). So the card targets without a selection,
//! which is why it can be written at all while the seam has no way for an
//! operation to wait for an answer.
//!
//! ## The third question
//!
//! `chkc` is the "would this card be a legal choice" call, and the script
//! answers it before either of the other two branches — `if chkc then
//! return chkc==tc end`. Nothing in the Goat pool reaches it (see
//! `processor-loop.md`, "The target's third question"), but it is
//! translated because it is what the script says.
//!
//! ## Asked twice, and the second time is not a repeat
//!
//! `chk == 0` demands the summon was the **opponent's** (`ep ~= tp`), the
//! monster face-up with 1000 attack or more, on the field, and targetable.
//! `activate` then checks face-up and the attack **again**, and adds
//! `IsRelateToEffect` — because a monster that left and returned, or was
//! flipped down, between activation and resolution is no longer the one
//! that was targeted. Note what is *not* re-checked: `ep ~= tp` and
//! `IsOnField`, which the reference leaves out too.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 4_206_964;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate(summon)
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::DESTROY);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_property(f, e1, flag::CARD_TARGET, 0);
    api::set_code(f, e1, code::SUMMON_SUCCESS);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
    let e2 = api::clone_effect(f, e1);
    api::set_code(f, e2, code::FLIP_SUMMON_SUCCESS);
    api::register_effect(f, c, e2, false);
}

/// `eg:GetFirst()` — the summoned monster the event carries.
fn summoned(ctx: &Ctx) -> Option<CardId> {
    ctx.event.event_cards.first().copied()
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    // `if not eg then return false end`
    let Some(tc) = summoned(ctx) else {
        return api::yes(false);
    };
    if let Some(chkc) = chkc {
        return api::yes(chkc == tc);
    }
    if !chk {
        return api::yes(
            ctx.event.event_player != ctx.player
                && api::is_faceup(f, tc)
                && api::get_attack(f, tc) >= 1000
                && api::is_on_field(f, tc)
                && api::is_can_be_effect_target(f, tc, ctx.reason_effect),
        );
    }
    api::set_target_card(f, ctx.event.event_cards.clone());
    api::set_operation_info(f, 0, category::DESTROY, Some(vec![tc]), 1, 0, 0);
    api::yes(true)
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(tc) = summoned(ctx) else {
        return Yield::Done(0);
    };
    if api::is_faceup(f, tc)
        && api::is_relate_to_effect(f, tc, ctx.reason_effect)
        && api::get_attack(f, tc) >= 1000
    {
        api::destroy(f, vec![tc], reason::EFFECT);
    }
    Yield::Done(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};
    use crate::duel::phases;
    use crate::field::Message;
    use crate::position;
    use crate::processor::{Kind, Status};

    /// `target`, asked as a plain yes/no. It answers through `returns`
    /// like any card function, so a test reads the finished value rather
    /// than the `Yield`.
    fn asks(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> bool {
        target(f, ctx, chk, chkc).finished().unwrap_or(0) != 0
    }

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

    fn monster(code_: u32, atk: i32) -> CardData {
        CardData {
            code: code_,
            type_: card_type::MONSTER | card_type::NORMAL,
            level: 4,
            attack: atk,
            defense: 1000,
            ..Default::default()
        }
    }

    /// Player 1's Main Phase with a monster of the given attack in hand to
    /// summon, and Trap Hole Set in player 0's row since an earlier turn.
    fn field(atk: i32) -> (Field, CardId, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 4;
        f.infos.turn_player = 1;
        f.infos.phase = phases::MAIN1;
        let summonable = put(&mut f, 1, monster(1000, atk), location::HAND, 0);
        let th = put(
            &mut f,
            0,
            crate::cards::card_data(CODE).expect("printed data"),
            location::SZONE,
            0,
        );
        f.cards[th].current.position = position::FACEDOWN;
        f.initialize_card(th);
        (f, th, summonable)
    }

    /// Summon player 1's monster and take every window offered. Returns
    /// how many chains player 0 was offered.
    fn summon_and_respond(f: &mut Field) -> Vec<usize> {
        f.emplace(Kind::IdleCommand {
            state: Default::default(),
        });
        let mut answered = false;
        let mut offered = Vec::new();
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectIdleCmd { .. }) if !answered => {
                        f.core.returns.set(0); // summon, index 0
                        answered = true;
                    }
                    Some(Message::SelectIdleCmd { .. }) => break,
                    Some(Message::SelectPlace { player, flag, .. }) => {
                        let (p, flag) = (*player, *flag);
                        let seq = (0..7u32)
                            .find(|s| flag & (1 << s) == 0)
                            .expect("a free monster seat");
                        f.core.returns.set_i8(0, p as i8);
                        f.core.returns.set_i8(1, location::MZONE as i8);
                        f.core.returns.set_i8(2, seq as i8);
                    }
                    Some(Message::SelectChain { player, chains, .. }) => {
                        if *player == 0 {
                            offered.push(chains.len());
                        }
                        f.core.returns.set(if chains.is_empty() { -1 } else { 0 });
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                _ => break,
            }
        }
        assert!(answered, "the summon was made");
        offered
    }

    /// **`initial_effect` registers two activate effects from one**, the
    /// second a clone differing only in its event. Both carry
    /// `EFFECT_FLAG_CARD_TARGET`, and the clone's handler is set by
    /// registering it — `Effect.Clone` clears it.
    #[test]
    fn the_script_clones_the_effect_for_the_flip_summon() {
        let (f, th, _) = field(1800);
        for ev in [code::SUMMON_SUCCESS, code::FLIP_SUMMON_SUCCESS] {
            let ids = f.cards[th].field_effect.equal_range(ev);
            assert_eq!(ids.len(), 1, "one effect on event {ev}");
            let e = f.effects.get(ids[0]).unwrap();
            assert!(e.is_type(effect_type::ACTIVATE));
            assert!(e.is_flag(flag::CARD_TARGET), "it targets");
            assert!(e.is_flag(crate::effect::flag::INITIAL));
            assert_eq!(e.category, category::DESTROY);
            assert_eq!(e.handler, Some(th), "the clone was registered too");
            assert!(e.target.is_some() && e.operation.is_some());
        }
    }

    /// **A summon of 1000 attack or more is caught**, the monster is
    /// named as the target, and it is destroyed by effect.
    #[test]
    fn a_big_enough_summon_is_destroyed() {
        let (mut f, th, m) = field(1800);
        let offered = summon_and_respond(&mut f);
        assert!(offered.contains(&1), "offered: {offered:?}");
        assert_eq!(f.cards[m].current.location, location::GRAVE);
        assert!(f.cards[m].reason & reason::DESTROY != 0);
        assert!(f.cards[m].reason & reason::EFFECT != 0);
        assert_eq!(
            f.cards[th].current.location,
            location::GRAVE,
            "a resolved Trap"
        );
        let announced: Vec<_> = f
            .messages
            .iter()
            .filter_map(|msg| match msg {
                Message::BecomeTarget { cards } => Some(cards.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(announced, vec![vec![m]], "the target was announced once");
    }

    /// **Under 1000 attack it is not offered.** The threshold is the
    /// whole of the card.
    #[test]
    fn a_small_summon_is_not_caught() {
        let (mut f, th, m) = field(900);
        let offered = summon_and_respond(&mut f);
        assert!(offered.iter().all(|&n| n == 0), "offered: {offered:?}");
        assert_eq!(f.cards[m].current.location, location::MZONE, "it stands");
        assert_eq!(f.cards[th].current.location, location::SZONE, "still Set");
    }

    /// **Its controller's own summon does not trigger it** — `ep ~= tp`.
    #[test]
    fn its_own_summon_is_not_caught() {
        let mut f = Field::new(8000);
        f.infos.turn_id = 4;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        let m = put(&mut f, 0, monster(1000, 1800), location::HAND, 0);
        let th = put(
            &mut f,
            0,
            crate::cards::card_data(CODE).expect("printed data"),
            location::SZONE,
            0,
        );
        f.cards[th].current.position = position::FACEDOWN;
        f.initialize_card(th);
        let offered = summon_and_respond(&mut f);
        assert!(offered.iter().all(|&n| n == 0), "offered: {offered:?}");
        assert_eq!(f.cards[m].current.location, location::MZONE);
    }

    /// **The third question answers about the summoned monster only.**
    /// No play reaches `chkc`, so it is asked directly — which is also
    /// the only way to reach the early return that precedes both other
    /// branches.
    #[test]
    fn the_third_question_names_the_summoned_monster() {
        let (mut f, th, m) = field(1800);
        let other = put(&mut f, 1, monster(2000, 1800), location::MZONE, 3);
        let e = f.cards[th].field_effect.equal_range(code::SUMMON_SUCCESS)[0];
        let mut ev = crate::event::Event::new(code::SUMMON_SUCCESS);
        ev.event_cards = vec![m];
        ev.event_player = 1;
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(asks(&mut f, &ctx, false, Some(m)), "the summoned monster");
        assert!(!asks(&mut f, &ctx, false, Some(other)), "anything else");
        // And it answers before the other branches: a board state the
        // `chk == 0` question would refuse still answers the third one.
        f.cards[m].current.position = position::FACEDOWN_DEFENSE;
        assert!(
            !asks(&mut f, &ctx, false, None),
            "face-down, so not activatable"
        );
        assert!(
            asks(&mut f, &ctx, false, Some(m)),
            "but still the card it would name"
        );
    }

    /// **Exactly 1000 attack is caught.** The comparison is `>=`, and
    /// only a monster sitting on the boundary can say which.
    #[test]
    fn exactly_one_thousand_is_caught() {
        let (mut f, _, m) = field(1000);
        let offered = summon_and_respond(&mut f);
        assert!(offered.contains(&1), "1000 is enough: {offered:?}");
        assert_eq!(f.cards[m].current.location, location::GRAVE);
    }

    /// **Resolution asks again, and can refuse.** Both re-checks matter:
    /// a monster that is no longer the one targeted, and one whose attack
    /// has since fallen below the threshold, are each left alone.
    #[test]
    fn resolution_refuses_a_changed_monster() {
        let (mut f, th, m) = field(1800);
        let e = f.cards[th].field_effect.equal_range(code::SUMMON_SUCCESS)[0];
        let mut ev = crate::event::Event::new(code::SUMMON_SUCCESS);
        ev.event_cards = vec![m];
        ev.event_player = 1;
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        // Stand it where the summon would have left it: on the field,
        // face-up, and related to the activation.
        f.add_card(1, m, location::MZONE, 0, false);
        f.cards[m].current.position = position::FACEUP_ATTACK;
        f.cards[m].relate_effect_insert_for_test(e);
        f.core.subunits.clear();
        activate(&mut f, &ctx);
        assert_eq!(
            f.core.subunits.len(),
            1,
            "related and big enough: destroyed"
        );

        // No longer the card that was targeted.
        f.cards[m].clear_relate_effect();
        f.core.subunits.clear();
        activate(&mut f, &ctx);
        assert!(f.core.subunits.is_empty(), "the relation is gone");

        // Related again, but its attack has fallen away.
        f.cards[m].relate_effect_insert_for_test(e);
        f.cards[m].data.attack = 900;
        f.core.subunits.clear();
        activate(&mut f, &ctx);
        assert!(f.core.subunits.is_empty(), "no longer big enough");
    }

    /// **With no event there is nothing to target.** `if not eg then
    /// return false end`, before anything dereferences it.
    #[test]
    fn an_empty_event_targets_nothing() {
        let (mut f, th, _) = field(1800);
        let e = f.cards[th].field_effect.equal_range(code::SUMMON_SUCCESS)[0];
        let ev = crate::event::Event::new(code::SUMMON_SUCCESS);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(!asks(&mut f, &ctx, false, None));
        assert!(!asks(&mut f, &ctx, true, None));
        assert!(!asks(&mut f, &ctx, false, Some(0)));
        assert_eq!(
            activate(&mut f, &ctx).finished(),
            Some(0),
            "and nothing to resolve"
        );
    }
}
