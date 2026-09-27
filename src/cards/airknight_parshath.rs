//! Airknight Parshath — `c18036057.lua`.
//!
//! The first **monster** through the seam, and with it three firsts: a
//! trigger effect, an effect that reads the *event* rather than the
//! board, and a card carrying two effects of different kinds at once.
//!
//! The draw is `EFFECT_TYPE_SINGLE + EFFECT_TYPE_TRIGGER_F` on
//! `EVENT_BATTLE_DAMAGE`. `TRIGGER_F` is the **forced** trigger: it goes
//! on the chain by itself, without being offered, which is why nothing in
//! the harness has to choose to use it. The condition is the whole of
//! "when it inflicts battle damage **to the opponent**": `ep ~= tp`, the
//! event's player against the effect's, since the same event is raised
//! for damage in either direction.
//!
//! The pierce is a bare `EFFECT_PIERCE` with no value, no condition and
//! no operation — a continuous property the battle code reads, not
//! something that ever resolves.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 18_036_057;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // draw
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::DRAW);
    api::set_property(f, e1, flag::PLAYER_TARGET, 0);
    api::set_type(f, e1, effect_type::SINGLE | effect_type::TRIGGER_F);
    api::set_code(f, e1, code::BATTLE_DAMAGE);
    api::set_condition(f, e1, condition);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, operation);
    api::register_effect(f, c, e1, false);
    // pierce
    let e2 = api::create_effect(f, c);
    api::set_type(f, e2, effect_type::SINGLE);
    api::set_code(f, e2, code::PIERCE);
    api::register_effect(f, c, e2, false);
}

fn condition(_f: &mut Field, ctx: &Ctx) -> bool {
    ctx.event.event_player != ctx.player
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        return api::yes(true);
    }
    api::set_target_player(f, tp);
    api::set_target_param(f, 1);
    api::set_operation_info(f, 0, category::DRAW, None, 0, tp, 1);
    api::yes(true)
}

fn operation(f: &mut Field, _ctx: &Ctx) -> Yield {
    let (p, d) = api::get_chain_target_player_param(f, 0);
    api::draw(f, p, d as u32, reason::EFFECT);
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

    /// Player 0's Battle Step with Airknight on the field and a deck to
    /// draw from; `theirs` is what player 1 has to be attacked, if
    /// anything.
    fn field(theirs: Option<i32>) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 4;
        f.infos.turn_player = 0;
        f.infos.phase = phases::BATTLE_STEP;
        for p in 0..2u8 {
            for i in 0..3u32 {
                put(&mut f, p, vanilla(5_053_103, 1000), location::DECK, i);
            }
        }
        let ak = put(
            &mut f,
            0,
            crate::cards::card_data(CODE).expect("printed data"),
            location::MZONE,
            0,
        );
        f.cards[ak].current.position = position::FACEUP_ATTACK;
        f.initialize_card(ak);
        if let Some(atk) = theirs {
            let t = put(&mut f, 1, vanilla(2000, atk), location::MZONE, 0);
            f.cards[t].current.position = position::FACEUP_ATTACK;
        }
        (f, ak)
    }

    fn draw_effect(f: &Field, ak: CardId) -> crate::event::EffectId {
        f.cards[ak].single_effect.equal_range(code::BATTLE_DAMAGE)[0]
    }

    /// **`initial_effect` registers the draw trigger and the pierce.** The
    /// draw is a *forced* trigger on battle damage with a player target
    /// and the draw category; the pierce is a bare continuous property
    /// with nothing attached to it at all.
    #[test]
    fn the_script_registers_a_forced_trigger_and_a_pierce() {
        let (f, ak) = field(None);
        let draw = f.cards[ak].single_effect.equal_range(code::BATTLE_DAMAGE);
        assert_eq!(draw.len(), 1);
        let e = f.effects.get(draw[0]).unwrap();
        assert!(e.is_type(effect_type::SINGLE));
        assert!(e.is_type(effect_type::TRIGGER_F), "forced, not optional");
        assert!(
            !e.is_type(effect_type::TRIGGER_O),
            "it is never offered as a choice"
        );
        assert!(e.is_flag(flag::PLAYER_TARGET));
        assert!(e.is_flag(flag::INITIAL));
        assert_eq!(e.category, category::DRAW);
        assert_eq!(e.description, api::stringid(CODE, 0));
        assert!(e.condition.is_some() && e.target.is_some() && e.operation.is_some());

        let pierce = f.cards[ak].single_effect.equal_range(code::PIERCE);
        assert_eq!(pierce.len(), 1);
        let p = f.effects.get(pierce[0]).unwrap();
        assert!(p.is_type(effect_type::SINGLE));
        assert!(
            !p.is_type(effect_type::TRIGGER_F) && !p.is_type(effect_type::TRIGGER_O),
            "a property, not a trigger"
        );
        assert!(
            p.condition.is_none() && p.target.is_none() && p.operation.is_none(),
            "nothing is attached to it: the battle code just reads it"
        );
    }

    /// **The condition is whose life points took the damage.** The same
    /// `EVENT_BATTLE_DAMAGE` is raised for damage in either direction, so
    /// `ep ~= tp` is what makes this "to the opponent".
    #[test]
    fn the_condition_wants_the_damage_to_be_the_opponents() {
        let (mut f, ak) = field(None);
        let e = draw_effect(&f, ak);
        for (event_player, want) in [(1u8, true), (0, false)] {
            let mut ev = crate::event::Event::new(code::BATTLE_DAMAGE);
            ev.event_player = event_player;
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: Some(ak),
                args: &[],
            };
            assert_eq!(
                condition(&mut f, &ctx),
                want,
                "damage to player {event_player}"
            );
        }
    }

    /// **A direct attack draws one card for its controller.** End to end:
    /// the attack goes through, the damage raises the event, the forced
    /// trigger goes on the chain by itself and the operation reads the
    /// player and count back off it.
    #[test]
    fn battle_damage_to_the_opponent_draws_one() {
        let (mut f, _ak) = field(None);
        let hand_before = f.players[0].hand.len();
        let deck_before = f.players[0].main.len();
        let lp_before = f.players[1].lp;
        run_battle(&mut f);
        assert!(f.players[1].lp < lp_before, "the attack connected");
        assert_eq!(
            f.players[0].main.len(),
            deck_before - 1,
            "one fewer in deck"
        );
        assert_eq!(f.players[0].hand.len(), hand_before + 1);
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

    /// **Damage the other way round draws nothing.** Attacking into a
    /// bigger monster hurts its controller, and the condition refuses.
    #[test]
    fn battle_damage_to_itself_draws_nothing() {
        let (mut f, _ak) = field(Some(2600));
        let deck_before = f.players[0].main.len();
        let lp_before = f.players[0].lp;
        run_battle(&mut f);
        assert!(f.players[0].lp < lp_before, "it took the damage");
        assert_eq!(
            f.players[0].main.len(),
            deck_before,
            "and drew nothing for it"
        );
    }

    /// Declare the one available attack and decline every optional
    /// window, stopping when the battle menu comes back.
    fn run_battle(f: &mut Field) {
        f.emplace(Kind::BattleCommand {
            state: Box::default(),
        });
        let mut attacked = false;
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectBattleCmd { attackable, .. }) if !attacked => {
                        assert!(!attackable.is_empty(), "the attack is available");
                        f.core.returns.set(1);
                        attacked = true;
                    }
                    Some(Message::SelectBattleCmd { .. }) => break,
                    Some(Message::SelectChain { chains, .. }) => {
                        f.core.returns.set(if chains.is_empty() { -1 } else { 0 });
                    }
                    Some(Message::SelectCard { min, .. }) => {
                        let min = usize::from(*min);
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
    }
}
