//! Dark Hole — `c53129443.lua`.
//!
//! The first card to reach across the field: one `EFFECT_TYPE_ACTIVATE`
//! effect on `EVENT_FREE_CHAIN` whose `target` asks whether any monster
//! is on either side's Monster Zone (the `chk == 0` question), records the
//! set to be destroyed as the operation info, and whose `operation` reads
//! the field **again** — not the recorded set — and destroys what is
//! there now. That re-read is the script's, and it matters: a monster that
//! left between activation and resolution is not destroyed, one that
//! arrived is.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 53_129_443;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::DESTROY);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
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
    let sg = api::get_matching_group(f, Some(&api::always), tp, MZONE, MZONE, api::Except::None);
    let n = sg.len() as u8;
    api::set_operation_info(f, 0, category::DESTROY, Some(sg), n, 0, 0);
    api::yes(true)
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let sg = api::get_matching_group(f, Some(&api::always), tp, MZONE, MZONE, api::Except::None);
    api::destroy(f, sg, reason::EFFECT);
    Yield::Done(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
    use crate::duel::phases;
    use crate::field::Message;
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

    /// Player 0 in Main Phase 1 with Dark Hole in hand, a deck to draw
    /// from, and — when asked — monsters on the given seats of each side
    /// plus a Spell of player 0's in the row.
    fn field(mine: &[u32], theirs: &[u32]) -> (Field, CardId, Vec<CardId>, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        for p in 0..2u8 {
            for i in 0..3u32 {
                let id = card(&mut f, p, 5_053_103, card_type::MONSTER | card_type::NORMAL);
                f.add_card(p, id, location::DECK, i, false);
            }
        }
        let mut monsters = Vec::new();
        for (p, seats) in [(0u8, mine), (1u8, theirs)] {
            for &seat in seats {
                let id = card(&mut f, p, 5_053_103, card_type::MONSTER | card_type::NORMAL);
                f.add_card(p, id, location::MZONE, seat, false);
                monsters.push(id);
            }
        }
        let row = card(&mut f, 0, 4_444, card_type::SPELL | card_type::CONTINUOUS);
        f.add_card(0, row, location::SZONE, 3, false);
        let hole = card(&mut f, 0, CODE, card_type::SPELL);
        f.add_card(0, hole, location::HAND, 0, false);
        f.initialize_card(hole);
        (f, hole, monsters, row)
    }

    /// Run the idle command until the menu is asked, returning the
    /// activatable codes offered.
    fn offered(f: &mut Field) -> Vec<u32> {
        f.emplace(Kind::IdleCommand {
            state: Default::default(),
        });
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => {
                    if let Some(Message::SelectIdleCmd { activatable, .. }) = f.messages.last() {
                        return activatable.iter().map(|o| o.code).collect();
                    }
                    panic!("a different question: {:?}", f.messages.last());
                }
                other => panic!("stopped with {other:?}"),
            }
        }
        panic!("no menu")
    }

    /// **`initial_effect` registers one printed activate effect** with the
    /// destroy category and no player target — the script sets none.
    #[test]
    fn the_script_registers_a_printed_activate_effect() {
        let (f, hole, _, _) = field(&[], &[]);
        let ids: Vec<_> = f.cards[hole]
            .field_effect
            .equal_range(code::FREE_CHAIN)
            .to_vec();
        assert_eq!(ids.len(), 1, "one effect on EVENT_FREE_CHAIN");
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(
            e.is_flag(crate::effect::flag::INITIAL),
            "stamped while initializing"
        );
        assert!(!e.is_flag(crate::effect::flag::PLAYER_TARGET));
        assert_eq!(e.category, category::DESTROY);
        assert_eq!(e.handler, Some(hole));
    }

    /// **It is offered only when a monster is on either side**: `target`
    /// with `chk == 0` scans both Monster Zones for at least one card.
    #[test]
    fn it_is_offered_when_either_side_has_a_monster() {
        let (mut f, _, _, _) = field(&[], &[]);
        assert!(
            !offered(&mut f).contains(&CODE),
            "an empty field: not offered"
        );
        let (mut f, _, _, _) = field(&[], &[2]);
        assert_eq!(
            offered(&mut f),
            vec![CODE],
            "only the opponent's monster: offered"
        );
        let (mut f, _, _, _) = field(&[0], &[]);
        assert_eq!(offered(&mut f), vec![CODE], "only mine: offered");
    }

    /// **A monster mid-summon does not count** — the Monster Zone scan's
    /// own exclusion, reached through the seam.
    #[test]
    fn a_monster_mid_summon_does_not_make_it_activatable() {
        let (mut f, _, monsters, _) = field(&[], &[0]);
        f.cards[monsters[0]].status |= crate::card::status::SUMMONING;
        assert!(!offered(&mut f).contains(&CODE));
    }

    /// **Activating it destroys every monster on both sides** by effect,
    /// leaves the Spell row alone, and sends the card itself to the
    /// graveyard as a resolved Normal Spell. The operation info recorded
    /// at activation lists the whole set.
    #[test]
    fn activating_it_destroys_every_monster_on_both_sides() {
        let (mut f, hole, monsters, row) = field(&[0, 4], &[1]);
        f.emplace(Kind::IdleCommand {
            state: Default::default(),
        });
        let mut answered_menu = false;
        let mut opinfo = None;
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectIdleCmd { .. }) if !answered_menu => {
                        // kind 5 = activate, index 0
                        f.core.returns.set(5);
                        answered_menu = true;
                    }
                    Some(Message::SelectIdleCmd { .. }) => break,
                    Some(Message::SelectPlace { player, flag, .. }) => {
                        let (p, flag) = (*player, *flag);
                        let seq = (0..8u32)
                            .find(|s| flag & (1 << (s + 8)) == 0)
                            .expect("a free spell seat");
                        f.core.returns.set_i8(0, p as i8);
                        f.core.returns.set_i8(1, location::SZONE as i8);
                        f.core.returns.set_i8(2, seq as i8);
                    }
                    Some(Message::SelectChain { .. }) => {
                        // The link is on the chain here: read its recorded
                        // operation the first time we are asked.
                        if opinfo.is_none() {
                            if let Some(link) = f.core.current_chain.last() {
                                opinfo = Some(link.opinfos.clone());
                            }
                        }
                        f.core.returns.set(-1);
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                _ => break,
            }
        }
        assert!(answered_menu, "the menu was reached");
        for &m in &monsters {
            assert_eq!(f.cards[m].current.location, location::GRAVE, "destroyed");
            assert!(f.cards[m].reason & reason::DESTROY != 0);
            assert!(f.cards[m].reason & reason::EFFECT != 0, "by effect");
        }
        assert_eq!(
            f.cards[row].current.location,
            location::SZONE,
            "the Spell row is untouched"
        );
        assert_eq!(
            f.cards[hole].current.location,
            location::GRAVE,
            "a resolved Normal Spell"
        );
        let opinfo = opinfo.expect("the chain was asked about");
        let recorded = opinfo
            .iter()
            .find(|(cat, _)| **cat == category::DESTROY)
            .map(|(_, t)| t.clone())
            .expect("a DESTROY operation");
        let mut want = monsters.clone();
        want.sort_unstable();
        assert_eq!(recorded.cards, Some(want), "the set, in creation order");
        assert_eq!(recorded.count, 3);
    }
}
