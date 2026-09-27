//! Pot of Greed — `c55144522.lua`.
//!
//! The simplest activation in the pool, and the first card through the
//! seam: one `EFFECT_TYPE_ACTIVATE` effect on `EVENT_FREE_CHAIN` with a
//! player target, whose `target` checks the player can draw two and
//! records the operation, and whose `operation` reads the recorded player
//! and count back off the chain and draws.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 55_144_522;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::DRAW);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_property(f, e1, flag::PLAYER_TARGET, 0);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        return api::yes(api::is_player_can_draw(f, tp, 2));
    }
    api::set_target_player(f, tp);
    api::set_target_param(f, 2);
    api::set_operation_info(f, 0, category::DRAW, None, 0, tp, 2);
    api::yes(true)
}

fn activate(f: &mut Field, _ctx: &Ctx) -> Yield {
    let (p, d) = api::get_chain_target_player_param(f, 0);
    api::draw(f, p, d as u32, reason::EFFECT);
    Yield::Done(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::location;
    use crate::card::{Card, CardData};
    use crate::duel::phases;
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    /// Player 0 in Main Phase 1 with Pot of Greed in hand and a deck to
    /// draw from.
    fn field() -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        for p in 0..2u8 {
            for i in 0..5u32 {
                let mut c = Card::with_data(
                    CardData {
                        code: 5_053_103,
                        type_: crate::card::card_type::MONSTER | crate::card::card_type::NORMAL,
                        level: 4,
                        attack: 1700,
                        defense: 1000,
                        ..Default::default()
                    },
                    p,
                );
                c.current.controller = p;
                let id = f.new_card(c);
                f.add_card(p, id, location::DECK, i, false);
                let fid = f.next_field_id_raw();
                f.cards[id].fieldid = fid;
                f.cards[id].fieldid_r = fid;
            }
        }
        let mut c = Card::with_data(
            CardData {
                code: CODE,
                type_: crate::card::card_type::SPELL,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        let pot = f.new_card(c);
        f.add_card(0, pot, location::HAND, 0, false);
        let fid = f.next_field_id_raw();
        f.cards[pot].fieldid = fid;
        f.cards[pot].fieldid_r = fid;
        f.initialize_card(pot);
        (f, pot)
    }

    /// **`initial_effect` registers one activate effect, flagged as
    /// printed.** `INITIAL` is stamped because the card was
    /// `STATUS_INITIALIZING` while its script ran — which is what tells a
    /// printed effect from a granted one later.
    #[test]
    fn the_script_registers_a_printed_activate_effect() {
        let (f, pot) = field();
        let ids: Vec<_> = f.cards[pot]
            .field_effect
            .equal_range(code::FREE_CHAIN)
            .to_vec();
        assert_eq!(ids.len(), 1, "one effect on EVENT_FREE_CHAIN");
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(
            e.is_type(effect_type::FIELD),
            "SetType folds ACTIVATE into FIELD"
        );
        assert!(e.is_flag(flag::INITIAL), "stamped while initializing");
        assert!(e.is_flag(flag::PLAYER_TARGET));
        assert_eq!(e.category, category::DRAW);
        assert_eq!(e.handler, Some(pot));
        assert!(
            !f.cards[pot].get_status(crate::card::status::INITIALIZING),
            "and the status is cleared afterwards"
        );
    }

    /// **The Main Phase menu offers it.** The registered effect is in the
    /// field's index (hand is in an activate effect's range), so the idle
    /// gather finds it and `target(chk = 0)` says the player can draw two.
    #[test]
    fn the_main_phase_menu_offers_the_activation() {
        let (mut f, pot) = field();
        f.emplace(Kind::IdleCommand {
            state: Default::default(),
        });
        let mut offered = None;
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => {
                    if let Some(Message::SelectIdleCmd { activatable, .. }) = f.messages.last() {
                        offered = Some(activatable.iter().map(|o| o.code).collect::<Vec<_>>());
                    }
                    break;
                }
                other => panic!("stopped with {other:?}"),
            }
        }
        assert_eq!(
            offered,
            Some(vec![CODE]),
            "Pot of Greed is the one activatable card"
        );
        assert_eq!(
            f.cards[pot].current.location,
            location::HAND,
            "still in hand at the ask"
        );
    }

    /// **Activating it draws two.** End to end through the seam: answer
    /// the menu with "activate 0", place it, decline the response windows,
    /// and the operation reads the target player and count back off the
    /// chain and draws.
    #[test]
    fn activating_it_draws_two() {
        let (mut f, pot) = field();
        let hand_before = f.players[0].hand.len();
        let deck_before = f.players[0].main.len();
        f.emplace(Kind::IdleCommand {
            state: Default::default(),
        });
        let mut answered_menu = false;
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
                    Some(Message::SelectChain { .. }) => f.core.returns.set(-1),
                    other => panic!("unexpected question: {other:?}"),
                },
                _ => break,
            }
        }
        assert!(answered_menu, "the menu was reached");
        assert_eq!(
            f.players[0].main.len(),
            deck_before - 2,
            "two fewer in the deck"
        );
        // The pot itself left the hand (to the Spell row, then the
        // graveyard as a resolved Normal Spell); the hand gained two.
        assert_eq!(f.players[0].hand.len(), hand_before - 1 + 2);
        assert_eq!(
            f.cards[pot].current.location,
            location::GRAVE,
            "a resolved Normal Spell"
        );
        let drew = f
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::Draw { player: 0, codes } => Some(codes.len()),
                _ => None,
            })
            .sum::<usize>();
        assert_eq!(drew, 2);
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;
    use crate::board::location;
    use crate::card::{Card, CardData};
    use crate::duel::phases;
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    /// **With one card in the deck it is not offered**: `target(chk = 0)`
    /// asks whether the player can draw two.
    #[test]
    fn it_is_not_offered_when_the_deck_cannot_give_two() {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(
            CardData {
                code: 5_053_103,
                type_: crate::card::card_type::MONSTER,
                level: 4,
                ..Default::default()
            },
            0,
        );
        d.current.controller = 0;
        let d = f.new_card(d);
        f.add_card(0, d, location::DECK, 0, false);
        let mut c = Card::with_data(
            CardData {
                code: CODE,
                type_: crate::card::card_type::SPELL,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        let pot = f.new_card(c);
        f.add_card(0, pot, location::HAND, 0, false);
        f.initialize_card(pot);
        f.emplace(Kind::IdleCommand {
            state: Default::default(),
        });
        let mut offered = None;
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => {
                    if let Some(Message::SelectIdleCmd { activatable, .. }) = f.messages.last() {
                        offered = Some(activatable.len());
                    }
                    break;
                }
                other => panic!("stopped with {other:?}"),
            }
        }
        assert_eq!(offered, Some(0), "one card in the deck: not offered");
    }
}
