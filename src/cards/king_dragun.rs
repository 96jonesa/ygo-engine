//! King Dragun (`13756293`) — `cardscripts/c13756293.lua`.
//!
//! The last card of the pool, and the one the harness **cannot reach**:
//! it is level 7, and the only route a Fusion Monster has to the field
//! here is Metamorphosis, which tributes a monster of the same level.
//! The pool's levels are 1 (Scapegoat's tokens), 4, 5, 6 and 8. No level
//! 7, so no King Dragun — the sixth kind of unreachable branch recorded
//! in `docs/processor-loop.md`, and the first that is a whole card.
//!
//! It is ported anyway, to the same standard: transcribed from the
//! script, every constant pinned, every branch under a unit test, and a
//! differential regression sweep that shows a King Dragun in the Extra
//! Deck changes nothing. What it cannot have is the reachability count
//! the other forty-nine carry.
//!
//! ## `aux.tgoval` is about the *targeting* player
//!
//! ```lua
//! function Auxiliary.tgoval(e,re,rp) return rp~=e:GetHandlerPlayer() end
//! ```
//!
//! Dragons cannot be targeted by the **opponent's** effects; this card's
//! own controller may still target them. The value seam hands the
//! protection over as `&Effect` and the targeting effect as
//! `ctx.reason_effect`, so "my handler's player" is asked of the effect
//! object — `get_handler_player_of` is the export for that — and "the
//! targeting player" is `ctx.player`.
//!
//! ## `EFFECT_FLAG_IGNORE_IMMUNE` on a protection
//!
//! The aura protects Dragons from targeting, and would itself be shrugged
//! off by a Dragon immune to this card's effects. The flag makes the
//! protection reach every Dragon in range regardless. Transcribed; no
//! card in this pool grants immunity, so the flag has no card to bite on.
//!
//! ## The Special Summon is by race and by permission
//!
//! `s.filter` asks `IsRace(RACE_DRAGON)` and `IsCanBeSpecialSummoned`,
//! the second with the *effect* and *player* the scan is handed — the
//! reference passes them as extra arguments and the port's closure
//! captures them. A seat is asked for twice: at the target, where it
//! decides whether the effect may be activated, and again at the
//! operation, where losing it since is a silent return.

use crate::board::{location, position};
use crate::card::race;
use crate::effect::{effect_type, flag, Ctx, Effect, Yield};
use crate::event::{category, code, CardId, EffectId};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 13756293;

/// `Fusion.AddProcMix(c,true,true,17985575,62113340)`, each material
/// pinned against the script by `tools/check_constants.py`.
const MATERIAL_A: u32 = 17_985_575;
const MATERIAL_B: u32 = 62_113_340;
const MATERIALS: [u32; 2] = [MATERIAL_A, MATERIAL_B];

const MZONE: u16 = location::MZONE as u16;
const HAND: u32 = location::HAND as u32;

pub fn initial_effect(f: &mut Field, c: CardId) {
    api::enable_revive_limit(f, c);
    super::proc_fusion::add_proc_mix(f, c, true, true, &MATERIALS);
    // Dragons cannot be targeted by the opponent
    let e1 = api::create_effect(f, c);
    api::set_type(f, e1, effect_type::FIELD);
    api::set_code(f, e1, code::CANNOT_BE_EFFECT_TARGET);
    api::set_property(f, e1, flag::IGNORE_IMMUNE, 0);
    api::set_range(f, e1, MZONE);
    api::set_target_range(f, e1, MZONE, MZONE);
    api::set_target_filter(f, e1, target_is_dragon);
    api::set_value_fn(f, e1, tgoval);
    api::register_effect(f, c, e1, false);
    // Special Summon a Dragon from the hand
    let e2 = api::create_effect(f, c);
    api::set_description(f, e2, api::stringid(CODE, 0));
    api::set_category(f, e2, category::SPECIAL_SUMMON);
    api::set_type(f, e2, effect_type::IGNITION);
    api::set_count_limit(f, e2, 1, 0, 0);
    api::set_range(f, e2, MZONE);
    api::set_target(f, e2, sptg);
    api::set_operation(f, e2, spop);
    api::register_effect(f, c, e2, false);
}

/// `aux.TargetBoolFunction(Card.IsRace, RACE_DRAGON)` — printed race,
/// under the `ABSENT_FROM_POOL` licence `is_race_readonly` records.
fn target_is_dragon(f: &Field, _e: EffectId, target: Option<CardId>, _a: &[i64]) -> bool {
    target.is_some_and(|c| api::is_race_readonly(f, c, race::DRAGON))
}

/// `aux.tgoval(e,re,rp)` — `rp ~= e:GetHandlerPlayer()`.
fn tgoval(e: &Effect, f: &Field, ctx: &Ctx) -> i64 {
    i64::from(ctx.player != api::get_handler_player_of(f, e))
}

/// `s.filter(c,e,tp)`.
fn filter(f: &mut Field, c: CardId, e: EffectId, tp: u8) -> bool {
    api::is_race(f, c, race::DRAGON)
        && api::is_can_be_special_summoned(f, c, e, 0, tp, false, false)
}

/// `s.sptg`.
fn sptg(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    let e = ctx.reason_effect;
    if !chk {
        let matches = move |f: &mut Field, c: CardId| filter(f, c, e, tp);
        return api::yes(
            api::get_location_count(f, tp, location::MZONE) > 0
                && api::is_existing_matching_card(
                    f,
                    Some(&matches),
                    tp,
                    HAND,
                    0,
                    1,
                    api::Except::None,
                ),
        );
    }
    api::set_operation_info(
        f,
        0,
        category::SPECIAL_SUMMON,
        None,
        1,
        tp,
        i32::from(location::HAND),
    );
    api::yes(true)
}

/// `s.spop`.
fn spop(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let e = ctx.reason_effect;
    if api::get_location_count(f, tp, location::MZONE) <= 0 {
        return api::done();
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::SPSUMMON);
    let matches = move |f: &mut Field, c: CardId| filter(f, c, e, tp);
    api::select_matching_card(f, tp, Some(&matches), tp, HAND, 0, 1, 1, api::Except::None);
    api::suspend(move |f, _ctx| {
        let g = api::group_selected(f);
        if g.is_empty() {
            return api::done();
        }
        api::special_summon(f, g, 0, tp, tp, false, false, position::FACEUP);
        api::suspend(move |_f, _ctx| api::done())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{attribute, card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::Event;
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    /// King Dragun in `tp`'s Monster Zone, initialised.
    fn board(tp: u8) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let kd = f.new_card(d);
        f.add_card(tp, kd, location::MZONE, 0, false);
        f.cards[kd].current.position = position::FACEUP_ATTACK;
        f.initialize_card(kd);
        (f, kd)
    }

    fn card_of(f: &mut Field, player: u8, seq: u32, race_: u64, loc: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 7700
                    + seq
                    + u32::from(player) * 50
                    + if loc == location::HAND { 500 } else { 0 },
                type_: card_type::MONSTER | card_type::EFFECT,
                level: 4,
                attack: 1500,
                defense: 1000,
                race: race_,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, loc, seq, false);
        if loc == location::MZONE {
            f.cards[id].current.position = position::FACEUP_ATTACK;
        }
        id
    }

    fn ignition(f: &Field, kd: CardId) -> EffectId {
        f.cards[kd]
            .field_effect
            .iter()
            .map(|(_, e)| *e)
            .find(|&e| {
                f.effects
                    .get(e)
                    .is_some_and(|x| x.is_type(effect_type::IGNITION))
            })
            .expect("the ignition effect")
    }

    fn protection(f: &Field, kd: CardId) -> EffectId {
        f.cards[kd]
            .field_effect
            .equal_range(code::CANNOT_BE_EFFECT_TARGET)[0]
    }

    fn ctx_for<'a>(e: EffectId, tp: u8, ev: &'a Event) -> Ctx<'a> {
        Ctx {
            reason_effect: e,
            player: tp,
            event: ev,
            card: None,
            args: &[],
        }
    }

    /// The target's `chk == 0` arm.
    fn can_activate(f: &mut Field, kd: CardId, tp: u8) -> bool {
        let e = ignition(f, kd);
        f.core.reason_effect = Some(e);
        let ev = Event::new(0);
        sptg(f, &ctx_for(e, tp, &ev), false, None)
            .finished()
            .unwrap_or(0)
            != 0
    }

    /// Answer the engine's questions until it settles; returns the last
    /// card offer.
    fn settle(f: &mut Field) -> Vec<CardId> {
        let mut offered = Vec::new();
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        return offered;
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectCard { min, cards, .. }) => {
                        offered = cards.clone();
                        let min = usize::from(*min);
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, i as i32);
                        }
                    }
                    Some(Message::SelectPosition { positions, .. }) => {
                        let pos = *positions;
                        let pick = [1u8, 2, 4, 8]
                            .into_iter()
                            .find(|b| pos & b != 0)
                            .expect("a position");
                        f.core.returns.set(i32::from(pick));
                    }
                    Some(Message::SelectPlace { player, flag, .. }) => {
                        let (pl, flag) = (*player, *flag);
                        let seq = (0..5u32)
                            .find(|s| flag & (1 << s) == 0)
                            .expect("a free monster seat");
                        f.core.returns.set_i8(0, pl as i8);
                        f.core.returns.set_i8(1, location::MZONE as i8);
                        f.core.returns.set_i8(2, seq as i8);
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        panic!("did not settle");
    }

    fn target_step(f: &mut Field, kd: CardId, tp: u8) {
        let e = ignition(f, kd);
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 81;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.cards[kd].create_chain_relation(e, 81);
        f.core.sub_solving_event.push_back(Event::new(0));
        f.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: e,
            player: tp,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        settle(f);
    }

    fn operation_step(f: &mut Field, kd: CardId, tp: u8) -> Vec<CardId> {
        let e = ignition(f, kd);
        f.core.sub_solving_event.push_back(Event::new(0));
        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player: tp,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        let offered = settle(f);
        f.core.chain_solving = false;
        f.core.current_chain.clear();
        offered
    }

    mod registration {
        use super::*;

        /// **Two effects**, an aura and an ignition.
        #[test]
        fn the_aura_and_the_ignition() {
            let (f, kd) = board(0);
            let x = f.effects.get(protection(&f, kd)).expect("the aura");
            assert!(x.is_type(effect_type::FIELD));
            assert!(x.is_flag(flag::IGNORE_IMMUNE), "reaches an immune Dragon");
            assert_eq!(x.range, MZONE);
            assert_eq!((x.s_range, x.o_range), (MZONE, MZONE), "both rows");
            assert!(x.target_filter.is_some(), "filtered to Dragons");
            assert!(x.value_fn.is_some(), "and valued by who is targeting");

            let y = f.effects.get(ignition(&f, kd)).expect("the ignition");
            assert_eq!(y.category, category::SPECIAL_SUMMON);
            assert_eq!(y.count_limit, 1, "SetCountLimit(1)");
            assert_eq!(y.range, MZONE);
            assert!(y.target.is_some() && y.operation.is_some());

            assert_eq!(api::fusion_materials(&f, kd), &MATERIALS);
            for wanted in [code::UNSUMMONABLE_CARD, code::REVIVE_LIMIT] {
                assert_eq!(f.cards[kd].single_effect.equal_range(wanted).len(), 1);
            }
        }

        /// **The printed line, transcribed from the oracle's table.** Level
        /// 7 is the whole reason this card is unreachable.
        #[test]
        fn its_printed_line_matches_the_oracles_table() {
            let d = crate::cards::card_data(CODE).expect("printed data");
            assert!(d.is_type(card_type::FUSION) && d.is_type(card_type::MONSTER));
            assert_eq!((d.level, d.attack, d.defense), (7, 2400, 1100));
            assert_eq!(d.attribute, attribute::DARK);
            assert_eq!(d.race, race::DRAGON);
        }
    }

    mod the_protection {
        use super::*;

        /// The filter: Dragons, and nothing else.
        #[test]
        fn the_filter_admits_dragons_alone() {
            let (mut f, kd) = board(0);
            let dragon = card_of(&mut f, 1, 0, race::DRAGON, location::MZONE);
            let fiend = card_of(&mut f, 1, 1, race::FIEND, location::MZONE);
            let e = protection(&f, kd);
            assert!(target_is_dragon(&f, e, Some(dragon), &[]));
            assert!(!target_is_dragon(&f, e, Some(fiend), &[]));
            assert!(!target_is_dragon(&f, e, None, &[]));
            assert!(
                target_is_dragon(&f, e, Some(kd), &[]),
                "including itself — the handler is not exempted here"
            );
        }

        /// **`tgoval` is about the targeting player**: the opponent is
        /// refused, this card's own controller is not.
        #[test]
        fn the_value_refuses_the_opponent_and_admits_its_controller() {
            let (f, kd) = board(0);
            let e = protection(&f, kd);
            let x = f.effects.get(e).expect("the aura");
            let ev = Event::new(0);
            // `reason_effect` is the *targeting* effect in the real call;
            // any id will do for the value, which reads only `player`.
            assert_eq!(tgoval(x, &f, &ctx_for(e, 1, &ev)), 1, "the opponent");
            assert_eq!(tgoval(x, &f, &ctx_for(e, 0, &ev)), 0, "its controller");
        }

        /// **And through the engine**: an opponent's effect cannot target
        /// a Dragon in either row, a non-Dragon is fair game, and the
        /// controller's own effect may target anything.
        #[test]
        fn dragons_cannot_be_targeted_by_the_opponents_effects() {
            let (mut f, kd) = board(0);
            let my_dragon = card_of(&mut f, 0, 1, race::DRAGON, location::MZONE);
            let their_dragon = card_of(&mut f, 1, 0, race::DRAGON, location::MZONE);
            let their_fiend = card_of(&mut f, 1, 1, race::FIEND, location::MZONE);
            let theirs = api::create_effect(&mut f, their_fiend);
            api::set_type(&mut f, theirs, effect_type::IGNITION);
            api::register_effect(&mut f, their_fiend, theirs, false);
            let mine = api::create_effect(&mut f, my_dragon);
            api::set_type(&mut f, mine, effect_type::IGNITION);
            api::register_effect(&mut f, my_dragon, mine, false);

            for c in [kd, my_dragon, their_dragon] {
                assert!(
                    !f.is_capable_be_effect_target(c, theirs, 1),
                    "the opponent cannot target a Dragon"
                );
                assert!(
                    f.is_capable_be_effect_target(c, mine, 0),
                    "its controller can"
                );
            }
            assert!(
                f.is_capable_be_effect_target(their_fiend, theirs, 1),
                "a Fiend is not protected"
            );
        }
    }

    mod the_exports {
        use super::*;

        /// `Card.IsRace` is a **mask** test: asked about two races at
        /// once, a Dragon says yes.
        #[test]
        fn is_race_readonly_is_a_mask() {
            let (mut f, _kd) = board(0);
            let dragon = card_of(&mut f, 1, 0, race::DRAGON, location::MZONE);
            assert!(api::is_race_readonly(
                &f,
                dragon,
                race::DRAGON | race::FIEND
            ));
            assert!(!api::is_race_readonly(&f, dragon, race::FIEND));
        }

        /// `Effect.GetHandlerPlayer` of an effect object answers the
        /// **handler's** controller, which for an effect one card put on
        /// another is not the owner's.
        #[test]
        fn get_handler_player_of_reads_the_handler_not_the_owner() {
            let (mut f, kd) = board(0);
            let theirs = card_of(&mut f, 1, 0, race::FIEND, location::MZONE);
            let e = api::create_effect(&mut f, kd);
            api::set_type(&mut f, e, effect_type::SINGLE);
            api::set_code(&mut f, e, code::CANNOT_ATTACK);
            api::register_effect(&mut f, theirs, e, false);
            let x = f.effects.get(e).expect("registered on the opponent's card");
            assert_eq!(x.owner, Some(kd), "owned by King Dragun (player 0)");
            assert_eq!(
                api::get_handler_player_of(&f, x),
                1,
                "handled by the opponent"
            );
        }
    }

    mod the_special_summon {
        use super::*;

        /// **A Dragon in the hand, and a seat for it.**
        #[test]
        fn a_dragon_in_hand_and_a_seat() {
            let (mut f, kd) = board(0);
            assert!(!can_activate(&mut f, kd, 0), "nothing in the hand");
            card_of(&mut f, 0, 0, race::DRAGON, location::HAND);
            assert!(can_activate(&mut f, kd, 0));
        }

        /// A non-Dragon in the hand is not enough.
        #[test]
        fn a_fiend_in_hand_is_not_enough() {
            let (mut f, kd) = board(0);
            card_of(&mut f, 0, 0, race::FIEND, location::HAND);
            assert!(!can_activate(&mut f, kd, 0));
        }

        /// **A Dragon that may not be Special Summoned is not enough**
        /// — `IsCanBeSpecialSummoned`, asked with this effect and player.
        #[test]
        fn a_dragon_that_cannot_be_special_summoned_is_not_enough() {
            let (mut f, kd) = board(0);
            let dragon = card_of(&mut f, 0, 0, race::DRAGON, location::HAND);
            // A revive limit on a card never properly summoned: the
            // script-level way to make a monster unsummonable.
            api::enable_revive_limit(&mut f, dragon);
            assert!(!can_activate(&mut f, kd, 0));
        }

        /// **No seat, no activation.**
        #[test]
        fn a_full_monster_zone_stops_it() {
            let (mut f, kd) = board(0);
            card_of(&mut f, 0, 0, race::DRAGON, location::HAND);
            for seq in 1..5 {
                card_of(&mut f, 0, seq, race::FIEND, location::MZONE);
            }
            assert!(!can_activate(&mut f, kd, 0));
        }

        /// The opponent's hand is not scanned.
        #[test]
        fn the_opponents_hand_is_not_scanned() {
            let (mut f, kd) = board(0);
            card_of(&mut f, 1, 0, race::DRAGON, location::HAND);
            assert!(!can_activate(&mut f, kd, 0));
        }

        /// **The announcement**: one Special Summon, by this player, from
        /// the hand.
        #[test]
        fn the_announcement_names_the_hand() {
            let (mut f, kd) = board(0);
            card_of(&mut f, 0, 0, race::DRAGON, location::HAND);
            target_step(&mut f, kd, 0);
            let info = f.core.current_chain[0]
                .opinfos
                .get(&category::SPECIAL_SUMMON)
                .expect("announced");
            assert_eq!(
                (info.count, info.player, info.param),
                (1, 0, i32::from(location::HAND))
            );
        }

        /// **End to end: the Dragon arrives face-up**, chosen from a
        /// filtered offer, with the prompt saying so.
        #[test]
        fn the_dragon_is_special_summoned_face_up() {
            let (mut f, kd) = board(0);
            let fiend = card_of(&mut f, 0, 0, race::FIEND, location::HAND);
            let dragon = card_of(&mut f, 0, 1, race::DRAGON, location::HAND);
            target_step(&mut f, kd, 0);
            let offered = operation_step(&mut f, kd, 0);
            assert_eq!(offered, vec![dragon], "the Fiend was not offered");
            assert_eq!(f.cards[dragon].current.location, location::MZONE);
            assert_eq!(f.cards[dragon].current.controller, 0);
            assert!(f.cards[dragon].current.is_faceup());
            assert_eq!(f.cards[fiend].current.location, location::HAND);
            assert!(
                f.messages.iter().any(|m| matches!(
                    m,
                    Message::Hint { kind, value, .. }
                        if *kind == hint::SELECTMSG && *value == hintmsg::SPSUMMON
                )),
                "HINTMSG_SPSUMMON"
            );
        }

        /// **The Dragon gone from the hand since the target**: the choice
        /// is empty, and an empty choice summons nothing.
        #[test]
        fn a_dragon_gone_since_the_target_summons_nothing() {
            let (mut f, kd) = board(0);
            let dragon = card_of(&mut f, 0, 0, race::DRAGON, location::HAND);
            target_step(&mut f, kd, 0);
            f.move_card(0, dragon, location::GRAVE, 0, false);
            let seats_before = f.players[0].mzone.iter().flatten().count();
            let offered = operation_step(&mut f, kd, 0);
            assert!(offered.is_empty());
            assert_eq!(f.cards[dragon].current.location, location::GRAVE);
            assert_eq!(f.players[0].mzone.iter().flatten().count(), seats_before);
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::SelectPosition { .. })),
                "nothing was asked which way up"
            );
        }

        /// **The seat is re-checked at resolution**: filled since the
        /// target, nothing is summoned and nothing is asked.
        #[test]
        fn a_seat_lost_since_the_target_is_a_silent_return() {
            let (mut f, kd) = board(0);
            let dragon = card_of(&mut f, 0, 0, race::DRAGON, location::HAND);
            target_step(&mut f, kd, 0);
            for seq in 1..5 {
                card_of(&mut f, 0, seq, race::FIEND, location::MZONE);
            }
            let before = f.messages.len();
            let offered = operation_step(&mut f, kd, 0);
            assert!(offered.is_empty(), "no choice was offered");
            assert_eq!(f.cards[dragon].current.location, location::HAND);
            assert_eq!(f.messages.len(), before, "and no prompt either");
        }
    }
}
