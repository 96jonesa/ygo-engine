//! Torrential Tribute — `c53582587.lua`.
//!
//! The pool's first Trap. Three `EFFECT_TYPE_ACTIVATE` effects, one each
//! on `EVENT_SUMMON_SUCCESS`, `EVENT_FLIP_SUMMON_SUCCESS` and
//! `EVENT_SPSUMMON_SUCCESS`, sharing Dark Hole's `target` and an
//! `operation` that differs from it in one line: the script guards the
//! destroy with `#g > 0`, so an empty field at resolution queues nothing.
//!
//! Being a Trap, it is Set face-down and activated from the row in the
//! response window the summon opens — the set-turn rule (a Trap cannot be
//! activated the turn it was Set) and the trigger window are the
//! reference's, reached through the seam rather than written here.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 53_582_587;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate(summon)
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::DESTROY);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_code(f, e1, code::SUMMON_SUCCESS);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
    let e2 = api::create_effect(f, c);
    api::set_category(f, e2, category::DESTROY);
    api::set_type(f, e2, effect_type::ACTIVATE);
    api::set_code(f, e2, code::FLIP_SUMMON_SUCCESS);
    api::set_target(f, e2, target);
    api::set_operation(f, e2, activate);
    api::register_effect(f, c, e2, false);
    let e3 = api::create_effect(f, c);
    api::set_category(f, e3, category::DESTROY);
    api::set_type(f, e3, effect_type::ACTIVATE);
    api::set_code(f, e3, code::SPSUMMON_SUCCESS);
    api::set_target(f, e3, target);
    api::set_operation(f, e3, activate);
    api::register_effect(f, c, e3, false);
}

const MZONE: u32 = location::MZONE as u32;

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        return api::yes(api::is_existing_matching_card(
            f,
            Some(&api::always),
            tp,
            MZONE,
            MZONE,
            1,
            api::Except::None,
        ));
    }
    let g = api::get_matching_group(f, Some(&api::always), tp, MZONE, MZONE, api::Except::None);
    let n = g.len() as u8;
    api::set_operation_info(f, 0, category::DESTROY, Some(g), n, 0, 0);
    api::yes(true)
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let g = api::get_matching_group(f, Some(&api::always), tp, MZONE, MZONE, api::Except::None);
    if !g.is_empty() {
        api::destroy(f, g, reason::EFFECT);
    }
    Yield::Done(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::duel::phases;
    use crate::field::Message;
    use crate::position;
    use crate::processor::{Kind, Status};

    fn card(f: &mut Field, owner: u8, code: u32, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code,
                type_,
                level: 4,
                attack: 1700,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        let id = f.new_card(c);
        let fid = f.next_field_id_raw();
        f.cards[id].fieldid = fid;
        f.cards[id].fieldid_r = fid;
        id
    }

    /// Player 1 in its Main Phase 1 with a vanilla to summon; player 0
    /// with Torrential Tribute Set face-down in the row since an earlier
    /// turn, and a monster of its own on the given seats.
    fn field(mine: &[u32]) -> (Field, CardId, CardId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 4;
        f.infos.turn_player = 1;
        f.infos.phase = phases::MAIN1;
        for p in 0..2u8 {
            for i in 0..3u32 {
                let id = card(&mut f, p, 5_053_103, card_type::MONSTER | card_type::NORMAL);
                f.add_card(p, id, location::DECK, i, false);
            }
        }
        let vanilla = card(&mut f, 1, 5_053_103, card_type::MONSTER | card_type::NORMAL);
        f.add_card(1, vanilla, location::HAND, 0, false);
        let mut monsters = Vec::new();
        for &seat in mine {
            let id = card(&mut f, 0, 5_053_103, card_type::MONSTER | card_type::NORMAL);
            f.add_card(0, id, location::MZONE, seat, false);
            f.cards[id].current.position = position::FACEUP_ATTACK;
            monsters.push(id);
        }
        let tt = card(&mut f, 0, CODE, card_type::TRAP);
        f.add_card(0, tt, location::SZONE, 2, false);
        f.cards[tt].current.position = position::FACEDOWN;
        f.initialize_card(tt);
        (f, tt, vanilla, monsters)
    }

    /// Play player 1's Main Phase: summon the vanilla, place it, and in
    /// every response window activate the first thing offered. Returns
    /// what each `SelectChain` offered player 0.
    fn summon_and_respond(f: &mut Field) -> Vec<usize> {
        f.emplace(Kind::IdleCommand {
            state: Default::default(),
        });
        let mut answered_menu = false;
        let mut offered = Vec::new();
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectIdleCmd { .. }) if !answered_menu => {
                        // kind 0 = summon, index 0
                        f.core.returns.set(0);
                        answered_menu = true;
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
        assert!(answered_menu, "the menu was reached");
        offered
    }

    /// **`initial_effect` registers three printed activate effects**, one
    /// per summon event, all with the destroy category.
    #[test]
    fn the_script_registers_three_printed_activate_effects() {
        let (f, tt, _, _) = field(&[]);
        for ev in [
            code::SUMMON_SUCCESS,
            code::FLIP_SUMMON_SUCCESS,
            code::SPSUMMON_SUCCESS,
        ] {
            let ids: Vec<_> = f.cards[tt].field_effect.equal_range(ev).to_vec();
            assert_eq!(ids.len(), 1, "one effect on event {ev}");
            let e = f.effects.get(ids[0]).unwrap();
            assert!(e.is_type(effect_type::ACTIVATE));
            assert!(e.is_flag(crate::effect::flag::INITIAL));
            assert_eq!(e.category, category::DESTROY);
            assert_eq!(e.handler, Some(tt));
        }
        assert!(
            f.cards[tt]
                .field_effect
                .equal_range(code::FREE_CHAIN)
                .is_empty(),
            "nothing on FREE_CHAIN: it is not a free-chain card"
        );
    }

    /// **The opponent's summon opens a window that offers it**, and
    /// activating it destroys the summoned monster and the controller's
    /// own — both sides — by effect, with the Trap going to the graveyard.
    #[test]
    fn the_opponents_summon_triggers_it_and_it_destroys_both_sides() {
        let (mut f, tt, vanilla, mine) = field(&[0]);
        let offered = summon_and_respond(&mut f);
        assert!(
            offered.contains(&1),
            "player 0 was offered one activation: {offered:?}"
        );
        assert_eq!(
            f.cards[vanilla].current.location,
            location::GRAVE,
            "the summoned monster"
        );
        assert!(f.cards[vanilla].reason & reason::DESTROY != 0);
        assert!(f.cards[vanilla].reason & reason::EFFECT != 0);
        assert_eq!(
            f.cards[mine[0]].current.location,
            location::GRAVE,
            "and my own"
        );
        assert_eq!(
            f.cards[tt].current.location,
            location::GRAVE,
            "a resolved Normal Trap"
        );
    }

    /// **Set this turn, it is not offered** — the reference's set-turn
    /// rule, reached through the seam. The summon then stands.
    #[test]
    fn set_this_turn_it_is_not_offered() {
        let (mut f, tt, vanilla, _) = field(&[]);
        f.cards[tt].status |= status::SET_TURN;
        let offered = summon_and_respond(&mut f);
        assert!(
            offered.iter().all(|&n| n == 0),
            "nothing offered: {offered:?}"
        );
        assert_eq!(
            f.cards[vanilla].current.location,
            location::MZONE,
            "the summon stands"
        );
        assert_eq!(f.cards[tt].current.location, location::SZONE, "still Set");
    }

    /// **The opponent's summoned monster alone is enough** — the `chk == 0`
    /// question scans both sides, not just the controller's.
    #[test]
    fn only_the_opponents_summoned_monster_is_enough() {
        let (mut f, tt, vanilla, _) = field(&[]);
        let offered = summon_and_respond(&mut f);
        assert!(
            offered.contains(&1),
            "offered with nothing of my own: {offered:?}"
        );
        assert_eq!(f.cards[vanilla].current.location, location::GRAVE);
        assert_eq!(f.cards[tt].current.location, location::GRAVE);
    }

    /// **An empty field at resolution queues nothing** — the script's
    /// `#g > 0` guard. `Field::destroy` queues a `Destroy` unit even for
    /// an empty set (the reference does too), so the guard is what keeps
    /// the queue empty; with a monster there, one unit is queued.
    #[test]
    fn an_empty_field_at_resolution_queues_nothing() {
        let (mut f, tt, _, _) = field(&[]);
        let e1 = f.cards[tt].field_effect.equal_range(code::SUMMON_SUCCESS)[0];
        let ev = crate::event::Event::new(0);
        let ctx = Ctx {
            reason_effect: e1,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(f.core.subunits.is_empty());
        activate(&mut f, &ctx);
        assert!(
            f.core.subunits.is_empty(),
            "nothing to destroy, nothing queued"
        );
        let m = card(&mut f, 1, 5_053_103, card_type::MONSTER | card_type::NORMAL);
        f.add_card(1, m, location::MZONE, 0, false);
        activate(&mut f, &ctx);
        assert_eq!(f.core.subunits.len(), 1, "one destroy queued");
        assert!(matches!(f.core.subunits[0].kind, Kind::Destroy { .. }));
    }
}
