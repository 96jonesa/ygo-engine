//! Threatening Roar — `c36361633.lua`.
//!
//! The first card that **builds an effect while it resolves**. Everything
//! so far has registered its effects once, in `initial_effect`, and then
//! acted on the board; this one's operation creates a fresh
//! `EFFECT_TYPE_FIELD` effect and hands it to the duel, where it sits
//! until the End Phase forbidding the opponent from declaring an attack.
//!
//! The prohibition is `EFFECT_CANNOT_ATTACK_ANNOUNCE`, which is a
//! different thing from `EFFECT_CANNOT_ATTACK` — one stops the
//! declaration, the other the attack itself — and the port had both long
//! before there was a card to register either.
//!
//! Two details worth keeping straight:
//!
//! - **`SetTargetRange(0, 1)` is not symmetric.** Zero for the
//!   controller's own side and one for the opponent's, and with
//!   `EFFECT_FLAG_PLAYER_TARGET` the range is about *players*, not cards.
//!   The effect is registered to `tp`, and it forbids `tp`'s opponent.
//! - **`SetHintTiming` is a hint.** It tells a client when to offer the
//!   card; it does not make anything legal. The condition does that.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::duel::phases;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{code, CardId};
use crate::field::{reset, timing, Field};
use crate::script_api as api;

pub const CODE: u32 = 36_361_633;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate
    let e1 = api::create_effect(f, c);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_hint_timing(f, e1, 0, timing::BATTLE_START);
    api::set_condition(f, e1, condition);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

fn condition(f: &mut Field, ctx: &Ctx) -> bool {
    let ph = api::get_current_phase(f);
    api::is_turn_player(f, 1 - ctx.player) && ph & (phases::MAIN2 | phases::END) == 0
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let Some(handler) = api::get_handler(f, ctx.reason_effect) else {
        return Yield::Done(0);
    };
    let e1 = api::create_effect(f, handler);
    api::set_type(f, e1, effect_type::FIELD);
    api::set_code(f, e1, code::CANNOT_ATTACK_ANNOUNCE);
    api::set_property(f, e1, flag::PLAYER_TARGET, 0);
    api::set_reset(f, e1, reset::PHASE | u32::from(phases::END), 0);
    api::set_target_range(f, e1, 0, 1);
    api::duel_register_effect(f, e1, tp);
    Yield::Done(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, Card, CardData};
    use crate::field::Message;
    use crate::position;
    use crate::processor::{Kind, Status};

    fn card(f: &mut Field, owner: u8, code_: u32, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_,
                level: 4,
                attack: 1700,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(crate::card::status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        let fid = f.next_field_id_raw();
        f.cards[id].fieldid = fid;
        f.cards[id].fieldid_r = fid;
        id
    }

    /// Player 1's turn in the given phase, with Threatening Roar Set in
    /// player 0's row since an earlier turn and a monster of player 1's
    /// ready to attack.
    fn field(phase: u16) -> (Field, CardId, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 4;
        f.infos.turn_player = 1;
        f.infos.phase = phase;
        let attacker = card(&mut f, 1, 1000, card_type::MONSTER);
        f.add_card(1, attacker, location::MZONE, 0, false);
        f.cards[attacker].current.position = position::FACEUP_ATTACK;
        let roar = card(&mut f, 0, CODE, card_type::TRAP);
        f.add_card(0, roar, location::SZONE, 0, false);
        f.cards[roar].current.position = position::FACEDOWN;
        f.initialize_card(roar);
        (f, roar, attacker)
    }

    fn ctx_for(f: &Field, roar: CardId) -> crate::event::EffectId {
        f.cards[roar].field_effect.equal_range(code::FREE_CHAIN)[0]
    }

    /// **`initial_effect` registers one printed activate effect** on
    /// `EVENT_FREE_CHAIN`, with a condition, a hint timing of
    /// `TIMING_BATTLE_START` for the opponent only, and **no category** —
    /// it destroys and draws nothing.
    #[test]
    fn the_script_registers_a_printed_activate_effect() {
        let (f, roar, _) = field(phases::BATTLE_START);
        let ids: Vec<_> = f.cards[roar]
            .field_effect
            .equal_range(code::FREE_CHAIN)
            .to_vec();
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(e.is_flag(flag::INITIAL));
        assert!(e.condition.is_some());
        assert_eq!(e.category, 0, "no category: it neither destroys nor draws");
        assert_eq!(
            e.hint_timing,
            [0, timing::BATTLE_START],
            "offered to the opponent at the start of the battle, and never on its own turn"
        );
    }

    /// **The condition wants the opponent's turn and a phase before Main
    /// 2.** Both halves, and both ways round.
    #[test]
    fn the_condition_wants_the_opponents_turn_before_main_two() {
        for (turn_player, phase, want) in [
            (1u8, phases::BATTLE_START, true),
            (1, phases::MAIN1, true),
            (1, phases::MAIN2, false),
            (1, phases::END, false),
            (0, phases::BATTLE_START, false),
            (0, phases::MAIN2, false),
        ] {
            let (mut f, roar, _) = field(phase);
            f.infos.turn_player = turn_player;
            let e = ctx_for(&f, roar);
            let ev = crate::event::Event::new(0);
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            assert_eq!(
                condition(&mut f, &ctx),
                want,
                "turn player {turn_player}, phase {phase:#x}"
            );
        }
    }

    /// **Resolving it registers a field effect on the duel**, not on a
    /// card: `EFFECT_TYPE_FIELD` with `EFFECT_CANNOT_ATTACK_ANNOUNCE`, a
    /// player target, ranges `(0, 1)` — nothing of its own side, the
    /// opponent's — and a reset at the End Phase that `SetReset` widens to
    /// both turns.
    #[test]
    fn resolving_it_registers_the_prohibition_against_the_opponent() {
        let (mut f, roar, _) = field(phases::BATTLE_START);
        let e = ctx_for(&f, roar);
        let before = f
            .field_effects
            .aura
            .equal_range(code::CANNOT_ATTACK_ANNOUNCE)
            .len();
        assert_eq!(before, 0);
        let ev = crate::event::Event::new(0);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        activate(&mut f, &ctx);
        let ids = f
            .field_effects
            .aura
            .equal_range(code::CANNOT_ATTACK_ANNOUNCE)
            .to_vec();
        assert_eq!(ids.len(), 1, "one prohibition registered");
        let new = f.effects.get(ids[0]).unwrap();
        assert!(new.is_type(effect_type::FIELD));
        assert!(new.is_flag(flag::PLAYER_TARGET));
        assert_eq!((new.s_range, new.o_range), (0, 1));
        assert_eq!(new.handler, Some(roar), "carried by the card that made it");
        assert!(new.reset_flag & reset::PHASE != 0);
        assert!(new.reset_flag & u32::from(phases::END) != 0);
        assert!(
            new.reset_flag & (reset::SELF_TURN | reset::OPPO_TURN)
                == reset::SELF_TURN | reset::OPPO_TURN,
            "SetReset folds both turns into a phase reset"
        );
    }

    /// **And the opponent then cannot declare an attack.** The end the
    /// card exists for, read off the battle menu: a monster that could
    /// attack a moment ago is not offered.
    #[test]
    fn after_it_resolves_the_opponent_has_nothing_to_attack_with() {
        // The baseline, on its own board: the attack is there to be lost.
        let (mut baseline, _, _) = field(phases::BATTLE_STEP);
        assert!(
            attackable_count(&mut baseline) > 0,
            "the attack is available to begin with"
        );
        let (mut f, roar, _) = field(phases::BATTLE_STEP);
        let e = ctx_for(&f, roar);
        let ev = crate::event::Event::new(0);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        activate(&mut f, &ctx);
        assert!(
            f.is_player_affected_by_effect(1, code::CANNOT_ATTACK_ANNOUNCE)
                .is_some(),
            "the turn player is forbidden"
        );
        assert!(
            f.is_player_affected_by_effect(0, code::CANNOT_ATTACK_ANNOUNCE)
                .is_none(),
            "and its controller is not"
        );
        assert_eq!(attackable_count(&mut f), 0, "nothing may declare an attack");
    }

    /// How many attacks the battle menu offers the turn player.
    fn attackable_count(f: &mut Field) -> usize {
        f.emplace(Kind::BattleCommand {
            state: Box::default(),
        });
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => {
                    let Some(Message::SelectBattleCmd { attackable, .. }) = f.messages.last()
                    else {
                        panic!("a different question: {:?}", f.messages.last());
                    };
                    return attackable.len();
                }
                other => panic!("stopped with {other:?}"),
            }
        }
        panic!("no battle menu")
    }
}
