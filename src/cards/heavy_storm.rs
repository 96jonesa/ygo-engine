//! Heavy Storm — `c19613556.lua`.
//!
//! Dark Hole for the Spell and Trap rows, with two things Dark Hole did
//! not need: a filter that is a real predicate (`Card.IsSpellTrap`, which
//! is `Card.IsType(TYPE_SPELL|TYPE_TRAP)` — the *effective* type, so a
//! monster treated as a Spell counts), and an exception — the card
//! itself, by `e:GetHandler()`, since at resolution it sits in the row it
//! is clearing. Face-down cards are in the group: the row scan does not
//! look at position.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 19_613_556;

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

fn filter(f: &mut Field, c: CardId) -> bool {
    api::is_spell_trap(f, c)
}

const ONFIELD: u32 = location::ONFIELD as u32;

/// The exception: the handler, or nothing if the effect has none.
fn except(f: &Field, ctx: &Ctx) -> api::Except<'static> {
    api::get_handler(f, ctx.reason_effect).map_or(api::Except::None, api::Except::Card)
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    let c = except(f, ctx);
    if !chk {
        return api::yes(api::is_existing_matching_card(
            f,
            Some(&filter),
            tp,
            ONFIELD,
            ONFIELD,
            1,
            c,
        ));
    }
    let sg = api::get_matching_group(f, Some(&filter), tp, ONFIELD, ONFIELD, c);
    let n = sg.len() as u8;
    api::set_operation_info(f, 0, category::DESTROY, Some(sg), n, 0, 0);
    api::yes(true)
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let c = except(f, ctx);
    let sg = api::get_matching_group(f, Some(&filter), tp, ONFIELD, ONFIELD, c);
    api::destroy(f, sg, reason::EFFECT);
    Yield::Done(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
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

    /// Player 0 in Main Phase 1 with Heavy Storm in hand; a face-down
    /// Trap of player 0's and a face-up Spell of player 1's in the rows
    /// when asked for (`mine`, `theirs`), and a monster of player 1's
    /// that must survive.
    fn field((mine, theirs): (bool, bool)) -> (Field, CardId, Vec<CardId>, CardId) {
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
        let mut rows = Vec::new();
        if mine {
            let trap = card(&mut f, 0, 4_444, card_type::TRAP);
            f.add_card(0, trap, location::SZONE, 1, false);
            f.cards[trap].current.position = position::FACEDOWN;
            rows.push(trap);
        }
        if theirs {
            let spell = card(&mut f, 1, 5_555, card_type::SPELL | card_type::CONTINUOUS);
            f.add_card(1, spell, location::SZONE, 4, false);
            f.cards[spell].current.position = position::FACEUP;
            rows.push(spell);
        }
        let monster = card(&mut f, 1, 5_053_103, card_type::MONSTER | card_type::NORMAL);
        f.add_card(1, monster, location::MZONE, 0, false);
        f.cards[monster].current.position = position::FACEUP_ATTACK;
        let storm = card(&mut f, 0, CODE, card_type::SPELL);
        f.add_card(0, storm, location::HAND, 0, false);
        f.initialize_card(storm);
        (f, storm, rows, monster)
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

    /// **`initial_effect` registers one printed activate effect** on
    /// `EVENT_FREE_CHAIN` with the destroy category and no player target.
    #[test]
    fn the_script_registers_a_printed_activate_effect() {
        let (f, storm, _, _) = field((false, false));
        let ids: Vec<_> = f.cards[storm]
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
        assert_eq!(e.handler, Some(storm));
    }

    /// **It is offered only when a Spell or Trap is on either side's row**
    /// — a monster alone is not enough, and the opponent's row card alone
    /// is.
    #[test]
    fn it_is_offered_when_a_spell_or_trap_is_in_either_row() {
        let (mut f, _, _, _) = field((false, false));
        assert!(
            !offered(&mut f).contains(&CODE),
            "a monster alone: not offered"
        );
        let (mut f, _, _, _) = field((false, true));
        assert_eq!(offered(&mut f), vec![CODE], "only the opponent's: offered");
        let (mut f, _, _, _) = field((true, false));
        assert_eq!(offered(&mut f), vec![CODE], "only mine: offered");
    }

    /// **The filter is the effective type**: a monster treated as a
    /// Spell/Trap by `get_type` counts (the `temp.type_` scratch, read
    /// through `Card.IsType`), which is why the filter cannot be a plain
    /// look at the printed type.
    #[test]
    fn the_filter_reads_the_effective_type() {
        let (mut f, _, _, monster) = field((false, false));
        assert!(!filter(&mut f, monster));
        f.cards[monster]
            .assume
            .insert(crate::card::assume::TYPE, u64::from(card_type::TRAP));
        assert!(filter(&mut f, monster), "assumed a Trap, it passes");
    }

    /// **Activating it destroys every Spell and Trap in both rows,
    /// face-down included, except itself** — which ends in the graveyard
    /// as a resolved Normal Spell, not as a destroyed one — and the
    /// monster stands. The operation info records the two cards.
    #[test]
    fn activating_it_clears_both_rows_but_not_itself() {
        let (mut f, storm, rows, monster) = field((true, true));
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
        for &r in &rows {
            assert_eq!(f.cards[r].current.location, location::GRAVE, "destroyed");
            assert!(f.cards[r].reason & reason::DESTROY != 0);
            assert!(f.cards[r].reason & reason::EFFECT != 0);
        }
        assert_eq!(
            f.cards[monster].current.location,
            location::MZONE,
            "the monster stands"
        );
        assert_eq!(
            f.cards[storm].current.location,
            location::GRAVE,
            "a resolved Normal Spell"
        );
        assert!(
            f.cards[storm].reason & reason::DESTROY == 0,
            "not destroyed by its own effect"
        );
        let opinfo = opinfo.expect("the chain was asked about");
        let recorded = opinfo
            .iter()
            .find(|(cat, _)| **cat == category::DESTROY)
            .map(|(_, t)| t.clone())
            .expect("a DESTROY operation");
        let mut want = rows.clone();
        want.sort_unstable();
        assert_eq!(recorded.cards, Some(want), "the two row cards, not itself");
        assert_eq!(recorded.count, 2);
    }
}
