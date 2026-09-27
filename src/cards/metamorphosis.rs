//! Metamorphosis (`46411259`) — `cardscripts/c46411259.lua`.
//!
//! ```text
//! Tribute 1 monster. Special Summon 1 Fusion Monster from your Extra
//! Deck with the same Level as the Tributed monster.
//! ```
//!
//! The only way a Fusion Monster reaches the field in this pool — nothing
//! here performs a Fusion Summon, so the material procedures the seven
//! Fusion Monsters register are never asked their question (see
//! [`super::proc_fusion`]).
//!
//! ## The label is a one-shot guard, not a branch
//!
//! `e1:SetLabel(0)` at registration, `s.cost` sets it to **100**, and
//! `s.target` refuses unless it finds a 100 and then puts it back to
//! zero:
//!
//! ```lua
//! if chk==0 then
//!     if e:GetLabel()~=100 then return false end
//!     e:SetLabel(0)
//!     return Duel.CheckReleaseGroupCost(...)
//! end
//! ```
//!
//! So the availability question can only be answered **once per cost
//! call**, and asking it twice in a row without the cost running between
//! says no. That is the script's way of making the activation check and
//! the activation itself agree about which tribute pool they are talking
//! about; a port that dropped the guard would offer the card in windows
//! the reference does not.
//!
//! ## The level travels on the chain link
//!
//! `e:GetChainData().level_of_the_tribute` is a **library** store keyed by
//! chain id and effect (`chain.lua:75`), not engine state. The tribute is
//! paid in the target and the summon happens in the operation, so the
//! level of the card that is now in the graveyard has to survive the gap.

use crate::board::location;
use crate::board::position;
use crate::card::reason;
use crate::cards::aux_release_cost as rel;
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, code, CardId, EffectId};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 46411259;

/// The label `s.cost` writes; `s.target` will not answer without it.
const COST_PAID: i64 = 100;

pub fn initial_effect(f: &mut Field, c: CardId) {
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::SPECIAL_SUMMON);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_label(f, e1, vec![0]);
    api::set_cost(f, e1, cost);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

fn cost(f: &mut Field, ctx: &Ctx, _chk: bool) -> bool {
    api::set_label(f, ctx.reason_effect, vec![COST_PAID]);
    true
}

/// `s.spfilter` — a Fusion Monster in the Extra Deck of that level, with
/// a seat for it once `mc` has left, that may be Special Summoned.
fn spfilter(f: &mut Field, c: CardId, level: u32, e: EffectId, tp: u8, mc: CardId) -> bool {
    api::is_fusion_monster(f, c)
        && api::is_level(f, c, level)
        && api::get_location_count_from_ex(f, tp, Some(tp), api::Except::Card(mc), Some(c)) > 0
        && api::is_can_be_special_summoned(f, c, e, 0, tp, false, false)
}

/// `s.costfilter` — a monster worth tributing: one with a level, and one
/// whose level some Fusion Monster in the Extra Deck shares.
fn costfilter(f: &mut Field, c: CardId, e: EffectId, tp: u8) -> bool {
    let lv = api::get_level(f, c);
    if lv == 0 {
        return false;
    }
    let matches = |f: &mut Field, x: CardId| spfilter(f, x, lv, e, tp, c);
    api::is_existing_matching_card(
        f,
        Some(&matches),
        tp,
        u32::from(location::EXTRA),
        0,
        1,
        api::Except::None,
    )
}

/// The release pool, matched by `s.costfilter`.
fn tribute_pool(f: &mut Field, e: EffectId, tp: u8) -> Vec<CardId> {
    let group = api::get_release_group(f, tp, false, false, None);
    group
        .into_iter()
        .filter(|&c| costfilter(f, c, e, tp))
        .collect()
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let (tp, e) = (ctx.player, ctx.reason_effect);
    if !chk {
        // **The one-shot guard.** See the module note: without a fresh
        // cost call this refuses, and answering consumes the mark.
        if api::get_label_of(f, e) != COST_PAID {
            return api::yes(false);
        }
        api::set_label(f, e, vec![0]);
        let mg = tribute_pool(f, e, tp);
        return api::yes(rel::check_release_group_cost(
            f,
            &mg,
            tp,
            rel::Cost::of(1, 1).borrowed(),
        ));
    }
    let mg = tribute_pool(f, e, tp);
    rel::select_release_group_cost(f, ctx, mg, tp, rel::Cost::of(1, 1), tributed)
}

/// The level of the card just chosen has to outlive it, so it goes on the
/// chain link before the release sends it away.
fn tributed(f: &mut Field, ctx: &Ctx, rg: Vec<CardId>) -> Yield {
    let Some(&first) = rg.first() else {
        return api::yes(false);
    };
    let level = api::get_level(f, first);
    api::set_chain_data(f, ctx.reason_effect, i64::from(level));
    api::release(f, rg, reason::COST, None);
    api::set_operation_info(
        f,
        0,
        category::SPECIAL_SUMMON,
        None,
        1,
        ctx.player,
        i32::from(location::EXTRA),
    );
    api::yes(true)
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let (tp, e) = (ctx.player, ctx.reason_effect);
    let Some(level) = api::get_chain_data(f, e) else {
        return api::done();
    };
    let level = level as u32;
    api::hint(f, hint::SELECTMSG, tp, hintmsg::SPSUMMON);
    // The tribute has already gone, so nothing is excluded from the seat
    // count here — where `s.costfilter` asked with it still on the board.
    let matches = move |f: &mut Field, c: CardId| {
        api::is_fusion_monster(f, c)
            && api::is_level(f, c, level)
            && api::get_location_count_from_ex(f, tp, Some(tp), api::Except::None, Some(c)) > 0
            && api::is_can_be_special_summoned(f, c, e, 0, tp, false, false)
    };
    api::select_matching_card(
        f,
        tp,
        Some(&matches),
        tp,
        u32::from(location::EXTRA),
        0,
        1,
        1,
        api::Except::None,
    );
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
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::Event;
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    fn monster(f: &mut Field, player: u8, seq: u32, level: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 2600 + seq + u32::from(player) * 50,
                type_: card_type::MONSTER | card_type::NORMAL,
                level,
                attack: 1000,
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

    /// Gatling Dragon in `player`'s Extra Deck — the pool's only Fusion
    /// Monster, and level 8.
    fn in_extra(f: &mut Field, player: u8) -> CardId {
        let code_ = crate::cards::gatling_dragon::CODE;
        let mut c = Card::with_data(
            crate::cards::card_data(code_).expect("printed data"),
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::EXTRA, 0, false);
        f.initialize_card(id);
        id
    }

    /// Metamorphosis face-up in `tp`'s row, with a monster of `level` to
    /// tribute and optionally a Fusion Monster in the Extra Deck.
    fn board(tp: u8, level: u32, extra: bool) -> (Field, CardId, CardId, Option<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let mm = f.new_card(d);
        f.add_card(tp, mm, location::SZONE, 0, false);
        f.cards[mm].current.position = position::FACEUP;
        f.initialize_card(mm);
        let tribute = monster(&mut f, tp, 0, level);
        let fusion = extra.then(|| in_extra(&mut f, tp));
        (f, mm, tribute, fusion)
    }

    fn effect_of(f: &Field, mm: CardId) -> EffectId {
        f.cards[mm].field_effect.equal_range(code::FREE_CHAIN)[0]
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

    fn as_reason(f: &mut Field, e: EffectId, tp: u8) {
        f.core.reason_effect = Some(e);
        f.core.reason_player = tp;
    }

    /// Ask the availability question the way an activation does: cost
    /// first, then the target at `chk == 0`.
    fn offered(f: &mut Field, mm: CardId, tp: u8) -> bool {
        let e = effect_of(f, mm);
        as_reason(f, e, tp);
        let ev = Event::new(code::FREE_CHAIN);
        cost(f, &ctx_for(e, &ev, tp), false);
        target(f, &ctx_for(e, &ev, tp), false, None).finished() == Some(1)
    }

    #[derive(Default)]
    struct Run {
        releases: Vec<Vec<CardId>>,
        cards: Vec<(u8, Vec<CardId>)>,
        hints: Vec<(u8, u8, u64)>,
        accepted: bool,
    }

    fn resolve(f: &mut Field, mm: CardId, tp: u8) -> Run {
        let e = effect_of(f, mm);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        as_reason(f, e, tp);
        let ev = Event::new(code::FREE_CHAIN);
        cost(f, &ctx_for(e, &ev, tp), true);
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
        let (mut seen, mut operated) = (0usize, false);
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
                        run.accepted = f.core.returns.get() != 0;
                        if !run.accepted {
                            break;
                        }
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
                    Some(Message::SelectUnselectCard { select, .. }) => {
                        run.releases.push(select.clone());
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 1);
                        f.core.returns.set_i32(1, 0);
                    }
                    Some(Message::SelectCard {
                        player, min, cards, ..
                    }) => {
                        run.cards.push((*player, cards.clone()));
                        let min = usize::from(*min);
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, i as i32);
                        }
                    }
                    // The summon asks which way up as well as where.
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
                Status::End => break,
            }
        }
        run
    }

    mod availability {
        use super::*;

        /// **A level-8 tribute and a level-8 Fusion Monster.**
        #[test]
        fn a_matching_level_in_the_extra_deck_offers_it() {
            let (mut f, mm, _, _) = board(0, 8, true);
            assert!(offered(&mut f, mm, 0));
        }

        /// **The levels have to match.** Everything else is the same
        /// board.
        #[test]
        fn a_level_that_matches_nothing_refuses() {
            let (mut f, mm, _, _) = board(0, 4, true);
            assert!(!offered(&mut f, mm, 0), "no level-4 Fusion Monster");
        }

        /// **An empty Extra Deck refuses**, which is the state this pool
        /// is in for every deck that does not name a Fusion Monster.
        #[test]
        fn an_empty_extra_deck_refuses() {
            let (mut f, mm, _, _) = board(0, 8, false);
            assert!(!offered(&mut f, mm, 0));
        }

        /// The opponent's Extra Deck is not ours to summon from.
        #[test]
        fn the_opponents_extra_deck_does_not_count() {
            let (mut f, mm, _, _) = board(0, 8, false);
            in_extra(&mut f, 1);
            assert!(!offered(&mut f, mm, 0));
        }

        /// **The one-shot guard.** Asking twice without the cost running
        /// between says no the second time — the script's way of making
        /// the check and the activation agree.
        #[test]
        fn the_question_may_only_be_asked_once_per_cost() {
            let (mut f, mm, _, _) = board(0, 8, true);
            let e = effect_of(&f, mm);
            as_reason(&mut f, e, 0);
            let ev = Event::new(code::FREE_CHAIN);
            cost(&mut f, &ctx_for(e, &ev, 0), false);
            assert_eq!(
                target(&mut f, &ctx_for(e, &ev, 0), false, None).finished(),
                Some(1),
                "the first ask"
            );
            assert_eq!(
                target(&mut f, &ctx_for(e, &ev, 0), false, None).finished(),
                Some(0),
                "and the second, with no cost between, refuses"
            );
        }

        /// A monster with no level cannot be the tribute.
        #[test]
        fn a_level_less_monster_is_not_a_tribute() {
            let (mut f, mm, tribute, _) = board(0, 8, true);
            assert!(offered(&mut f, mm, 0), "the positive sibling");
            f.cards[tribute].data.level = 0;
            assert!(!offered(&mut f, mm, 0));
        }

        /// **Only a Fusion Monster.** An ordinary monster sitting in the
        /// Extra Deck of the same level is not a summon target.
        #[test]
        fn a_non_fusion_card_in_the_extra_deck_does_not_count() {
            let (mut f, mm, _, _) = board(0, 8, false);
            let mut c = Card::with_data(
                CardData {
                    code: 909_090,
                    type_: card_type::MONSTER | card_type::NORMAL,
                    level: 8,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            c.set_status(status::EFFECT_ENABLED, true);
            let id = f.new_card(c);
            f.add_card(0, id, location::EXTRA, 0, false);
            assert!(!offered(&mut f, mm, 0), "level 8, but not a Fusion Monster");
        }

        /// **A Fusion Monster that may not be Special Summoned is not a
        /// target either.** A `REVIVE_LIMIT` on a card that was never
        /// properly summoned is the plainest way to say so.
        #[test]
        fn a_fusion_monster_that_cannot_be_summoned_does_not_count() {
            let (mut f, mm, _, fusion) = board(0, 8, true);
            let fusion = fusion.expect("a fusion monster");
            assert!(offered(&mut f, mm, 0), "the positive sibling");
            let mut e =
                crate::effect::Effect::new(effect_type::SINGLE, code::CANNOT_SPECIAL_SUMMON);
            e.owner = Some(fusion);
            e.handler = Some(fusion);
            let id = f.new_effect(e);
            f.cards[fusion]
                .single_effect
                .insert(code::CANNOT_SPECIAL_SUMMON, id);
            f.cards[fusion].indexer.insert(id);
            assert!(!offered(&mut f, mm, 0));
        }

        /// **A full row still works when the tribute is in it** — the
        /// seat count is asked with the tribute already gone.
        #[test]
        fn a_full_row_is_fine_because_the_tribute_frees_a_seat() {
            let (mut f, mm, _, _) = board(0, 8, true);
            for i in 1..5 {
                monster(&mut f, 0, i, 4);
            }
            assert!(
                offered(&mut f, mm, 0),
                "five monsters, but one of them is leaving"
            );
        }
    }

    mod resolving {
        use super::*;

        /// **End to end**: the tribute is paid as a cost and the Fusion
        /// Monster arrives face-up from the Extra Deck.
        #[test]
        fn it_tributes_and_summons_the_fusion_monster() {
            let (mut f, mm, tribute, fusion) = board(0, 8, true);
            let fusion = fusion.expect("a fusion monster");
            let run = resolve(&mut f, mm, 0);
            assert!(run.accepted);
            assert_eq!(run.releases.len(), 1, "one tribute question");
            assert_eq!(run.releases[0], vec![tribute]);
            assert_eq!(
                f.cards[tribute].current.location,
                location::GRAVE,
                "the tribute is paid"
            );
            assert!(
                f.cards[tribute].reason & reason::COST != 0,
                "as a cost, not an effect"
            );
            assert_eq!(run.cards.len(), 1, "one summon question");
            assert_eq!(run.cards[0].1, vec![fusion], "the Extra Deck's only match");
            assert_eq!(
                f.cards[fusion].current.location,
                location::MZONE,
                "and it arrives"
            );
            assert_eq!(
                f.cards[fusion].current.position & position::FACEUP,
                f.cards[fusion].current.position,
                "face-up"
            );
            assert!(run.hints.contains(&(hint::SELECTMSG, 0, hintmsg::SPSUMMON)));
        }

        /// **The level travels on the chain link.** The tribute is in the
        /// graveyard by the time the summon is chosen, so the level it
        /// had has to have been written down.
        #[test]
        fn the_level_outlives_the_tribute() {
            let (mut f, mm, tribute, _) = board(0, 8, true);
            let e = effect_of(&f, mm);
            resolve(&mut f, mm, 0);
            assert_eq!(
                f.cards[tribute].current.location,
                location::GRAVE,
                "gone before the summon was chosen"
            );
            assert_eq!(
                api::get_chain_data(&f, e),
                Some(8),
                "and its level was kept on the chain link"
            );
        }

        /// **The resolution offers only the matching level**, not every
        /// Fusion Monster in the Extra Deck.
        #[test]
        fn only_the_matching_level_is_offered_at_resolution() {
            let (mut f, mm, _, fusion) = board(0, 8, true);
            let fusion = fusion.expect("a fusion monster");
            // A second Fusion Monster of a different level.
            let mut c = Card::with_data(
                CardData {
                    code: 808_080,
                    type_: card_type::MONSTER | card_type::FUSION | card_type::EFFECT,
                    level: 5,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            c.set_status(status::EFFECT_ENABLED, true);
            let other = f.new_card(c);
            f.add_card(0, other, location::EXTRA, 1, false);

            let run = resolve(&mut f, mm, 0);
            assert!(run.accepted);
            assert_eq!(
                run.cards[0].1,
                vec![fusion],
                "the level-8 one alone; the level-5 one is not offered"
            );
        }

        /// The operation info names the Extra Deck, which is where the
        /// summon comes from.
        #[test]
        fn the_operation_info_names_the_extra_deck() {
            let (mut f, mm, _, _) = board(1, 8, true);
            resolve(&mut f, mm, 1);
            let info = f.core.current_chain[0]
                .opinfos
                .get(&category::SPECIAL_SUMMON)
                .expect("the summon category");
            assert_eq!(info.count, 1);
            assert_eq!(info.player, 1, "the activating player");
            assert_eq!(info.param, i32::from(location::EXTRA));
        }
    }
}
