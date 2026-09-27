//! Snatch Steal — `c45986603.lua`.
//!
//! The thirty-third card, and the pool's first to use a **library
//! procedure**: `aux.AddEquipProcedure` builds its activation and its
//! equip limit, and the card supplies only what is specific to it. See
//! `cards/proc_equip.rs` for the procedure itself.
//!
//! The card is three ideas, one per effect.
//!
//! ## It equips to the *opponent's* monster, and the limit says so twice
//!
//! ```lua
//! function s.eqlimit(e,c)
//!   return e:GetHandlerPlayer()~=c:GetControler() or e:GetHandler():GetEquipTarget()==c
//! end
//! ```
//!
//! A disjunction, and the second half is what keeps it attached. The
//! first says "not my own monster" — which is the rule the card is for.
//! But once it has taken control, the monster **is** its controller's, so
//! the first half turns false and the limit would come off by rule a
//! moment after it applied. The second half is the exemption: whatever
//! this very card is already attached to is allowed, whoever it belongs
//! to now.
//!
//! `GetHandlerPlayer` is doing the work in both halves: it is the player
//! who *played* the Spell, which does not change when the monster does.
//!
//! ## Control is an equip effect, not an action
//!
//! ```lua
//! e4:SetType(EFFECT_TYPE_EQUIP)
//! e4:SetCode(EFFECT_SET_CONTROL)
//! e4:SetValue(s.ctval)   -- returns e:GetHandlerPlayer()
//! ```
//!
//! Nothing calls `Duel.GetControl`. An `EFFECT_TYPE_EQUIP` effect applies
//! to whatever the card is attached to, and `EFFECT_SET_CONTROL` is read
//! by `refresh_control_status` — so control follows the attachment, and
//! **lapses by itself** when the Spell leaves. That is why Snatch Steal
//! gives the monster back rather than needing an effect that says so.
//!
//! ## The recovery is a forced trigger on the opponent's Standby Phase
//!
//! `EFFECT_TYPE_FIELD + EFFECT_TYPE_TRIGGER_F`, ranged to the Spell &
//! Trap row, count-limited to one, conditional on it being the
//! opponent's turn. The 1000 life goes to `1-tp` — the player whose
//! monster was taken — and travels on the chain link as a target player
//! and parameter rather than as an argument, the same indirection
//! Delinquent Duo uses.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::{card_type, reason};
use crate::duel::{flags, phases};
use crate::effect::{effect_type, Ctx, Effect, Yield};
use crate::event::{category, code, CardId, EffectId};
use crate::field::Field;
use crate::script_api as api;

use super::proc_equip::{add_equip_procedure, EquipProcedure, Side};

pub const CODE: u32 = 45_986_603;

/// The life the opponent gets back each of their Standby Phases.
const RECOVER_AMOUNT: i32 = 1000;

pub fn initial_effect(f: &mut Field, c: CardId) {
    add_equip_procedure(
        f,
        c,
        EquipProcedure {
            side: Side::Opponent,
            filter: Some(check_steal_equip),
            eqlimit,
            after_target: Some(after_target),
        },
    );
    // recover
    let e2 = api::create_effect(f, c);
    api::set_description(f, e2, api::stringid(CODE, 0));
    api::set_type(f, e2, effect_type::FIELD | effect_type::TRIGGER_F);
    api::set_category(f, e2, category::RECOVER);
    api::set_code(f, e2, code::PHASE | u32::from(phases::STANDBY));
    api::set_range(f, e2, u16::from(location::SZONE));
    api::set_count_limit(f, e2, 1, 0, 0);
    api::set_condition(f, e2, reccon);
    api::set_target(f, e2, rectg);
    api::set_operation(f, e2, recop);
    api::register_effect(f, c, e2, false);
    // control
    let e4 = api::create_effect(f, c);
    api::set_type(f, e4, effect_type::EQUIP);
    api::set_code(f, e4, code::SET_CONTROL);
    api::set_value_fn(f, e4, ctval);
    api::register_effect(f, c, e4, false);
}

/// `aux.CheckStealEquip` (`cards_specific_functions.lua:405`) — the
/// procedure's filter.
///
/// Four tests, and the third is a configuration the Goat field does not
/// have: a **trap monster** occupies a Spell & Trap seat as well, so
/// taking one needs a seat in that row too — one to put it in, and the
/// count of two because the equip card itself needs one. Transcribed and
/// unreachable here, like Creature Swap's extra-zone clause.
///
/// The second test is the interesting exit: once the equip card is
/// already **in the Spell & Trap row** the answer is unconditionally
/// yes, because by then the core is enforcing the seat arithmetic
/// itself. The comment in the reference says exactly that.
fn check_steal_equip(f: &mut Field, c: CardId, e: EffectId, tp: u8) -> bool {
    if api::is_facedown(f, c) || !api::is_controler_can_be_changed(f, c, false) {
        return false;
    }
    if !api::is_controler(f, c, 1 - tp) {
        return false;
    }
    let Some(handler) = api::get_handler(f, e) else {
        return false;
    };
    if api::is_location(f, handler, u16::from(location::SZONE)) {
        // Already handled in the core.
        return true;
    }
    if !api::is_duel_type(f, flags::TRAP_MONSTERS_NOT_USE_ZONE)
        && api::is_type(f, c, card_type::TRAPMONSTER)
    {
        return api::get_location_count_for(
            f,
            tp,
            location::SZONE,
            tp,
            api::LOCATION_REASON_CONTROL,
        ) > 0
            && api::get_location_count_for(f, tp, location::SZONE, tp, 0) >= 2;
    }
    true
}

/// `s.eqlimit` — not my own monster, **or** the one I am already on.
///
/// `ctx.reason_effect` is this effect: the value seam is entered with the
/// effect being asked about, so the script's `e` and the context's
/// effect are the same thing.
fn eqlimit(_e: &Effect, f: &Field, ctx: &Ctx) -> i64 {
    let Some(c) = ctx.card else {
        return 0;
    };
    let e = ctx.reason_effect;
    if api::get_handler_player(f, e) != api::get_controler(f, c) {
        return 1;
    }
    let attached = api::get_handler(f, e).and_then(|h| api::get_equip_target(f, h));
    i64::from(attached == Some(c))
}

/// `s.target` — the procedure's `tg` hook, run once the monster is
/// chosen.
///
/// Two things, and the first is why the hook exists: the category is
/// **rewritten** on the effect, from the procedure's plain `EQUIP` to
/// control-and-equip. An announcement made before the target was known
/// could not have said that.
fn after_target(f: &mut Field, ctx: &Ctx, tc: CardId) {
    api::set_category(f, ctx.reason_effect, category::CONTROL | category::EQUIP);
    api::set_operation_info(f, 0, category::CONTROL, Some(vec![tc]), 1, 0, 0);
}

/// `s.ctval` — control goes to whoever played the Spell.
fn ctval(_e: &Effect, f: &Field, ctx: &Ctx) -> i64 {
    i64::from(api::get_handler_player(f, ctx.reason_effect))
}

/// `s.reccon` — only on the opponent's turn.
fn reccon(f: &mut Field, ctx: &Ctx) -> bool {
    api::is_turn_player(f, 1 - ctx.player)
}

fn rectg(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        return api::yes(true);
    }
    api::set_target_player(f, 1 - tp);
    api::set_target_param(f, RECOVER_AMOUNT);
    api::set_operation_info(f, 0, category::RECOVER, None, 0, 1 - tp, RECOVER_AMOUNT);
    api::yes(true)
}

fn recop(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return api::done();
    };
    if !api::is_relate_to_effect(f, c, ctx.reason_effect) {
        return api::done();
    }
    // The player and the figure come off the chain link, not from `tp`.
    let Some((p, d)) = api::get_chain(f, 0).map(|ch| (ch.target_player, ch.target_param)) else {
        return api::done();
    };
    api::recover(f, p, i64::from(d), reason::EFFECT);
    api::suspend(|_f, _ctx| api::done())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{status, Card, CardData};
    use crate::chain::Chain;
    use crate::event::Event;
    use crate::field::Message;
    use crate::host_question::{hint, hintmsg};
    use crate::processor::{Kind, Status};

    fn monster_at(f: &mut Field, player: u8, code_: u32, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1700,
                defense: 1000,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::MZONE, seq, false);
        f.cards[id].current.position = position::FACEUP_ATTACK;
        id
    }

    /// `tp`'s Main Phase 1 with the Spell face-up in its row, and `mine` /
    /// `theirs` monsters in the two Monster Zones.
    fn field_as(tp: u8, mine: u32, theirs: u32) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let ss = f.new_card(d);
        f.add_card(tp, ss, location::SZONE, 0, false);
        f.cards[ss].current.position = position::FACEUP;
        f.initialize_card(ss);
        let ours = (0..mine)
            .map(|i| monster_at(&mut f, tp, 7_000 + i, i))
            .collect();
        let theirs = (0..theirs)
            .map(|i| monster_at(&mut f, 1 - tp, 8_000 + i, i))
            .collect();
        (f, ss, ours, theirs)
    }

    fn field(mine: u32, theirs: u32) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        field_as(0, mine, theirs)
    }

    fn activate_effect(f: &Field, ss: CardId) -> EffectId {
        f.cards[ss].field_effect.equal_range(code::FREE_CHAIN)[0]
    }
    fn limit_effect(f: &Field, ss: CardId) -> EffectId {
        f.cards[ss].single_effect.equal_range(code::EQUIP_LIMIT)[0]
    }
    fn recover_effect(f: &Field, ss: CardId) -> EffectId {
        f.cards[ss]
            .field_effect
            .equal_range(code::PHASE | u32::from(phases::STANDBY))[0]
    }
    fn control_effect(f: &Field, ss: CardId) -> EffectId {
        f.cards[ss].equip_effect.equal_range(code::SET_CONTROL)[0]
    }

    fn ctx_for<'a>(e: EffectId, ev: &'a Event, tp: u8) -> Ctx<'a> {
        Ctx {
            reason_effect: e,
            player: tp,
            event: ev,
            card: None,
            args: &[],
        }
    }

    fn ask_target(f: &mut Field, e: EffectId, tp: u8, chkc: Option<CardId>) -> bool {
        f.core.reason_effect = Some(e);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = ctx_for(e, &ev, tp);
        let t = f.effects.get(e).and_then(|x| x.target).expect("a target");
        t(f, &ctx, false, chkc).finished().unwrap_or(0) != 0
    }

    #[derive(Default)]
    struct Run {
        asked: Vec<(u8, Vec<CardId>)>,
        hints: Vec<(u8, u8, u64)>,
    }

    fn resolve_as(f: &mut Field, tp: u8, e: EffectId, pick: usize) -> Run {
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        let handler = api::get_handler(f, e).expect("an activating card");
        f.cards[handler].create_chain_relation(e, 11);
        f.core
            .sub_solving_event
            .push_back(Event::new(code::FREE_CHAIN));
        f.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: e,
            player: tp,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        let mut run = Run::default();
        let mut seen = 0usize;
        let mut operated = false;
        for _ in 0..8192 {
            while seen < f.messages.len() {
                if let Message::Hint {
                    kind,
                    player,
                    value,
                } = &f.messages[seen]
                {
                    run.hints.push((*kind, *player, *value));
                }
                seen += 1;
            }
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        if operated {
                            break;
                        }
                        operated = true;
                        f.core
                            .sub_solving_event
                            .push_back(Event::new(code::FREE_CHAIN));
                        f.emplace(Kind::ExecuteOperation {
                            resume: None,
                            effect: e,
                            player: tp,
                            subject: None,
                            args: Vec::new(),
                            was_disabled: false,
                        });
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectCard {
                        player, min, cards, ..
                    }) => {
                        run.asked.push((*player, cards.clone()));
                        let min = usize::from(*min);
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, (pick + i) as i32);
                        }
                    }
                    Some(Message::SelectPlace { player, flag, .. }) => {
                        let (pl, flag) = (*player, *flag);
                        let seq = (0..5u32)
                            .find(|s| flag & (0x100 << s) == 0)
                            .expect("a free spell seat");
                        f.core.returns.set_i8(0, pl as i8);
                        f.core.returns.set_i8(1, location::SZONE as i8);
                        f.core.returns.set_i8(2, seq as i8);
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        run
    }

    fn resolve(f: &mut Field, e: EffectId, pick: usize) -> Run {
        resolve_as(f, 0, e, pick)
    }

    /// **Four printed effects**: the procedure's two, the recovery, and
    /// the control value — and the last is an **equip** effect, not a
    /// single one, so it applies to whatever the card is attached to.
    #[test]
    fn the_script_registers_the_procedure_plus_a_recovery_and_a_control_value() {
        let (f, ss, _, _) = field(0, 1);
        let e1 = f.effects.get(activate_effect(&f, ss)).unwrap();
        assert!(e1.is_type(effect_type::ACTIVATE));
        assert_eq!(e1.category, category::EQUIP, "before a target is chosen");
        assert!(e1.is_flag(crate::effect::flag::CARD_TARGET));
        assert!(e1.is_flag(crate::effect::flag::CONTINUOUS_TARGET));
        assert_eq!(
            e1.description,
            super::super::proc_equip::ACTIVATE_DESCRIPTION
        );

        let lim = f.effects.get(limit_effect(&f, ss)).unwrap();
        assert!(lim.is_type(effect_type::SINGLE));
        assert!(lim.is_flag(crate::effect::flag::CANNOT_DISABLE));

        let rec = f.effects.get(recover_effect(&f, ss)).unwrap();
        assert!(rec.is_type(effect_type::FIELD));
        assert!(rec.is_type(effect_type::TRIGGER_F), "forced, not optional");
        assert_eq!(rec.category, category::RECOVER);
        assert_eq!(rec.range, u16::from(location::SZONE));
        assert_eq!(rec.count_limit, 1);
        assert!(rec.condition.is_some());

        let ctl = f.effects.get(control_effect(&f, ss)).unwrap();
        assert!(
            ctl.is_type(effect_type::EQUIP),
            "an equip effect, so it follows the attachment"
        );
        assert_eq!(ctl.code, code::SET_CONTROL);
    }

    /// **It wants a face-up monster of the opponent's**, and each of the
    /// three clauses gets a board where only it refuses.
    #[test]
    fn it_wants_a_face_up_monster_of_the_opponents() {
        // The positive sibling.
        let (mut f, ss, _, theirs) = field(0, 1);
        let e = activate_effect(&f, ss);
        assert!(ask_target(&mut f, e, 0, Some(theirs[0])));
        assert!(ask_target(&mut f, e, 0, None));

        // Only its own monsters.
        let (mut f, ss, ours, _) = field(1, 0);
        let e = activate_effect(&f, ss);
        assert!(!ask_target(&mut f, e, 0, Some(ours[0])), "its own");
        assert!(!ask_target(&mut f, e, 0, None));

        // Theirs, but face-down.
        let (mut f, ss, _, theirs) = field(0, 1);
        f.cards[theirs[0]].current.position = position::FACEDOWN_DEFENSE;
        let e = activate_effect(&f, ss);
        assert!(!ask_target(&mut f, e, 0, Some(theirs[0])), "face-down");

        // Theirs and face-up, but under a control lock.
        let (mut f, ss, _, theirs) = field(0, 1);
        let mut lock = Effect::new(effect_type::SINGLE, code::CANNOT_CHANGE_CONTROL);
        lock.owner = Some(theirs[0]);
        lock.handler = Some(theirs[0]);
        let id = f.new_effect(lock);
        f.cards[theirs[0]]
            .single_effect
            .insert(code::CANNOT_CHANGE_CONTROL, id);
        f.cards[theirs[0]].indexer.insert(id);
        let e = activate_effect(&f, ss);
        assert!(!ask_target(&mut f, e, 0, Some(theirs[0])), "locked");
    }

    /// **And their side must have room to give the monster up** — which
    /// is `IsControlerCanBeChanged`'s seat test, counted on *our* side,
    /// the side that would receive it.
    #[test]
    fn our_row_must_have_room_to_receive_it() {
        let (mut f, ss, _, theirs) = field(0, 1);
        let e = activate_effect(&f, ss);
        assert!(ask_target(&mut f, e, 0, Some(theirs[0])), "the sibling");
        for i in 0..5 {
            monster_at(&mut f, 0, 9_000 + i, i);
        }
        assert!(
            !ask_target(&mut f, e, 0, Some(theirs[0])),
            "nowhere to put it"
        );
    }

    /// **The same, as player 1** — the side follows `tp`.
    #[test]
    fn the_side_follows_the_activating_player() {
        let (mut f, ss, ours, theirs) = field_as(1, 1, 1);
        let e = activate_effect(&f, ss);
        // Player 1 activating: `ours` is player 1's, `theirs` player 0's.
        assert!(!ask_target(&mut f, e, 1, Some(ours[0])), "its own");
        assert!(ask_target(&mut f, e, 1, Some(theirs[0])), "the opponent's");
    }

    /// **One monster, from the opponent's row, chosen by the activating
    /// player**, prefaced by the equip prompt.
    #[test]
    fn it_asks_its_controller_for_one_of_the_opponents() {
        let (mut f, ss, _, theirs) = field(2, 2);
        let e = activate_effect(&f, ss);
        let run = resolve(&mut f, e, 0);
        assert_eq!(run.asked.len(), 1);
        let (player, offered) = &run.asked[0];
        assert_eq!(*player, 0);
        assert_eq!(offered, &theirs, "only the opponent's, and all of them");
        assert!(run
            .hints
            .iter()
            .any(|&(k, p, v)| (k, p, v) == (hint::SELECTMSG, 0, hintmsg::EQUIP)));
    }

    /// **The announcement gains the control category once the target is
    /// known** — which is what the procedure's `tg` hook is for — and
    /// names the monster for it while naming the Spell for the equip.
    #[test]
    fn the_announcement_gains_control_once_the_target_is_known() {
        let (mut f, ss, _, theirs) = field(0, 2);
        let e = activate_effect(&f, ss);
        let run = resolve(&mut f, e, 0);
        let chosen = run.asked[0].1[0];
        assert_eq!(chosen, theirs[0]);
        assert_eq!(
            f.effects.get(e).unwrap().category,
            category::CONTROL | category::EQUIP,
            "rewritten on the effect"
        );
        let ops = &f.core.current_chain[0].opinfos;
        let ctl = ops.get(&category::CONTROL).expect("the control category");
        assert_eq!(ctl.cards.as_deref(), Some(&[chosen][..]));
        assert_eq!(ctl.count, 1);
        let eq = ops.get(&category::EQUIP).expect("the equip category");
        assert_eq!(eq.cards.as_deref(), Some(&[ss][..]), "the Spell itself");
    }

    /// **It attaches to the opponent's monster and takes control of
    /// it**, and the monster stays in its own seat — control here is an
    /// equip effect, not a move.
    #[test]
    fn it_attaches_and_takes_control() {
        let (mut f, ss, _, theirs) = field(0, 2);
        let e = activate_effect(&f, ss);
        let run = resolve(&mut f, e, 0);
        let chosen = run.asked[0].1[0];
        assert_eq!(f.cards[ss].equiping_target, Some(chosen));
        assert!(f.cards[chosen].equiping_cards.contains(&ss));
        // The control value names the player who played the Spell.
        let ctl = control_effect(&f, ss);
        let ev = Event::new(0);
        let ctx = Ctx {
            reason_effect: ctl,
            player: 0,
            event: &ev,
            card: Some(chosen),
            args: &[],
        };
        let eff = f.effects.get(ctl).unwrap();
        assert_eq!(ctval(eff, &f, &ctx), 0, "player 0 played it");
        assert_eq!(
            f.refresh_control_status(chosen).0,
            0,
            "so the monster answers to player 0"
        );
        assert_eq!(
            f.cards[theirs[1]].current.controller, 1,
            "and only that one"
        );
    }

    /// **The limit's two halves.** Not my own monster, *or* the one I am
    /// already attached to — and the second is what keeps the card on
    /// after the first turns false.
    #[test]
    fn the_limit_exempts_the_monster_it_already_holds() {
        let (mut f, ss, ours, theirs) = field(1, 2);
        let e = activate_effect(&f, ss);
        let run = resolve(&mut f, e, 0);
        let chosen = run.asked[0].1[0];
        let lim = limit_effect(&f, ss);
        let ev = Event::new(0);
        let ask = |f: &Field, c: CardId| {
            let ctx = Ctx {
                reason_effect: lim,
                player: 0,
                event: &ev,
                card: Some(c),
                args: &[],
            };
            eqlimit(f.effects.get(lim).unwrap(), f, &ctx)
        };
        // The control change is an *effect*: `current.controler` does
        // not move until the rules adjust, so the board has to be
        // settled before the first half of the limit can be false.
        settle(&mut f);
        assert_eq!(
            f.cards[chosen].current.controller, 0,
            "the adjust moved it across"
        );
        assert_eq!(ask(&f, chosen), 1, "the one it already holds");
        // Another of the opponent's: the first half carries it.
        assert_eq!(ask(&f, theirs[1]), 1, "still theirs");
        // One of its own that it is not attached to: neither half.
        assert_eq!(ask(&f, ours[0]), 0, "its own, and not the one it holds");
    }

    /// Run the rules out until nothing is queued.
    fn settle(f: &mut Field) {
        f.adjust_all();
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        return;
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectPlace { player, flag, .. }) => {
                        let (pl, flag) = (*player, *flag);
                        let seq = (0..5u32).find(|s| flag & (1 << s) == 0).unwrap_or(0);
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

    /// **The card survives the adjust that follows its own steal** —
    /// which is what the limit's second half is for, and the only way to
    /// see it working.
    ///
    /// Once control has changed the monster is its controller's, so the
    /// first half ("not my own monster") is false. `adjust_equips` asks
    /// the limit once per adjust about the card's *current* target and
    /// destroys any equip card that answers no — so without the
    /// exemption the Spell would take control and then immediately be
    /// destroyed by the rules for holding what it just took.
    #[test]
    fn it_survives_the_adjust_that_follows_its_own_steal() {
        let (mut f, ss, _, _) = field(0, 2);
        let e = activate_effect(&f, ss);
        let run = resolve(&mut f, e, 0);
        let chosen = run.asked[0].1[0];
        settle(&mut f);
        assert_eq!(
            f.cards[chosen].current.controller, 0,
            "control changed, so the first half is now false"
        );
        assert_eq!(
            f.cards[ss].current.location,
            location::SZONE,
            "and the card is still on the row"
        );
        assert_eq!(f.cards[ss].equiping_target, Some(chosen));
    }

    /// **The recovery fires on the opponent's turn only**, gives them
    /// exactly 1000, and carries both on the chain link rather than as
    /// arguments.
    #[test]
    fn the_recovery_goes_to_the_opponent_on_their_turn() {
        let (mut f, ss, _, _) = field(0, 1);
        let e = recover_effect(&f, ss);
        let ev = Event::new(code::PHASE | u32::from(phases::STANDBY));
        // Its own turn: no.
        f.infos.turn_player = 0;
        assert!(!reccon(&mut f, &ctx_for(e, &ev, 0)));
        // The opponent's: yes.
        f.infos.turn_player = 1;
        assert!(reccon(&mut f, &ctx_for(e, &ev, 0)));

        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        f.core.current_chain.push(ch);
        rectg(&mut f, &ctx_for(e, &ev, 0), true, None);
        assert_eq!(f.core.current_chain[0].target_player, 1, "the opponent");
        assert_eq!(f.core.current_chain[0].target_param, 1000);
        let info = f.core.current_chain[0]
            .opinfos
            .get(&category::RECOVER)
            .expect("the recover category");
        assert_eq!(info.player, 1);
        assert_eq!(info.param, 1000);
    }

    /// **And it is paid to whoever the chain link names**, not to `tp` —
    /// the indirection is the script's, and the effect is registered by
    /// one player while the life goes to the other.
    #[test]
    fn the_recovery_pays_the_player_the_chain_named() {
        let (mut f, ss, _, _) = field(0, 1);
        let e = recover_effect(&f, ss);
        f.cards[ss].create_chain_relation(e, 11);
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        ch.target_player = 1;
        ch.target_param = 1000;
        f.core.current_chain.push(ch);
        let ev = Event::new(code::PHASE | u32::from(phases::STANDBY));
        recop(&mut f, &ctx_for(e, &ev, 0));
        match &f.core.subunits[0].kind {
            Kind::Recover { arg } => {
                assert_eq!(arg.playerid, 1, "the opponent, not the controller");
                assert_eq!(arg.amount, 1000);
            }
            other => panic!("expected a Recover: {other:?}"),
        }
    }

    /// **A Spell that has left the field pays nothing.**
    #[test]
    fn a_spell_no_longer_related_pays_nothing() {
        let (mut f, ss, _, _) = field(0, 1);
        let e = recover_effect(&f, ss);
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        ch.target_player = 1;
        ch.target_param = 1000;
        f.core.current_chain.push(ch);
        // No chain relation created: the card is not related.
        let ev = Event::new(code::PHASE | u32::from(phases::STANDBY));
        recop(&mut f, &ctx_for(e, &ev, 0));
        assert!(f.core.subunits.is_empty());
    }

    /// A duel with `DUEL_TRAP_MONSTERS_NOT_USE_ZONE` **off**, which is
    /// what the filter's third clause is gated on — and which the
    /// harness configuration turns *on*, so the clause is unreachable
    /// there for that reason rather than for want of trap monsters.
    fn field_where_trap_monsters_take_a_seat() -> Field {
        let flags = crate::duel::REFERENCE_CONFIGURATION & !flags::TRAP_MONSTERS_NOT_USE_ZONE;
        Field::with_flags(8000, flags)
    }

    /// A trap monster of player 1's, face-up in their first seat.
    fn trap_monster(f: &mut Field, code_: u32, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::TRAP | card_type::TRAPMONSTER | card_type::MONSTER,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            1,
        );
        c.current.controller = 1;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(1, id, location::MZONE, seq, false);
        f.cards[id].current.position = position::FACEUP_ATTACK;
        id
    }

    /// Fill `n` of player 0's Spell & Trap seats, from seat `from`.
    fn fill_spell_row(f: &mut Field, from: u32, n: u32) {
        for i in from..from + n {
            let mut x = Card::with_data(
                CardData {
                    code: 9_100 + i,
                    type_: card_type::SPELL,
                    ..Default::default()
                },
                0,
            );
            x.current.controller = 0;
            let id = f.new_card(x);
            f.add_card(0, id, location::SZONE, i, false);
        }
    }

    /// **The filter's trap-monster branch.** A trap monster occupies a
    /// Spell & Trap seat as well, so taking one needs a seat in that row
    /// too — one for the monster and one for the equip card, hence the
    /// count of two.
    ///
    /// Gated on `DUEL_TRAP_MONSTERS_NOT_USE_ZONE` being **off**, which
    /// is why the harness never reaches it: the reference configuration
    /// turns that option on. Pinned here with it off, the same way
    /// Creature Swap's extra-zone clause is pinned.
    #[test]
    fn a_trap_monster_needs_two_spell_and_trap_seats() {
        // The equip card **off** the Spell & Trap row, so the shortcut
        // above does not answer first.
        let mut f = field_where_trap_monsters_take_a_seat();
        f.infos.turn_id = 3;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), 0);
        d.current.controller = 0;
        d.set_status(status::EFFECT_ENABLED, true);
        let ss = f.new_card(d);
        f.add_card(0, ss, location::HAND, 0, false);
        f.initialize_card(ss);
        let e = activate_effect(&f, ss);

        let tm = trap_monster(&mut f, 9_001, 0);

        // Five free Spell & Trap seats: plenty.
        assert!(check_steal_equip(&mut f, tm, e, 0), "room for both");
        // Four occupied leaves one, which is one short of the two the
        // branch wants.
        fill_spell_row(&mut f, 0, 4);
        assert!(!check_steal_equip(&mut f, tm, e, 0), "one seat is not two");

        // And an ordinary monster on the same board is fine, because the
        // branch is about trap monsters only.
        let ordinary = monster_at(&mut f, 1, 8_900, 1);
        assert!(check_steal_equip(&mut f, ordinary, e, 0));
    }

    /// **The Spell & Trap shortcut.** Once the equip card is already in
    /// the row, the seat arithmetic is the core's problem and the filter
    /// answers yes without asking — which is the reference's own comment.
    ///
    /// The same board with the card **in the hand** refuses, which is
    /// what makes this about the shortcut rather than about the seats.
    #[test]
    fn a_card_already_in_the_row_skips_the_seat_arithmetic() {
        let mut f = field_where_trap_monsters_take_a_seat();
        f.infos.turn_id = 3;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), 0);
        d.current.controller = 0;
        d.set_status(status::EFFECT_ENABLED, true);
        let ss = f.new_card(d);
        f.add_card(0, ss, location::SZONE, 0, false);
        f.cards[ss].current.position = position::FACEUP;
        f.initialize_card(ss);
        let e = activate_effect(&f, ss);
        let tm = trap_monster(&mut f, 9_001, 0);
        // One seat left, which is enough for the *first* guard and one
        // short of the two the arithmetic branch wants — so the branch
        // would refuse and the shortcut does not reach it.
        fill_spell_row(&mut f, 1, 3);
        assert!(
            check_steal_equip(&mut f, tm, e, 0),
            "already in the row, so the core handles it"
        );

        // The same board, with the card in the hand instead.
        let mut f = field_where_trap_monsters_take_a_seat();
        f.infos.turn_id = 3;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), 0);
        d.current.controller = 0;
        d.set_status(status::EFFECT_ENABLED, true);
        let ss = f.new_card(d);
        f.add_card(0, ss, location::HAND, 0, false);
        f.initialize_card(ss);
        let e = activate_effect(&f, ss);
        let tm = trap_monster(&mut f, 9_001, 0);
        fill_spell_row(&mut f, 0, 4);
        assert!(
            !check_steal_equip(&mut f, tm, e, 0),
            "not in the row, so the seats are counted — and one is not two"
        );
    }

    /// **The filter's own two guards**, asked directly.
    ///
    /// The procedure checks face-up and the side as well, so on a driven
    /// activation these two are masked — a redundancy the reference has
    /// too. Calling the filter on its own is the only way to see them,
    /// and they are worth seeing because the filter is what a *future*
    /// caller would reuse.
    #[test]
    fn the_filter_wants_a_face_up_monster_of_the_opponents() {
        let (mut f, ss, ours, theirs) = field(1, 1);
        let e = activate_effect(&f, ss);
        assert!(
            check_steal_equip(&mut f, theirs[0], e, 0),
            "the positive sibling"
        );
        assert!(!check_steal_equip(&mut f, ours[0], e, 0), "its own monster");
        f.cards[theirs[0]].current.position = position::FACEDOWN_DEFENSE;
        assert!(!check_steal_equip(&mut f, theirs[0], e, 0), "face-down");
    }

    /// **The trap-monster branch counts seats *for a control change*,
    /// not for putting a card down.**
    ///
    /// The two agree on an ordinary board, so telling them apart needs a
    /// cap that applies to one reason and not the other — which is the
    /// only thing that makes the argument observable at all.
    #[test]
    fn the_trap_monster_count_is_for_a_control_change() {
        let mut f = field_where_trap_monsters_take_a_seat();
        f.infos.turn_id = 3;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), 0);
        d.current.controller = 0;
        d.set_status(status::EFFECT_ENABLED, true);
        let ss = f.new_card(d);
        f.add_card(0, ss, location::HAND, 0, false);
        f.initialize_card(ss);
        let e = activate_effect(&f, ss);
        let tm = trap_monster(&mut f, 9_001, 0);
        assert!(check_steal_equip(&mut f, tm, e, 0), "the positive sibling");

        // A Spell & Trap cap of zero, but only for a control change.
        let mut source = Card::new(1, 0);
        source.current.controller = 0;
        source.current.location = location::MZONE;
        source.current.position = position::FACEUP_ATTACK;
        source.set_status(status::EFFECT_ENABLED, true);
        let source = f.new_card(source);
        let mut cap = Effect::new(effect_type::FIELD, code::MAX_SZONE);
        cap.owner = Some(source);
        cap.handler = Some(source);
        cap.effect_owner = 0;
        cap.flag[0] |= crate::effect::flag::PLAYER_TARGET | crate::effect::flag::FUNC_VALUE;
        cap.range = u16::from(location::MZONE);
        cap.s_range = u16::from(location::MZONE);
        cap.value_fn = Some(nothing_for_a_control_change);
        let id = f.new_effect(cap);
        f.field_effects.aura.insert(code::MAX_SZONE, id);
        f.field_effects.indexer.insert(id);

        assert!(
            !check_steal_equip(&mut f, tm, e, 0),
            "no seats for a control change, whatever the other count says"
        );
    }

    /// A Spell & Trap cap of zero for a control change **asked by
    /// player 0**, and five otherwise. `args` is `(playerid, uplayer,
    /// reason)`.
    ///
    /// The `uplayer` clause is what keeps the cap off the *first* guard.
    /// `is_controler_can_be_changed` counts the same row for the same
    /// reason but on behalf of the monster's own controller — player 1
    /// here — so a cap keyed only on the reason would refuse there and
    /// the branch under test would never be reached.
    fn nothing_for_a_control_change(_e: &Effect, _f: &Field, ctx: &Ctx) -> i64 {
        let control = ctx.args.get(2) == Some(&i64::from(api::LOCATION_REASON_CONTROL));
        if control && ctx.args.get(1) == Some(&0) {
            0
        } else {
            5
        }
    }
}
