//! Creature Swap — `c31036355.lua`.
//!
//! The thirtieth card, and the first whose whole effect is an
//! **exchange**: each player picks one of their own monsters, and the two
//! trade sides. Nobody chooses what they get.
//!
//! ## The filter is about the seat, not the monster
//!
//! ```lua
//! function s.filter(c)
//!   return c:IsAbleToChangeControler()
//!     and (c:GetSequence()<5 or Duel.GetLocationCount(c:GetControler(),LOCATION_MZONE)>0)
//! end
//! ```
//!
//! The second half is the interesting one, and it reads oddly until you
//! see what it is for. A monster sitting in one of the five main Monster
//! Zones (`GetSequence() < 5`) is fine: it is going to hand its own seat
//! over as part of the exchange, so no new room is needed. A monster in
//! an **Extra Monster Zone** is not, because its seat does not belong to
//! the row it is leaving — its controller needs a spare main seat for the
//! incoming monster, and without one the exchange cannot be made.
//!
//! In a five-zone Goat-format field no monster is ever above seat 4, so
//! the `or` short-circuits every time and `GetLocationCount` is never
//! reached. It is transcribed anyway and pinned by a unit test that seats
//! a monster at 5 by hand — the branch is unreachable *in this
//! configuration*, which is not the same as unreachable.
//!
//! ## Both players are asked, and the activating player goes first
//!
//! Two `SelectMatchingCard`s, each scoped to one side of the table with
//! `(tp, LOCATION_MZONE, 0)` and `(1-tp, LOCATION_MZONE, 0)` — note the
//! second is asked **of `1-tp` about `1-tp`'s own row**, not of `tp`
//! about the opponent's. Neither player picks the monster they will
//! receive; each picks the one they give away, and the opponent's choice
//! is made after seeing the first.
//!
//! `Duel.HintSelection` after each one is what makes that visible: the
//! choice is announced to the table before the next question is asked.
//!
//! ## The position lock is conditional, and it is two effects
//!
//! `Duel.SwapControl` returns whether the exchange actually happened —
//! it can refuse, for instance if a monster stopped being able to change
//! hands between the choice and the resolution. Only then does the card
//! register `EFFECT_CANNOT_CHANGE_POSITION` on **both** monsters, until
//! the end of the turn.
//!
//! The second effect is `e1:Clone()`, not a fresh one built the same way.
//! That matters: a clone copies the description and the client-hint
//! property too, so both monsters carry the same visible reminder.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::duel::phases;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::{reset, Field};
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 31_036_355;

/// The reminder text the position lock carries — `e1:SetDescription(3313)`.
const CANNOT_CHANGE_POSITION_HINT: u64 = 3313;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Each player changes control of 1 of their monsters
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::CONTROL);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

const MZONE: u32 = location::MZONE as u32;

/// `s.filter` — a monster that can change hands and whose side will have
/// room for what it gets back.
fn filter(f: &mut Field, c: CardId) -> bool {
    api::is_able_to_change_controler(f, c)
        && (api::get_sequence(f, c) < 5
            || api::get_location_count(f, api::get_controler(f, c), location::MZONE) > 0)
}

/// `Duel.IsExistingMatchingCard(s.filter, tp, s, o, 1, nil)` — the pair
/// of scans the card makes twice over: once to decide whether it may be
/// activated, and again on resolution, because the board can change in
/// between.
fn has_one(f: &mut Field, tp: u8, s: u32, o: u32) -> bool {
    api::is_existing_matching_card(f, Some(&filter), tp, s, o, 1, api::Except::None)
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        return api::yes(has_one(f, tp, MZONE, 0) && has_one(f, tp, 0, MZONE));
    }
    // Names the category with no cards and no count: the exchange is
    // announced, but which monsters is not decided until it resolves.
    api::set_operation_info(f, 0, category::CONTROL, None, 0, 0, 0);
    api::yes(true)
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let Some(handler) = api::get_handler(f, ctx.reason_effect) else {
        return api::done();
    };
    if !has_one(f, tp, MZONE, 0) || !has_one(f, tp, 0, MZONE) {
        return api::done();
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::CONTROL);
    api::select_matching_card(f, tp, Some(&filter), tp, MZONE, 0, 1, 1, api::Except::None);
    api::suspend(move |f, _ctx| {
        let g1 = api::group_selected(f);
        api::hint_selection(f, &g1, true);
        api::hint(f, hint::SELECTMSG, 1 - tp, hintmsg::CONTROL);
        api::select_matching_card(
            f,
            1 - tp,
            Some(&filter),
            1 - tp,
            MZONE,
            0,
            1,
            1,
            api::Except::None,
        );
        // Carried into the next suspension by value; the outer closure is
        // re-entrant, so it may not give its own capture away.
        let first = g1.clone();
        api::suspend(move |f, _ctx| {
            let g2 = api::group_selected(f);
            api::hint_selection(f, &g2, true);
            let (Some(&c1), Some(&c2)) = (first.first(), g2.first()) else {
                return api::done();
            };
            api::swap_control(f, c1, c2, 0, 0);
            api::suspend(move |f, _ctx| {
                if api::resumed_value(f) == 0 {
                    return api::done();
                }
                for locked in [c1, c2] {
                    // Cannot change their battle positions
                    let e = api::create_effect(f, handler);
                    api::set_description(f, e, CANNOT_CHANGE_POSITION_HINT);
                    api::set_property(f, e, flag::CLIENT_HINT, 0);
                    api::set_type(f, e, effect_type::SINGLE);
                    api::set_code(f, e, code::CANNOT_CHANGE_POSITION);
                    api::set_reset(f, e, reset::PHASE | u32::from(phases::END), 1);
                    api::register_effect(f, locked, e, false);
                }
                api::done()
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::event::{EffectId, Event};
    use crate::field::Message;
    use crate::position;
    use crate::processor::{Kind, Status};

    fn monster_at(f: &mut Field, player: u8, code_: u32, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
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
        let cs = f.new_card(d);
        f.add_card(tp, cs, location::SZONE, 0, false);
        f.cards[cs].current.position = position::FACEUP;
        f.initialize_card(cs);
        let ours = (0..mine)
            .map(|i| monster_at(&mut f, tp, 7_000 + i, i))
            .collect();
        let theirs = (0..theirs)
            .map(|i| monster_at(&mut f, 1 - tp, 8_000 + i, i))
            .collect();
        (f, cs, ours, theirs)
    }

    fn field(mine: u32, theirs: u32) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        field_as(0, mine, theirs)
    }

    fn effect_of(f: &Field, cs: CardId) -> EffectId {
        f.cards[cs].field_effect.equal_range(code::FREE_CHAIN)[0]
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

    /// Everything a driven resolution said and was asked.
    #[derive(Default)]
    struct Run {
        /// Each `SelectCard`: who was asked, and what they were offered.
        card_questions: Vec<(u8, Vec<CardId>)>,
        /// Each `SelectPlace`: who was asked, and the forbidden-seat mask.
        place_questions: Vec<(u8, u32)>,
        /// Each `Hint`, in order.
        hints: Vec<(u8, u8, u64)>,
        /// Each `CardSelected` announcement, as controller/sequence pairs.
        announced: Vec<Vec<(u8, u32)>>,
        /// Whether a `BecomeTarget` was written instead.
        become_target: usize,
        /// Each `Swap` message.
        swaps: Vec<(u32, u32)>,
        /// Each `Move` message, in order.
        moves: Vec<u32>,
    }

    /// Drive target then operation, answering both questions the card asks.
    ///
    /// `pick` is the index each player takes from what they are offered;
    /// `seat` chooses a seat out of the mask — `Seat::First` takes the
    /// lowest free one, `Seat::Vacated` takes the seat being given up,
    /// which is the one the mask specially un-forbids.
    fn resolve_as(f: &mut Field, tp: u8, e: EffectId, pick: usize, seat: Seat) -> Run {
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
        let mut run = Run::default();
        let mut seen = 0usize;
        let mut operated = false;
        for _ in 0..8192 {
            // Messages are read as they arrive, not at the end: the
            // *order* of hint, announcement and question is the thing
            // several of these tests are about.
            while seen < f.messages.len() {
                match &f.messages[seen] {
                    Message::Hint {
                        kind,
                        player,
                        value,
                    } => run.hints.push((*kind, *player, *value)),
                    Message::CardSelected { cards } => run.announced.push(
                        cards
                            .iter()
                            .map(|i| (i.controller, i.sequence))
                            .collect::<Vec<_>>(),
                    ),
                    Message::BecomeTarget { .. } => run.become_target += 1,
                    Message::Swap { first, second } => run.swaps.push((*first, *second)),
                    Message::Move { code, .. } => run.moves.push(*code),
                    _ => {}
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
                        run.card_questions.push((*player, cards.clone()));
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
                        run.place_questions.push((pl, flag));
                        let free: Vec<u32> = (0..5u32).filter(|s| flag & (1 << s) == 0).collect();
                        let last = *free.last().expect("a free monster seat");
                        let seq = match seat {
                            Seat::First => free[0],
                            Seat::Last => last,
                            Seat::ByPlayer if pl == 0 => free[0],
                            Seat::ByPlayer => last,
                        };
                        f.core.returns.set_i8(0, pl as i8);
                        f.core.returns.set_i8(1, location::MZONE as i8);
                        f.core.returns.set_i8(2, seq as i8);
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        run
    }

    #[derive(Clone, Copy)]
    enum Seat {
        First,
        Last,
        /// Player 0 takes the lowest free seat, player 1 the highest —
        /// so the two answers differ, which is the only way to tell one
        /// seat from the other.
        ByPlayer,
    }

    fn resolve(f: &mut Field, e: EffectId, pick: usize) -> Run {
        resolve_as(f, 0, e, pick, Seat::First)
    }

    /// **One printed activate effect in the control category**, with no
    /// cost, no condition and no card target.
    #[test]
    fn the_script_registers_one_activate_effect_in_the_control_category() {
        let (f, cs, _, _) = field(1, 1);
        let ids = f.cards[cs].field_effect.equal_range(code::FREE_CHAIN);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert_eq!(e.category, category::CONTROL);
        assert_eq!(e.code, code::FREE_CHAIN);
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert!(!e.is_flag(flag::CARD_TARGET), "it names no card");
        assert!(e.cost.is_none());
        assert!(e.condition.is_none());
        assert!(e.target.is_some());
        assert!(e.operation.is_some());
    }

    /// **Both sides must have a monster.** Three boards, and only the one
    /// with a monster on each side may activate — the two negative cases
    /// are the sibling pair, so a scan that looked at one side only would
    /// pass one of them.
    #[test]
    fn it_needs_a_monster_on_each_side() {
        let ev = Event::new(code::FREE_CHAIN);
        for (mine, theirs, want) in [(0, 0, false), (1, 0, false), (0, 1, false), (1, 1, true)] {
            let (mut f, cs, _, _) = field(mine, theirs);
            let e = effect_of(&f, cs);
            let ctx = ctx_for(e, &ev, 0);
            let got = target(&mut f, &ctx, false, None).finished().unwrap_or(0) != 0;
            assert_eq!(got, want, "mine={mine} theirs={theirs}");
        }
    }

    /// **The same, asked as player 1.** A suite that only ever activates
    /// as player 0 cannot tell `tp` from a literal `0`.
    #[test]
    fn the_scan_follows_the_activating_player() {
        let ev = Event::new(code::FREE_CHAIN);
        // Only player 1 has a monster, and player 1 is activating: its
        // own side is stocked, the opponent's is empty.
        let (mut f, cs, _, _) = field_as(1, 1, 0);
        let e = effect_of(&f, cs);
        let ctx = ctx_for(e, &ev, 1);
        assert_eq!(
            target(&mut f, &ctx, false, None).finished().unwrap_or(0),
            0,
            "nothing on the opponent's side"
        );
        let (mut f, cs, _, _) = field_as(1, 1, 1);
        let e = effect_of(&f, cs);
        let ctx = ctx_for(e, &ev, 1);
        assert_ne!(target(&mut f, &ctx, false, None).finished().unwrap_or(0), 0);
    }

    /// **A monster that cannot change hands is not a monster it can
    /// use.** The positive sibling is the same board without the lock.
    #[test]
    fn a_control_locked_monster_does_not_count() {
        let ev = Event::new(code::FREE_CHAIN);
        let (mut f, cs, _, theirs) = field(1, 1);
        let e = effect_of(&f, cs);
        let ctx = ctx_for(e, &ev, 0);
        assert_ne!(
            target(&mut f, &ctx, false, None).finished().unwrap_or(0),
            0,
            "the positive sibling"
        );
        lock_control(&mut f, theirs[0]);
        assert_eq!(
            target(&mut f, &ctx, false, None).finished().unwrap_or(0),
            0,
            "their only monster cannot change hands"
        );
    }

    fn lock_control(f: &mut Field, card: CardId) {
        let mut e = crate::effect::Effect::new(effect_type::SINGLE, code::CANNOT_CHANGE_CONTROL);
        e.owner = Some(card);
        e.handler = Some(card);
        let id = f.new_effect(e);
        f.cards[card]
            .single_effect
            .insert(code::CANNOT_CHANGE_CONTROL, id);
        f.cards[card].indexer.insert(id);
    }

    /// **The seat half of the filter**, which the Goat field never
    /// reaches on its own: a monster above seat 4 needs its controller to
    /// have a spare main seat, and one in a main seat never does.
    ///
    /// Both halves are exercised here by hand, because `< 5` short
    /// circuits for every monster the harness can produce.
    #[test]
    fn a_monster_above_seat_four_needs_room_and_one_below_does_not() {
        let (mut f, _, _, _) = field(0, 0);
        // Seats 0..4 full, plus one in the extra zone.
        let below: Vec<CardId> = (0..5)
            .map(|i| monster_at(&mut f, 0, 7_100 + i, i))
            .collect();
        let above = monster_at(&mut f, 0, 7_200, 5);
        assert_eq!(f.cards[above].current.sequence, 5, "seated above the row");
        assert_eq!(
            api::get_location_count(&mut f, 0, location::MZONE),
            0,
            "the row is full"
        );
        assert!(filter(&mut f, below[0]), "a main seat needs no room");
        assert!(!filter(&mut f, above), "the extra zone does");
        // Free one main seat and the extra-zone monster qualifies too.
        f.remove_card(below[4]);
        assert!(filter(&mut f, above), "now there is room");
    }

    /// **The announcement names the control category and nothing else**
    /// — no cards, no player, no count, because none of it is decided
    /// until the exchange resolves.
    #[test]
    fn the_announcement_is_the_bare_control_category() {
        let (mut f, cs, _, _) = field(1, 1);
        let e = effect_of(&f, cs);
        let ev = Event::new(code::FREE_CHAIN);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        f.core.current_chain.push(ch);
        let ctx = ctx_for(e, &ev, 0);
        target(&mut f, &ctx, true, None);
        let ops = &f.core.current_chain[0].opinfos;
        let info = ops.get(&category::CONTROL).expect("the control category");
        assert!(info.cards.is_none(), "no cards named");
        assert_eq!(info.player, 0);
        assert_eq!(info.param, 0);
        assert_eq!(ops.len(), 1, "and nothing else announced");
    }

    /// **Each player is asked about their own row, activating player
    /// first** — and each is offered only their own monsters.
    #[test]
    fn each_player_is_asked_about_their_own_row_in_turn() {
        let (mut f, cs, ours, theirs) = field(2, 2);
        let e = effect_of(&f, cs);
        let run = resolve(&mut f, e, 0);
        assert_eq!(run.card_questions.len(), 2, "two questions, one each");
        assert_eq!(run.card_questions[0].0, 0, "the activating player first");
        assert_eq!(run.card_questions[0].1, ours);
        assert_eq!(run.card_questions[1].0, 1, "then the opponent");
        assert_eq!(run.card_questions[1].1, theirs);
    }

    /// **The same, activating as player 1** — the questions follow `tp`,
    /// not the table.
    #[test]
    fn the_questions_follow_the_activating_player() {
        let (mut f, cs, ours, theirs) = field_as(1, 2, 2);
        let e = effect_of(&f, cs);
        let run = resolve_as(&mut f, 1, e, 0, Seat::First);
        assert_eq!(run.card_questions[0].0, 1, "player 1 activated");
        assert_eq!(run.card_questions[0].1, ours);
        assert_eq!(run.card_questions[1].0, 0);
        assert_eq!(run.card_questions[1].1, theirs);
    }

    /// **Each question is prefaced by the control prompt, addressed to
    /// whoever is about to answer it**, and each answer is announced to
    /// the table as a selection rather than as a target.
    #[test]
    fn each_question_is_prompted_and_each_answer_announced() {
        let (mut f, cs, _, _) = field(2, 2);
        let e = effect_of(&f, cs);
        let run = resolve(&mut f, e, 1);
        let control: Vec<(u8, u8, u64)> = run
            .hints
            .iter()
            .copied()
            .filter(|&(_, _, v)| v == hintmsg::CONTROL)
            .collect();
        assert_eq!(
            control,
            vec![
                (hint::SELECTMSG, 0, hintmsg::CONTROL),
                (hint::SELECTMSG, 1, hintmsg::CONTROL)
            ],
            "one prompt each, in the order the questions come"
        );
        // Both answers announced, and both are the second monster of the
        // row — the index the test asked for.
        assert_eq!(run.announced, vec![vec![(0, 1)], vec![(1, 1)]]);
        assert_eq!(run.become_target, 0, "a choice, not a target");
    }

    /// **The exchange happens: each monster ends up in the other
    /// player's row, and it is the chosen pair that moves.**
    #[test]
    fn the_chosen_pair_changes_hands() {
        let (mut f, cs, ours, theirs) = field(2, 2);
        let e = effect_of(&f, cs);
        let (given, taken) = (ours[1], theirs[1]);
        let (stays_mine, stays_theirs) = (ours[0], theirs[0]);
        resolve(&mut f, e, 1);
        assert_eq!(f.cards[given].current.controller, 1, "mine went across");
        assert_eq!(f.cards[taken].current.controller, 0, "theirs came back");
        assert_eq!(f.cards[stays_mine].current.controller, 0);
        assert_eq!(f.cards[stays_theirs].current.controller, 1);
        // Owners never change.
        assert_eq!(f.cards[given].owner, 0);
        assert_eq!(f.cards[taken].owner, 1);
        // And each is findable in the row it now sits in.
        let seat = f.cards[given].current.sequence as usize;
        assert_eq!(f.players[1].mzone[seat], Some(given));
        let seat = f.cards[taken].current.sequence as usize;
        assert_eq!(f.players[0].mzone[seat], Some(taken));
    }

    /// **Each player is asked for a seat, for their own row, and the
    /// mask offers the seat they are vacating** — but not the seats
    /// their other monsters hold, and nothing above seat 4.
    #[test]
    fn each_player_picks_a_seat_in_their_own_row() {
        let (mut f, cs, _, _) = field(2, 2);
        let e = effect_of(&f, cs);
        let run = resolve(&mut f, e, 1);
        assert_eq!(run.place_questions.len(), 2, "one seat question each");
        let (p0, mask0) = run.place_questions[0];
        let (p1, mask1) = run.place_questions[1];
        assert_eq!((p0, p1), (0, 1), "each about their own row");
        for mask in [mask0, mask1] {
            // Seat 1 is the one being given up, so it is offered back.
            assert_eq!(mask & (1 << 1), 0, "the vacated seat is free");
            assert_ne!(mask & (1 << 0), 0, "the seat still occupied is not");
            for s in 5..32 {
                assert_ne!(mask & (1 << s), 0, "seat {s} is forbidden");
            }
        }
    }

    /// **The seat prompt names the monster arriving, not the one being
    /// given away** — the question is "where do you want this?".
    #[test]
    fn the_seat_prompt_names_the_incoming_monster() {
        let (mut f, cs, ours, theirs) = field(2, 2);
        let e = effect_of(&f, cs);
        let (mine, theirs1) = (f.cards[ours[1]].data.code, f.cards[theirs[1]].data.code);
        let run = resolve(&mut f, e, 1);
        let seat_hints: Vec<(u8, u64)> = run
            .hints
            .iter()
            .filter(|&&(_, _, v)| v == u64::from(mine) || v == u64::from(theirs1))
            .map(|&(_, p, v)| (p, v))
            .collect();
        assert_eq!(
            seat_hints,
            vec![(0, u64::from(theirs1)), (1, u64::from(mine))],
            "each side is told what it is about to receive"
        );
    }

    /// **A player may take the seat they vacated, or another** — and the
    /// two seats are independent, so each answer lands where its own
    /// player put it.
    #[test]
    fn each_side_seats_its_new_monster_where_it_chose() {
        // Both sides hold seats 0 and 1 and give up seat 1. Taking the
        // *last* free seat of the mask puts the arrival at seat 4.
        let (mut f, cs, ours, theirs) = field(2, 2);
        let e = effect_of(&f, cs);
        let (given, taken) = (ours[1], theirs[1]);
        resolve_as(&mut f, 0, e, 1, Seat::Last);
        assert_eq!(f.cards[given].current.sequence, 4);
        assert_eq!(f.cards[taken].current.sequence, 4);
        assert_eq!(f.players[1].mzone[4], Some(given));
        assert_eq!(f.players[0].mzone[4], Some(taken));
        // The vacated seats are empty and no longer marked as used.
        assert_eq!(f.players[0].mzone[1], None);
        assert_eq!(f.players[1].mzone[1], None);
        assert_eq!(f.players[0].used_location & (1 << 1), 0);
        assert_eq!(f.players[1].used_location & (1 << 1), 0);
    }

    /// **Two monsters keeping their own sequence write one `Swap`; a
    /// reseating writes two `Move`s instead.**
    #[test]
    fn the_message_says_whether_anyone_moved_seat() {
        let (mut f, cs, ours, theirs) = field(2, 2);
        let e = effect_of(&f, cs);
        let (mine, other) = (f.cards[ours[1]].data.code, f.cards[theirs[1]].data.code);
        let run = resolve_as(&mut f, 0, e, 1, Seat::First);
        assert_eq!(
            run.swaps,
            vec![(mine, other)],
            "each kept its own seat: one swap"
        );
        assert!(run.moves.is_empty());

        let (mut f, cs, _, _) = field(2, 2);
        let e = effect_of(&f, cs);
        let run = resolve_as(&mut f, 0, e, 1, Seat::Last);
        assert!(run.swaps.is_empty(), "both reseated");
        assert_eq!(run.moves, vec![other, mine], "card 2 first");
    }

    /// **Both monsters are locked out of changing position until the end
    /// of the turn**, and both locks are registered by the Spell.
    #[test]
    fn both_monsters_are_locked_out_of_changing_position() {
        let (mut f, cs, ours, theirs) = field(1, 1);
        let e = effect_of(&f, cs);
        let (given, taken) = (ours[0], theirs[0]);
        resolve(&mut f, e, 0);
        for card in [given, taken] {
            let ids = f.cards[card]
                .single_effect
                .equal_range(code::CANNOT_CHANGE_POSITION);
            assert_eq!(ids.len(), 1, "one lock on card {card}");
            let lock = f.effects.get(ids[0]).unwrap();
            assert!(lock.is_type(effect_type::SINGLE));
            assert_eq!(lock.owner, Some(cs), "the Spell owns it");
            assert_eq!(lock.handler, Some(card));
            assert_eq!(lock.description, CANNOT_CHANGE_POSITION_HINT);
            assert!(lock.is_flag(flag::CLIENT_HINT));
            assert_ne!(lock.reset_flag & reset::PHASE, 0);
            assert_ne!(lock.reset_flag & u32::from(phases::END), 0);
            assert_eq!(lock.reset_count, 1);
        }
    }

    /// **A swapped monster stops attacking and loses what it held by
    /// being on that side** — `STATUS_ATTACK_CANCELED` on both, and a
    /// `RESET_CONTROL` effect gone.
    #[test]
    fn the_exchange_cancels_attacks_and_resets_control_effects() {
        let (mut f, cs, ours, theirs) = field(1, 1);
        let e = effect_of(&f, cs);
        let (given, taken) = (ours[0], theirs[0]);
        let watched = {
            let mut x = crate::effect::Effect::new(effect_type::SINGLE, code::UPDATE_ATTACK);
            x.owner = Some(given);
            x.handler = Some(given);
            x.reset_flag = crate::field::reset::EVENT + crate::field::reset::CONTROL;
            let id = f.new_effect(x);
            f.cards[given].single_effect.insert(code::UPDATE_ATTACK, id);
            f.cards[given].indexer.insert(id);
            id
        };
        resolve(&mut f, e, 0);
        for card in [given, taken] {
            assert!(
                f.cards[card].is_status(status::ATTACK_CANCELED),
                "card {card} is no longer attacking"
            );
        }
        assert!(
            !f.cards[given].indexer.contains(&watched),
            "a control-reset effect did not survive the change of sides"
        );
    }

    /// **The two seats are independent answers, and each goes to the
    /// right monster.** Both players are asked, they answer differently,
    /// and the monster each receives lands on the seat *that player*
    /// named — not the other one's.
    #[test]
    fn each_seat_answer_goes_to_the_monster_that_player_receives() {
        let (mut f, cs, ours, theirs) = field(2, 2);
        let e = effect_of(&f, cs);
        let (given, taken) = (ours[1], theirs[1]);
        let run = resolve_as(&mut f, 0, e, 1, Seat::ByPlayer);
        assert_eq!(run.place_questions[0].0, 0);
        assert_eq!(run.place_questions[1].0, 1);
        // Player 0 said "the lowest free seat", which is the one it just
        // vacated; player 1 said "the highest", which is seat 4.
        assert_eq!(
            f.cards[taken].current.sequence, 1,
            "player 0 receives at the seat player 0 chose"
        );
        assert_eq!(
            f.cards[given].current.sequence, 4,
            "player 1 receives at the seat player 1 chose"
        );
        assert_eq!(f.players[0].mzone[1], Some(taken));
        assert_eq!(f.players[1].mzone[4], Some(given));
    }

    /// **A resolution with nothing left to exchange asks nothing.** The
    /// operation re-runs the same pair of scans the activation did,
    /// because the board can be emptied by a chain link on top of it.
    #[test]
    fn a_one_sided_board_asks_nothing_on_resolution() {
        let (mut f, cs, _, _) = field(1, 0);
        let e = effect_of(&f, cs);
        let run = resolve(&mut f, e, 0);
        assert!(run.card_questions.is_empty(), "nobody was asked");
        assert!(run.place_questions.is_empty());
        assert!(run.swaps.is_empty());
    }

    /// **A monster immune to this Spell passes the filter and is refused
    /// by the exchange** — which is the one route to a resolution whose
    /// `SwapControl` answers false, and so the one board where the
    /// position lock must not be registered.
    ///
    /// The filter asks `IsAbleToChangeControler`, which is about control
    /// locks only; immunity to the *resolving effect* is checked by the
    /// exchange itself.
    #[test]
    fn a_monster_immune_to_the_spell_is_chosen_and_then_refused() {
        let (mut f, cs, ours, theirs) = field(1, 1);
        let e = effect_of(&f, cs);
        let (given, taken) = (ours[0], theirs[0]);
        let mut imm = crate::effect::Effect::new(effect_type::SINGLE, code::IMMUNE_EFFECT);
        imm.owner = Some(taken);
        imm.handler = Some(taken);
        imm.value = 1;
        let imm = f.new_effect(imm);
        f.cards[taken].immune_effect.push(imm);
        f.cards[taken].indexer.insert(imm);

        let run = resolve(&mut f, e, 0);
        assert_eq!(run.card_questions.len(), 2, "it still asked both players");
        assert!(run.place_questions.is_empty(), "but never got to the seats");
        assert_eq!(f.cards[given].current.controller, 0, "nothing moved");
        assert_eq!(f.cards[taken].current.controller, 1);
        for card in [given, taken] {
            assert!(
                f.cards[card]
                    .single_effect
                    .equal_range(code::CANNOT_CHANGE_POSITION)
                    .is_empty(),
                "a refused exchange locks nothing"
            );
        }
    }

    /// **Both events are raised over both monsters.**
    #[test]
    fn the_exchange_raises_a_control_change_and_a_move() {
        let (mut f, cs, ours, theirs) = field(1, 1);
        let e = effect_of(&f, cs);
        let _ = (ours, theirs);
        resolve(&mut f, e, 0);
        for want in [code::CONTROL_CHANGED, code::MOVE] {
            assert!(
                f.core
                    .instant_event
                    .iter()
                    .chain(f.core.used_event.iter())
                    .any(|ev| ev.event_code == want),
                "event {want} was raised"
            );
        }
    }
}
