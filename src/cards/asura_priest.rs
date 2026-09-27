//! Asura Priest — `c2134346.lua`.
//!
//! The thirty-sixth card, and the pool's first **spirit**. Eighteen lines,
//! of which one is a procedure call — see `cards/proc_spirit.rs` for the
//! End Phase return, which is most of what a spirit is.
//!
//! What is left is two single effects, and they are a matched pair: the
//! card cannot be Special Summoned, and it can attack every monster the
//! opponent controls once each. Neither has a condition; both simply
//! apply.
//!
//! ## `EFFECT_SPSUMMON_CONDITION` with no value
//!
//! ```lua
//! e1:SetCode(EFFECT_SPSUMMON_CONDITION)
//! ```
//!
//! No `SetValue`, so the value is zero and the condition is never
//! satisfied — which is how "cannot be Special Summoned" is written. It
//! is `EFFECT_FLAG_CANNOT_DISABLE` so that switching the card off does
//! not switch the restriction off with it, and
//! `EFFECT_FLAG_UNCOPYABLE` so that a card copying this one does not
//! inherit it.
//!
//! ## `EFFECT_ATTACK_ALL` with value 1
//!
//! The value is how many times each target may be attacked, not how many
//! attacks there are. One, here — so every monster the opponent controls
//! can be attacked once, which is the card's whole point.
//!
//! ## The spirit events this card names
//!
//! `Spirit.AddProcedure(c, EVENT_SUMMON_SUCCESS, EVENT_FLIP)` — a Normal
//! Summon or a flip sets the return flag. Not a special summon, because
//! `e1` has just made that impossible.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::effect::{effect_type, flag};
use crate::event::{code, CardId};
use crate::field::Field;
use crate::script_api as api;

use super::proc_spirit;

pub const CODE: u32 = 2_134_346;

/// How many times each monster may be attacked — `e2:SetValue(1)`.
const ATTACKS_EACH: i64 = 1;

pub fn initial_effect(f: &mut Field, c: CardId) {
    proc_spirit::add_procedure(f, c, &[code::SUMMON_SUCCESS, code::FLIP]);
    // Cannot be Special Summoned
    let e1 = api::create_effect(f, c);
    api::set_type(f, e1, effect_type::SINGLE);
    api::set_property(f, e1, flag::CANNOT_DISABLE | flag::UNCOPYABLE, 0);
    api::set_code(f, e1, code::SPSUMMON_CONDITION);
    api::register_effect(f, c, e1, false);
    // Can attack all monsters once each
    let e2 = api::create_effect(f, c);
    api::set_type(f, e2, effect_type::SINGLE);
    api::set_code(f, e2, code::ATTACK_ALL);
    api::set_value(f, e2, ATTACKS_EACH);
    api::register_effect(f, c, e2, false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{location, position};
    use crate::card::{card_type, status, Card};
    use crate::effect::Ctx;
    use crate::event::{EffectId, Event};

    /// Asura Priest face-up in `tp`'s first Monster Zone.
    fn field_as(tp: u8) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = crate::duel::phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let a = f.new_card(d);
        f.add_card(tp, a, location::MZONE, 0, false);
        f.cards[a].current.position = position::FACEUP_ATTACK;
        f.initialize_card(a);
        f.apply_field_effect(a);
        (f, a)
    }

    fn field() -> (Field, CardId) {
        field_as(0)
    }

    /// **Two single effects of its own, and the spirit procedure's.**
    #[test]
    fn the_script_registers_two_singles_and_the_spirit_procedure() {
        let (f, a) = field();
        // Cannot be Special Summoned: the value is **zero**, which is how
        // an unsatisfiable condition is written.
        let ban = f.cards[a]
            .single_effect
            .equal_range(code::SPSUMMON_CONDITION);
        assert_eq!(ban.len(), 1);
        let e1 = f.effects.get(ban[0]).unwrap();
        assert!(e1.is_type(effect_type::SINGLE));
        assert_eq!(e1.value, 0, "never satisfied");
        assert!(e1.value_fn.is_none(), "and not a function either");
        assert!(e1.is_flag(flag::CANNOT_DISABLE));
        assert!(e1.is_flag(flag::UNCOPYABLE));

        let all = f.cards[a].single_effect.equal_range(code::ATTACK_ALL);
        assert_eq!(all.len(), 1);
        let e2 = f.effects.get(all[0]).unwrap();
        assert_eq!(e2.value, 1, "each monster once");

        // And the procedure's two triggers plus two flag-setters.
        let end = code::PHASE | u32::from(crate::duel::phases::END);
        assert_eq!(
            f.cards[a].field_effect.equal_range(end).len(),
            2,
            "a mandatory return and its optional twin"
        );
        for event in [code::SUMMON_SUCCESS, code::FLIP] {
            assert_eq!(
                f.cards[a].single_effect.equal_range(event).len(),
                1,
                "a flag-setter for {event}"
            );
        }
    }

    /// **It cannot be Special Summoned**, which is the whole content of
    /// the first effect and is visible through the permission check.
    #[test]
    fn it_cannot_be_special_summoned() {
        let (mut f, a) = field();
        // Move it somewhere it could be summoned from.
        f.move_card(0, a, location::GRAVE, 0, false);
        let mut e = crate::effect::Effect::new(effect_type::SINGLE, 0);
        e.owner = Some(a);
        e.handler = Some(a);
        let by = f.new_effect(e);
        assert!(
            !api::is_can_be_special_summoned(&mut f, a, by, 0, 0, false, false),
            "the condition is never satisfied"
        );
        // The sibling: an ordinary monster on the same board can be.
        let mut c = Card::with_data(
            crate::card::CardData {
                code: 9_001,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        c.set_status(status::EFFECT_ENABLED, true);
        let plain = f.new_card(c);
        f.add_card(0, plain, location::GRAVE, 1, false);
        assert!(api::is_can_be_special_summoned(
            &mut f, plain, by, 0, 0, false, false
        ));
    }

    /// **It may attack each monster once**, which is `EFFECT_ATTACK_ALL`
    /// with a value of one — the value being *per target*, not a total.
    #[test]
    fn it_may_attack_each_monster_once() {
        let (f, a) = field();
        let e = f.cards[a].single_effect.equal_range(code::ATTACK_ALL)[0];
        let ev = Event::new(0);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: Some(a),
            args: &[],
        };
        assert_eq!(f.effects.get(e).unwrap().get_value(&f, &ctx), 1);
    }

    /// **The spirit events are a Normal Summon and a flip — not a
    /// special summon**, which the card has just made impossible.
    #[test]
    fn the_flag_is_set_by_a_summon_or_a_flip_and_nothing_else() {
        let (f, a) = field();
        assert!(
            !api::has_flag_effect(&f, a, proc_spirit::FLAG_SPIRIT_RETURN),
            "nothing yet"
        );
        for event in [code::SUMMON_SUCCESS, code::FLIP] {
            let (mut f, a) = field();
            let e: EffectId = f.cards[a].single_effect.equal_range(event)[0];
            let ev = Event::new(event);
            let ctx = Ctx {
                reason_effect: e,
                player: 0,
                event: &ev,
                card: None,
                args: &[],
            };
            let op = f.effects.get(e).unwrap().operation.expect("an operation");
            op(&mut f, &ctx);
            assert!(
                api::has_flag_effect(&f, a, proc_spirit::FLAG_SPIRIT_RETURN),
                "{event} set the flag"
            );
        }
        // No flag-setter is registered for a special summon at all.
        assert!(f.cards[a]
            .single_effect
            .equal_range(code::SPSUMMON_SUCCESS)
            .is_empty());
    }
}
