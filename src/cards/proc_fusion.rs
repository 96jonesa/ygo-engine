//! `Fusion.AddProcMix` (`proc_fusion.lua:53`) — the procedure a Fusion
//! Monster registers so that a Fusion Summon can ask what it is made of.
//!
//! ## What this pool does and does not reach
//!
//! All seven Fusion Monsters here call it with two named codes:
//! `Fusion.AddProcMix(c, sub, insf, code1, code2)`. **Nothing in this
//! pool can perform a Fusion Summon** — there is no Polymerization and no
//! other card that fuses — so the `EFFECT_FUSION_MATERIAL` effect this
//! registers is never once asked its question. The only route a Fusion
//! Monster takes to the field here is Metamorphosis, which Special
//! Summons one from the Extra Deck by level and never touches the
//! material procedure.
//!
//! That is checked rather than asserted: `tools/check_constants.py`'s
//! `ABSENT_FROM_POOL` scan proves no pool script names
//! `SUMMON_TYPE_FUSION`, `Duel.SetFusionMaterial` or
//! `EFFECT_FUSION_MATERIAL`.
//!
//! So what is ported is the **registration** — the effect, its flags, and
//! the material list, which are what another card would read — and the
//! condition and operation are named functions that **panic**. That is
//! the port's standing rule for a subsystem it has not reached
//! (`README.md`): loud rather than a silent no-op, so the first card that
//! does reach it fails in the right place instead of quietly summoning
//! the wrong thing.
//!
//! `Fusion.ConditionMix` and `Fusion.OperationMix` are 90 lines apiece of
//! contact fusion, `FUSPROC_*` flags, `EFFECT_FUSION_MAT_RESTRICTION`,
//! substitute materials and three hard-coded card numbers. Translating
//! that on the strength of seven cards that cannot invoke it would be
//! writing an unexercised engine, which is the thing this port's
//! discipline exists to avoid.

use crate::card::status;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{code, CardId};
use crate::field::Field;
use crate::script_api as api;

/// `Fusion.AddProcMix(c, sub, insf, ...)` with a list of **named card
/// codes**, which is the only form this pool uses.
///
/// `sub` allows substitute materials and `insf` is the Instant Fusion
/// arm; both are carried so the call sites read like their scripts, and
/// both are only consulted by the condition this pool cannot reach.
pub fn add_proc_mix(f: &mut Field, c: CardId, sub: bool, insf: bool, materials: &[u32]) {
    // `if c:IsStatus(STATUS_COPYING_EFFECT) then return end`
    if api::is_status(f, c, status::COPYING_EFFECT) {
        return;
    }
    let _ = (sub, insf);
    // `mt.material`, `mt.material_count`, `mt.min/max_material_count` —
    // the metatable fields another card reads to ask what this monster is
    // made of. Nothing in this pool reads them; they are recorded because
    // they are the part of the procedure that is *data* rather than
    // control flow.
    api::set_fusion_materials(f, c, materials);

    let e1 = api::create_effect(f, c);
    api::set_type(f, e1, effect_type::SINGLE);
    api::set_property(f, e1, flag::CANNOT_DISABLE | flag::UNCOPYABLE, 0);
    api::set_code(f, e1, code::FUSION_MATERIAL);
    api::set_condition(f, e1, condition_mix);
    api::set_operation(f, e1, operation_mix);
    api::register_effect(f, c, e1, false);
}

/// `Fusion.ConditionMix` — **not ported**; see the module note.
fn condition_mix(_f: &mut Field, _ctx: &Ctx) -> bool {
    unimplemented!(
        "Fusion.ConditionMix: no card in this pool performs a Fusion Summon, so the material \
         procedure is unreachable (tools/check_constants.py's ABSENT_FROM_POOL proves it). \
         Porting it needs contact fusion, the FUSPROC_* flags, substitute materials and \
         EFFECT_FUSION_MAT_RESTRICTION."
    )
}

/// `Fusion.OperationMix` — **not ported**; see the module note.
fn operation_mix(_f: &mut Field, _ctx: &Ctx) -> Yield {
    unimplemented!(
        "Fusion.OperationMix: no card in this pool performs a Fusion Summon, so the material \
         procedure is unreachable (tools/check_constants.py's ABSENT_FROM_POOL proves it)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
    use crate::event::EffectId;

    fn fusion_card(f: &mut Field) -> CardId {
        let c = Card::with_data(
            CardData {
                code: 777_777,
                type_: card_type::MONSTER | card_type::FUSION,
                level: 6,
                ..Default::default()
            },
            0,
        );
        let id = f.new_card(c);
        f.add_card(0, id, crate::board::location::EXTRA, 0, false);
        id
    }

    fn procedures(f: &Field, c: CardId) -> Vec<EffectId> {
        f.cards[c]
            .single_effect
            .equal_range(code::FUSION_MATERIAL)
            .to_vec()
    }

    #[test]
    fn it_registers_one_undisableable_uncopyable_procedure() {
        let mut f = Field::new(8000);
        let c = fusion_card(&mut f);
        add_proc_mix(&mut f, c, true, true, &[11, 22]);
        let ids = procedures(&f, c);
        assert_eq!(ids.len(), 1);
        let x = f.effects.get(ids[0]).expect("the procedure");
        assert!(x.is_type(effect_type::SINGLE));
        assert!(x.is_flag(flag::CANNOT_DISABLE));
        assert!(x.is_flag(flag::UNCOPYABLE));
        assert_eq!(api::fusion_materials(&f, c), &[11, 22]);
    }

    /// `if c:IsStatus(STATUS_COPYING_EFFECT) then return end` — a card
    /// copying another's effects registers nothing at all, not even the
    /// material list.
    #[test]
    fn a_copying_card_registers_nothing() {
        let mut f = Field::new(8000);
        let c = fusion_card(&mut f);
        f.cards[c].set_status(status::COPYING_EFFECT, true);
        add_proc_mix(&mut f, c, true, true, &[11, 22]);
        assert!(procedures(&f, c).is_empty(), "no procedure");
        assert!(
            api::fusion_materials(&f, c).is_empty(),
            "and no material list either"
        );
    }

    /// **The condition and operation are not ported**, and say so loudly.
    /// Nothing in this pool can reach them —
    /// `tools/check_constants.py`'s `ABSENT_FROM_POOL` scan is the proof
    /// — so a panic here means a card was added that does perform a
    /// Fusion Summon, which is exactly when someone should be told.
    #[test]
    #[should_panic(expected = "no card in this pool performs a Fusion Summon")]
    fn the_condition_is_a_loud_refusal() {
        let mut f = Field::new(8000);
        let ev = crate::event::Event::new(0);
        let ctx = Ctx {
            reason_effect: 0,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        condition_mix(&mut f, &ctx);
    }

    #[test]
    #[should_panic(expected = "no card in this pool performs a Fusion Summon")]
    fn the_operation_is_a_loud_refusal() {
        let mut f = Field::new(8000);
        let ev = crate::event::Event::new(0);
        let ctx = Ctx {
            reason_effect: 0,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        operation_mix(&mut f, &ctx);
    }
}
