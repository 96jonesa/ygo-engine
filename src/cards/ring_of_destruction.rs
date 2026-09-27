//! Ring of Destruction — `c83555666.lua`.
//!
//! The twenty-ninth card. It destroys a face-up monster the opponent
//! controls and burns **both** players for that monster's printed
//! attack — the controller first, and the opponent only if the
//! controller survived it.
//!
//! ## The filter depends on how much life the opponent has
//!
//! ```lua
//! local lp = Duel.GetLP(1-tp)
//! function s.filter(c,lp) return c:IsFaceup() and c:IsAttackBelow(lp) end
//! ```
//!
//! A monster is only a legal target if its attack is **at or below the
//! opponent's life total** — the card refuses to name something that
//! would kill the opponent outright. So the legal set shrinks as the
//! opponent's life does, and the same board offers different targets at
//! different times. The bound comes in as an extra argument in the
//! reference and as a closure here.
//!
//! ## The damage is the *printed* attack
//!
//! `tc:GetTextAttack()`, not `GetAttack()`. A monster pumped to 3000 by
//! an equip still burns for what its card says. Reading the effective
//! attack would be a plausible and wrong translation, and on a board with
//! no modifiers the two agree — which is exactly why it needs a test
//! where they do not.
//!
//! The reference then clamps a negative to zero, which `Duel.Damage`
//! would do anyway; both are kept because both are the reference's.
//!
//! ## The second burn is conditional on surviving the first
//!
//! ```lua
//! local val = Duel.Damage(tp,atk,REASON_EFFECT)
//! if val>0 and Duel.GetLP(tp)>0 then
//!     Duel.BreakEffect()
//!     Duel.Damage(1-tp,val,REASON_EFFECT)
//! end
//! ```
//!
//! Two reads of a result, not one. `val` is how much damage was
//! *actually* dealt — a replacement effect may have changed it, and the
//! opponent takes that figure rather than the printed one. And the
//! controller has to still be alive: a Ring that kills its own user does
//! not go on to burn the opponent.
//!
//! ## Once per turn, and refunded if negated
//!
//! `SetCountLimit(1, id, EFFECT_COUNT_CODE_OATH)`. The oath flag is what
//! gives the use back when the activation is negated — without it a
//! negated Ring would still have spent the turn's use.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_count, effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId, PLAYER_ALL};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 83_555_666;

/// `TIMINGS_CHECK_MONSTER_E` (`constant.lua:902`).
const TIMINGS_CHECK_MONSTER_E: u32 = 0x1e0;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::DESTROY | category::DAMAGE);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_property(f, e1, flag::CARD_TARGET, 0);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_hint_timing(f, e1, 0, TIMINGS_CHECK_MONSTER_E);
    api::set_count_limit(f, e1, 1, CODE, effect_count::OATH);
    api::set_condition(f, e1, condition);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

const MZONE: u32 = location::MZONE as u32;

fn condition(f: &mut Field, ctx: &Ctx) -> bool {
    api::is_turn_player(f, 1 - ctx.player)
}

/// `s.filter(c, lp)` — as a closure over the life total, which is what
/// the reference passes along the scan.
fn filter_for(lp: i32) -> impl Fn(&mut Field, CardId) -> bool {
    move |f: &mut Field, c: CardId| api::is_faceup(f, c) && api::is_attack_below(f, c, lp)
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    let lp = api::get_lp(f, 1 - tp);
    let filter = filter_for(lp);
    if let Some(chkc) = chkc {
        return api::yes(
            api::is_location(f, chkc, u16::from(location::MZONE))
                && api::is_controler(f, chkc, 1 - tp)
                && filter(f, chkc),
        );
    }
    if !chk {
        return api::yes(api::is_existing_target(
            f,
            Some(&filter),
            tp,
            0,
            MZONE,
            1,
            api::Except::None,
        ));
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::DESTROY);
    api::select_target(f, tp, Some(&filter), tp, 0, MZONE, 1, 1, api::Except::None);
    api::suspend(move |f, _ctx| {
        if let Some(g) = api::selected_targets(f) {
            api::set_operation_info(f, 0, category::DESTROY, Some(g), 1, 0, 0);
        }
        // Both players are burned, and by how much is not known yet.
        api::set_operation_info(f, 0, category::DAMAGE, None, 0, PLAYER_ALL, 0);
        api::yes(true)
    })
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if !api::is_relate_to_effect(f, tc, ctx.reason_effect) || !api::is_faceup(f, tc) {
        return api::done();
    }
    api::destroy(f, vec![tc], reason::EFFECT);
    api::suspend(move |f, ctx| {
        if api::resumed_value(f) == 0 {
            return api::done();
        }
        let tp = ctx.player;
        // What the card *says*, not what it is worth.
        // `if atk<0 then atk=0` — the script's, and belt-and-braces:
        // `Duel.Damage` clamps a negative to zero itself. Both are the
        // reference's and both are kept; nothing in this pool has a
        // negative printed attack to tell them apart.
        let atk = api::get_text_attack(f, tc).max(0);
        api::damage(f, tp, i64::from(atk), reason::EFFECT);
        api::suspend(move |f, ctx| {
            let tp = ctx.player;
            // How much was actually dealt, and whether its user lived.
            let val = api::resumed_value(f);
            if val <= 0 || api::get_lp(f, tp) <= 0 {
                return api::done();
            }
            api::break_effect(f);
            api::damage(f, 1 - tp, i64::from(val), reason::EFFECT);
            api::suspend(|_, _| api::done())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::{EffectId, Event};
    use crate::field::timing;
    use crate::field::Message;
    use crate::position;
    use crate::processor::{Kind, Status};

    fn monster(f: &mut Field, owner: u8, seat: u32, attack: i32, faceup: bool) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 7_000 + seat + u32::from(owner) * 100,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, location::MZONE, seat, false);
        f.cards[id].current.position = if faceup {
            position::FACEUP_ATTACK
        } else {
            position::FACEDOWN_DEFENSE
        };
        let fid = f.next_field_id_raw();
        f.cards[id].fieldid = fid;
        f.cards[id].fieldid_r = fid;
        id
    }

    /// The opponent's Main Phase 1 with the Ring face-up in `tp`'s row,
    /// and `theirs` monsters of the opponent's as `(attack, face-up)`.
    fn field_as(tp: u8, theirs: &[(i32, bool)]) -> (Field, CardId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 1 - tp;
        f.infos.phase = phases::MAIN1;
        let mut r = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        r.current.controller = tp;
        r.set_status(status::EFFECT_ENABLED, true);
        let rod = f.new_card(r);
        f.add_card(tp, rod, location::SZONE, 0, false);
        f.cards[rod].current.position = position::FACEUP;
        f.initialize_card(rod);
        let board = theirs
            .iter()
            .enumerate()
            .map(|(i, &(atk, up))| monster(&mut f, 1 - tp, i as u32, atk, up))
            .collect();
        (f, rod, board)
    }

    fn field(theirs: &[(i32, bool)]) -> (Field, CardId, Vec<CardId>) {
        field_as(0, theirs)
    }

    fn effect_of(f: &Field, rod: CardId) -> EffectId {
        f.cards[rod].field_effect.equal_range(code::FREE_CHAIN)[0]
    }

    fn ctx_for(e: EffectId, ev: &Event, tp: u8) -> Ctx<'_> {
        Ctx {
            reason_effect: e,
            player: tp,
            event: ev,
            card: None,
            args: &[],
        }
    }

    fn asks(f: &mut Field, e: EffectId, tp: u8, chkc: Option<CardId>) -> bool {
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = ctx_for(e, &ev, tp);
        f.core.reason_effect = Some(e);
        f.core.reason_player = tp;
        let answer = target(f, &ctx, false, chkc).finished().unwrap_or(0) != 0;
        f.core.reason_effect = None;
        answer
    }

    struct Run {
        offered: Vec<Vec<CardId>>,
        damages: Vec<(u8, u32)>,
    }

    fn resolve_as(f: &mut Field, tp: u8, e: EffectId, choice: usize) -> Run {
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
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
        let mut run = Run {
            offered: Vec::new(),
            damages: Vec::new(),
        };
        let mut operated = false;
        for _ in 0..8192 {
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
                    Some(Message::SelectCard { min, cards, .. }) => {
                        let min = usize::from(*min);
                        run.offered.push(cards.clone());
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, (choice + i) as i32);
                        }
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        for m in &f.messages {
            if let Message::Damage { player, amount } = m {
                run.damages.push((*player, *amount));
            }
        }
        run
    }

    fn resolve(f: &mut Field, e: EffectId, choice: usize) -> Run {
        resolve_as(f, 0, e, choice)
    }

    /// **One printed activate effect that targets**, once per turn with
    /// the oath flag, on the opponent's turn only.
    #[test]
    fn the_script_registers_a_once_per_turn_activate_effect() {
        let (f, rod, _) = field(&[]);
        let ids = f.cards[rod].field_effect.equal_range(code::FREE_CHAIN);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(e.is_flag(flag::CARD_TARGET));
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert_eq!(e.category, category::DESTROY | category::DAMAGE);
        assert!(e.is_flag(crate::effect::flag::COUNT_LIMIT));
        assert_eq!(e.count_code, CODE);
        assert!(
            e.count_flag & effect_count::OATH != 0,
            "the use comes back if the activation is negated"
        );
        assert!(e.condition.is_some());
        assert_eq!(e.hint_timing, [0, TIMINGS_CHECK_MONSTER_E]);
    }

    /// **The condition is the opponent's turn.**
    #[test]
    fn the_condition_wants_the_opponents_turn() {
        let (mut f, rod, _) = field(&[]);
        let e = effect_of(&f, rod);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = ctx_for(e, &ev, 0);
        assert!(condition(&mut f, &ctx), "the opponent's turn");
        f.infos.turn_player = 0;
        assert!(!condition(&mut f, &ctx), "not on its own");
    }

    /// **A monster is only a target if its attack is at or below the
    /// opponent's life.** The bound moves as their life does.
    #[test]
    fn the_target_bound_is_the_opponents_life() {
        let (mut f, rod, board) = field(&[(2000, true)]);
        let e = effect_of(&f, rod);
        assert!(asks(&mut f, e, 0, None), "8000 life, a 2000 monster");
        assert!(asks(&mut f, e, 0, Some(board[0])));

        f.players[1].lp = 2000;
        assert!(asks(&mut f, e, 0, None), "exactly enough is enough");
        f.players[1].lp = 1999;
        assert!(
            !asks(&mut f, e, 0, None),
            "a monster that would kill them outright is not a legal target"
        );
        assert!(!asks(&mut f, e, 0, Some(board[0])));
        // And it is *their* life, not mine.
        f.players[1].lp = 8000;
        f.players[0].lp = 1;
        assert!(asks(&mut f, e, 0, None), "my own life does not bound it");
    }

    /// **The target is a face-up monster of the opponent's**, and each
    /// clause refuses on its own.
    #[test]
    fn the_third_question_wants_a_face_up_monster_of_theirs() {
        let (mut f, rod, board) = field(&[(1000, true), (1000, false)]);
        let mine = monster(&mut f, 0, 3, 1000, true);
        let e = effect_of(&f, rod);
        assert!(asks(&mut f, e, 0, Some(board[0])), "theirs, face-up");
        assert!(!asks(&mut f, e, 0, Some(board[1])), "face-down is not");
        assert!(
            !asks(&mut f, e, 0, Some(mine)),
            "and mine is the wrong side"
        );
    }

    /// **Neither scan reaches its own row.** A face-up monster of the
    /// activating player's is not a legal target and is not offered —
    /// the `s` mask is zero in both places.
    #[test]
    fn neither_scan_reaches_its_own_row() {
        let (mut f, rod, _) = field(&[]);
        let mine = monster(&mut f, 0, 0, 1000, true);
        let e = effect_of(&f, rod);
        assert!(
            !asks(&mut f, e, 0, None),
            "the only face-up monster is mine, and it does not count"
        );
        // With one of theirs as well, only theirs is offered.
        let theirs = monster(&mut f, 1, 1, 1000, true);
        assert!(asks(&mut f, e, 0, None));
        let run = resolve(&mut f, e, 0);
        assert_eq!(run.offered[0], vec![theirs], "theirs only");
        assert_eq!(
            f.cards[mine].current.location,
            location::MZONE,
            "and mine is untouched"
        );
    }

    /// **The third question wants the monster row.** A face-up card of
    /// the opponent's in the Spell/Trap row is the wrong location.
    #[test]
    fn the_third_question_wants_the_monster_row() {
        let (mut f, rod, board) = field(&[(1000, true)]);
        // A Spell is refused by the *filter* — `IsAttackBelow` guards on
        // being a monster at all — so it cannot tell the location clause
        // apart. What can: a monster of theirs in the **graveyard**,
        // which is face-up, has an attack under their life, and is
        // theirs. Only the location refuses it.
        let mut buried = Card::with_data(
            CardData {
                code: 7_900,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            1,
        );
        buried.current.controller = 1;
        buried.set_status(status::EFFECT_ENABLED, true);
        let buried = f.new_card(buried);
        f.add_card(1, buried, location::GRAVE, 0, false);
        f.cards[buried].current.position = position::FACEUP;
        let e = effect_of(&f, rod);
        assert!(
            asks(&mut f, e, 0, Some(board[0])),
            "the monster on the field"
        );
        f.core.reason_player = 0;
        assert!(
            api::is_faceup(&f, buried) && api::is_attack_below(&mut f, buried, 8000),
            "the buried one passes the filter"
        );
        assert!(api::is_controler(&f, buried, 1), "and it is the opponent's");
        assert!(
            !asks(&mut f, e, 0, Some(buried)),
            "so only the location clause can be refusing it"
        );
    }

    /// **A target that lost its relation, or was turned face-down, is
    /// spared** — two separate re-checks at resolution.
    #[test]
    fn a_target_that_changed_is_spared() {
        for turn_down in [false, true] {
            let (mut f, rod, board) = field(&[(1500, true)]);
            let e = effect_of(&f, rod);
            let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
            ch.triggering_player = 0;
            ch.chain_count = 1;
            ch.chain_id = 11;
            ch.target_cards = vec![board[0]];
            f.core.current_chain.push(ch);
            if turn_down {
                // Related, but turned face-down in between.
                f.cards[board[0]].create_chain_relation(e, 11);
                f.cards[board[0]].current.position = position::FACEDOWN_DEFENSE;
            }
            // Otherwise: still face-up, but never related.
            f.core
                .sub_solving_event
                .push_back(Event::new(code::FREE_CHAIN));
            f.emplace(Kind::ExecuteOperation {
                resume: None,
                effect: e,
                player: 0,
                subject: None,
                args: Vec::new(),
                was_disabled: false,
            });
            for _ in 0..4096 {
                match f.process() {
                    Status::Continue => {
                        if f.core.units.is_empty() && f.core.subunits.is_empty() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            assert_eq!(
                f.cards[board[0]].current.location,
                location::MZONE,
                "turn_down = {turn_down}"
            );
            assert_eq!(f.players[0].lp, 8000, "and nobody was burned");
            assert_eq!(f.players[1].lp, 8000);
        }
    }

    /// **The second burn breaks the timing window.** `Duel.BreakEffect`
    /// between the two, so the opponent's burn is not part of the same
    /// timing as the controller's.
    #[test]
    fn the_second_burn_breaks_the_window() {
        let (mut f, rod, _) = field(&[(1500, true)]);
        let e = effect_of(&f, rod);
        f.core.hint_timing = [timing::MAIN_END, timing::MAIN_END];
        resolve(&mut f, e, 0);
        assert_eq!(
            f.core.hint_timing[0] & timing::MAIN_END,
            0,
            "Duel.BreakEffect cleared the window between the two burns"
        );
    }

    /// **It destroys the target and burns both players for the printed
    /// attack** — the controller first, then the opponent.
    #[test]
    fn it_destroys_and_burns_both_players() {
        let (mut f, rod, board) = field(&[(1800, true)]);
        let e = effect_of(&f, rod);
        let run = resolve(&mut f, e, 0);
        assert_eq!(f.cards[board[0]].current.location, location::GRAVE);
        assert!(f.cards[board[0]].reason & reason::EFFECT != 0);
        assert_eq!(
            run.damages,
            vec![(0u8, 1800u32), (1u8, 1800u32)],
            "the controller first, then the opponent, both for 1800"
        );
        assert_eq!(f.players[0].lp, 8000 - 1800);
        assert_eq!(f.players[1].lp, 8000 - 1800);
    }

    /// **The burn is the *printed* attack, not the current one.**
    ///
    /// A monster pumped by an effect still burns for what its card says.
    /// On a board with no modifiers the two agree, which is why this
    /// needs a board where they do not.
    #[test]
    fn the_burn_is_the_printed_attack() {
        let (mut f, rod, board) = field(&[(1200, true)]);
        // Currently worth 2600, printed 1200.
        f.cards[board[0]]
            .assume
            .insert(crate::card::assume::ATTACK, 2600);
        assert_eq!(api::get_attack(&mut f, board[0]), 2600);
        assert_eq!(api::get_text_attack(&f, board[0]), 1200);
        let e = effect_of(&f, rod);
        let run = resolve(&mut f, e, 0);
        assert_eq!(
            run.damages,
            vec![(0u8, 1200u32), (1u8, 1200u32)],
            "what the card says, not what it is worth"
        );
    }

    /// **A Ring that kills its own user does not burn the opponent.**
    #[test]
    fn a_ring_that_kills_its_user_stops_there() {
        let (mut f, rod, board) = field(&[(1800, true)]);
        f.players[0].lp = 1800;
        let e = effect_of(&f, rod);
        let run = resolve(&mut f, e, 0);
        assert_eq!(f.cards[board[0]].current.location, location::GRAVE);
        assert_eq!(
            run.damages,
            vec![(0u8, 1800u32)],
            "only the controller was burned"
        );
        assert_eq!(f.players[0].lp, 0);
        assert_eq!(f.players[1].lp, 8000, "the opponent is untouched");
    }

    /// **A destruction that did not happen burns nobody.**
    #[test]
    fn a_destruction_that_did_nothing_burns_nobody() {
        fn immune(_: &crate::effect::Effect, _: &Field, _: &Ctx) -> i64 {
            1
        }
        let (mut f, rod, board) = field(&[(1800, true)]);
        let mut immunity = crate::effect::Effect::new(effect_type::SINGLE, code::IMMUNE_EFFECT);
        immunity.owner = Some(board[0]);
        immunity.handler = Some(board[0]);
        immunity.flag[0] |= crate::effect::flag::FUNC_VALUE;
        immunity.value_fn = Some(immune);
        let immunity = f.new_effect(immunity);
        f.cards[board[0]].immune_effect.push(immunity);

        let e = effect_of(&f, rod);
        let run = resolve(&mut f, e, 0);
        assert_eq!(f.cards[board[0]].current.location, location::MZONE);
        assert!(run.damages.is_empty(), "no destruction, no burn");
        assert_eq!(f.players[0].lp, 8000);
        assert_eq!(f.players[1].lp, 8000);
    }

    /// **Both declarations are recorded**: the destroy names its card,
    /// and the damage names both players without a figure.
    #[test]
    fn both_declarations_are_recorded() {
        let (mut f, rod, board) = field(&[(1500, true)]);
        let e = effect_of(&f, rod);
        resolve(&mut f, e, 0);
        let link = &f.core.current_chain[0];
        let destroy = link
            .opinfos
            .get(&category::DESTROY)
            .cloned()
            .expect("a DESTROY operation");
        assert_eq!(destroy.cards, Some(vec![board[0]]));
        assert_eq!(destroy.count, 1);
        let damage = link
            .opinfos
            .get(&category::DAMAGE)
            .cloned()
            .expect("a DAMAGE operation");
        assert_eq!(damage.cards, None, "no card is burned");
        assert_eq!(damage.player, PLAYER_ALL, "both players");
    }

    /// **The prompt is the destroy message, to the activating player.**
    #[test]
    fn the_prompt_is_the_destroy_message() {
        let (mut f, rod, _) = field(&[(1000, true)]);
        let e = effect_of(&f, rod);
        resolve(&mut f, e, 0);
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::Hint { kind, player: 0, value }
                    if *kind == hint::SELECTMSG && *value == hintmsg::DESTROY
            )),
            "HINTMSG_DESTROY to the activating player"
        );
    }
}
