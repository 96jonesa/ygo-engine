//! Dust Tornado — `c60082869.lua`.
//!
//! Destroys one Spell or Trap the opponent controls and, **if that
//! destruction actually happened**, offers to Set one Spell or Trap from
//! the hand. The eighteenth card, and the first whose second half is
//! conditional on the result of its first.
//!
//! ## Reading a result back three times over
//!
//! ```lua
//! if tc and tc:IsRelateToEffect(e) and Duel.Destroy(tc,REASON_EFFECT)~=0 then
//!     local g=Duel.GetMatchingGroup(Card.IsSSetable,tp,LOCATION_HAND,0,nil)
//!     if #g>0 and Duel.SelectYesNo(tp,aux.Stringid(id,0)) then
//!         Duel.BreakEffect()
//!         Duel.Hint(HINT_SELECTMSG,tp,HINTMSG_SET)
//!         local sg=g:Select(tp,1,1,nil)
//!         Duel.SSet(tp,sg:GetFirst(),tp,false)
//!     end
//! end
//! ```
//!
//! Three exports in that body yield and hand a value back — `Duel.Destroy`
//! (a count), `Duel.SelectYesNo` (a boolean) and `Group.Select` (a group)
//! — so the operation suspends three times, each resumption starting where
//! the last left off. The port expresses that as nested continuations;
//! `execute_resume` re-parks a suspension that suspends again in the same
//! token, so the depth costs nothing.
//!
//! **The `~= 0` is load-bearing.** A destruction can be replaced or
//! prevented, and then the card gets no Set. Queueing the destroy and
//! moving straight on would give the Set away for free, and would look
//! right on every board where nothing interferes.
//!
//! ## `Duel.SSet` is the last statement, so it does not need to suspend
//!
//! The reference yields on it like the others, but its result is
//! discarded and nothing follows. Queueing it and returning finishes in
//! the same processor order, which is what the port does.
//!
//! ## An exception that never excludes anything
//!
//! Both scans pass `e:GetHandler()` as the exception while scanning
//! `s = 0, o = LOCATION_ONFIELD` — the opponent's field only. Dust Tornado
//! is on its controller's field when it resolves, so the exception can
//! never match. It is transcribed anyway: the reference's argument list
//! is the specification, and a scan that is one-sided *today* because of
//! its masks should not quietly depend on that.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::{timing, Field};
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 60_082_869;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Destroy 1 Spell/Trap the opponent controls, then optionally Set
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::DESTROY | category::SET);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_property(f, e1, flag::CARD_TARGET, 0);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_hint_timing(f, e1, 0, timing::END_PHASE);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

const ONFIELD: u32 = location::ONFIELD as u32;
const HAND: u32 = location::HAND as u32;

fn filter(f: &mut Field, c: CardId) -> bool {
    api::is_spell_trap(f, c)
}

/// `Card.IsSSetable` with the reference's defaults: not ignoring the
/// field, and for `core.reason_player`.
fn ssetable(f: &mut Field, c: CardId) -> bool {
    api::is_ssetable(f, c, false, None)
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    let handler = api::get_handler(f, ctx.reason_effect);
    if let Some(chkc) = chkc {
        // Three tests, and the middle one is what makes this card's
        // reach different from Mystical Space Typhoon's: the opponent's
        // side only.
        return api::yes(
            api::is_on_field(f, chkc)
                && api::is_controler(f, chkc, 1 - tp)
                && api::is_spell_trap(f, chkc),
        );
    }
    let except = handler.map_or(api::Except::None, api::Except::Card);
    if !chk {
        return api::yes(api::is_existing_target(
            f,
            Some(&filter),
            tp,
            0,
            ONFIELD,
            1,
            except,
        ));
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::DESTROY);
    api::select_target(f, tp, Some(&filter), tp, 0, ONFIELD, 1, 1, except);
    api::suspend(move |f, _| {
        if let Some(g) = api::selected_targets(f) {
            // The recorded player is **zero**, not `tp` — the script's
            // literal, where Mystical Space Typhoon passes `tp`.
            api::set_operation_info(f, 0, category::DESTROY, Some(g), 1, 0, 0);
        }
        api::yes(true)
    })
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if !api::is_relate_to_effect(f, tc, ctx.reason_effect) {
        return api::done();
    }
    api::destroy(f, vec![tc], reason::EFFECT);
    api::suspend(move |f, ctx| {
        // `Duel.Destroy(...) ~= 0`: how many actually went.
        if api::resumed_value(f) == 0 {
            return api::done();
        }
        let tp = ctx.player;
        let g = api::get_matching_group(f, Some(&ssetable), tp, HAND, 0, api::Except::None);
        if g.is_empty() {
            return api::done();
        }
        api::select_yes_no(f, tp, api::stringid(CODE, 0));
        api::suspend(move |f, ctx| {
            if api::resumed_value(f) == 0 {
                return api::done();
            }
            let tp = ctx.player;
            // The group is re-scanned rather than carried across the
            // question: the reference's `g` is a live group object, and
            // answering the prompt is a point at which the hand can have
            // changed.
            let g = api::get_matching_group(f, Some(&ssetable), tp, HAND, 0, api::Except::None);
            if g.is_empty() {
                return api::done();
            }
            api::break_effect(f);
            api::hint(f, hint::SELECTMSG, tp, hintmsg::SET);
            api::group_select(f, &g, tp, 1, 1, api::Except::None);
            api::suspend(move |f, ctx| {
                let sg = api::group_selected(f);
                // `sg:GetFirst()` — one card, and nothing if the
                // selection came back empty.
                if let Some(&first) = sg.first() {
                    api::sset(f, ctx.player, vec![first], Some(ctx.player), false);
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
    use crate::duel::phases;
    use crate::event::Event;
    use crate::field::Message;
    use crate::position;
    use crate::processor::{Kind, Status};

    fn put(f: &mut Field, owner: u8, code_: u32, type_: u32, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        f.cards[id].current.position = position::FACEUP;
        let fid = f.next_field_id_raw();
        f.cards[id].fieldid = fid;
        f.cards[id].fieldid_r = fid;
        id
    }

    /// Player 0's Main Phase 1 with Dust Tornado face-up in its own row —
    /// where it sits once activated — `theirs` Spell/Traps of player 1's
    /// to aim at, `mine` more of player 0's, and `hand` Traps in player
    /// 0's hand for the Set half.
    fn field(theirs: u32, mine: u32, hand: u32) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        field_as(0, theirs, mine, hand)
    }

    /// The same board with the tornado under `tp`'s control, for the
    /// places where "the activating player" and "player zero" must not be
    /// allowed to coincide.
    fn field_as(
        tp: u8,
        theirs: u32,
        mine: u32,
        hand: u32,
    ) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let dt = put(&mut f, tp, CODE, card_type::TRAP, location::SZONE, 0);
        f.initialize_card(dt);
        let targets = (0..theirs)
            .map(|i| {
                put(
                    &mut f,
                    1 - tp,
                    8_000 + i,
                    card_type::TRAP,
                    location::SZONE,
                    i,
                )
            })
            .collect();
        for i in 0..mine {
            put(
                &mut f,
                tp,
                8_500 + i,
                card_type::TRAP,
                location::SZONE,
                i + 1,
            );
        }
        let in_hand = (0..hand)
            .map(|i| put(&mut f, tp, 8_900 + i, card_type::TRAP, location::HAND, i))
            .collect();
        (f, dt, targets, in_hand)
    }

    fn effect_of(f: &Field, dt: CardId) -> crate::event::EffectId {
        f.cards[dt].field_effect.equal_range(code::FREE_CHAIN)[0]
    }

    /// `target`, asked as a plain yes/no. The executor sets
    /// `core.reason_effect` around every call to a card's function and the
    /// targeting scan reads it, so a direct call has to set it too.
    fn asks(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> bool {
        f.core.reason_effect = Some(ctx.reason_effect);
        f.core.reason_player = ctx.player;
        let answer = target(f, ctx, chk, chkc).finished().unwrap_or(0) != 0;
        f.core.reason_effect = None;
        answer
    }

    fn ctx_for(e: crate::event::EffectId, ev: &Event) -> Ctx<'_> {
        Ctx {
            reason_effect: e,
            player: 0,
            event: ev,
            card: None,
            args: &[],
        }
    }

    /// What one driven resolution saw.
    struct Run {
        /// `(player, description)` of each bare yes/no.
        yes_no: Vec<(u8, u64)>,
        /// The cards offered at each card question.
        offered: Vec<Vec<CardId>>,
        /// `(player, min, max)` of each card question — what the answer
        /// alone cannot show.
        asked: Vec<(u8, u8, u8)>,
    }

    /// Build a chain link for the effect, run its target and then its
    /// operation to completion, answering each card question with its
    /// `choice`-th card and each yes/no with `say_yes`.
    fn resolve(f: &mut Field, e: crate::event::EffectId, choice: usize, say_yes: bool) -> Run {
        resolve_as(f, 0, e, choice, say_yes)
    }

    fn resolve_as(
        f: &mut Field,
        tp: u8,
        e: crate::event::EffectId,
        choice: usize,
        say_yes: bool,
    ) -> Run {
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
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
            yes_no: Vec::new(),
            offered: Vec::new(),
            asked: Vec::new(),
        };
        let mut operated = false;
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => {
                    // Both queues, not just the front one: the last thing
                    // the operation does is emplace the Set, which is
                    // still sitting in `subunits` at the moment the
                    // executor finishes.
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        if operated {
                            break;
                        }
                        // The target is finished; resolve for real.
                        //
                        // A second event, because the executor *drains*
                        // `sub_solving_event` onto the solving stack and
                        // pops it when it finishes: the target consumed
                        // the first one, and an operation that finds an
                        // empty stack returns without running its
                        // function at all. In a real duel the chain
                        // solver supplies one per executor.
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
                        player,
                        min,
                        max,
                        cards,
                        ..
                    }) => {
                        run.asked.push((*player, *min, *max));
                        let min = usize::from(*min);
                        run.offered.push(cards.clone());
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, (choice + i) as i32);
                        }
                    }
                    // Where the Set card goes. The mask marks the seats
                    // that are taken, so the first clear bit is a free
                    // one.
                    Some(Message::SelectPlace { player, flag, .. }) => {
                        let (pl, flag) = (*player, *flag);
                        let seq = (8..13u32)
                            .find(|s| flag & (1 << s) == 0)
                            .expect("a free Spell/Trap seat")
                            - 8;
                        f.core.returns.set_i8(0, pl as i8);
                        f.core.returns.set_i8(1, location::SZONE as i8);
                        f.core.returns.set_i8(2, seq as i8);
                    }
                    Some(Message::SelectYesNo {
                        player,
                        description,
                        ..
                    }) => {
                        run.yes_no.push((*player, *description));
                        f.core.returns.set(i32::from(say_yes));
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        run
    }

    /// **One printed activate effect that targets**, with the destroy and
    /// set categories and the opponent-side End Phase timing.
    #[test]
    fn the_script_registers_a_targeting_activate_effect() {
        let (f, dt, _, _) = field(0, 0, 0);
        let ids = f.cards[dt].field_effect.equal_range(code::FREE_CHAIN);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(e.is_flag(flag::CARD_TARGET), "it targets");
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert_eq!(
            e.category,
            category::DESTROY | category::SET,
            "both halves of the card are declared"
        );
        assert_eq!(
            e.hint_timing,
            [0, timing::END_PHASE],
            "nothing on its own side; the End Phase on the opponent's"
        );
    }

    /// **The third question is three tests**, and the middle one is the
    /// side: Mystical Space Typhoon would accept a Spell or Trap of the
    /// activating player's, and this one never does.
    #[test]
    fn the_third_question_wants_the_opponents_side() {
        let (mut f, dt, theirs, _) = field(1, 1, 0);
        let mine = f.players[0].szone[1].expect("a second card of mine");
        let monster = put(&mut f, 1, 9_000, card_type::MONSTER, location::MZONE, 0);
        let e = effect_of(&f, dt);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = ctx_for(e, &ev);
        assert!(asks(&mut f, &ctx, false, Some(theirs[0])), "theirs");
        assert!(
            !asks(&mut f, &ctx, false, Some(mine)),
            "a Trap of my own is the wrong side"
        );
        assert!(!asks(&mut f, &ctx, false, Some(dt)), "never itself");
        assert!(!asks(&mut f, &ctx, false, Some(monster)), "not a monster");
        f.cards[theirs[0]].current.location = location::GRAVE;
        assert!(
            !asks(&mut f, &ctx, false, Some(theirs[0])),
            "not off the field"
        );
    }

    /// **The scan is one-sided too.** A row full of the activating
    /// player's own Spell/Traps offers nothing; one card of the
    /// opponent's is enough.
    #[test]
    fn it_is_not_offered_against_my_own_row() {
        let (mut f, dt, _, _) = field(0, 3, 0);
        let e = effect_of(&f, dt);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = ctx_for(e, &ev);
        assert!(!asks(&mut f, &ctx, false, None), "all of them are mine");
        let (mut f, dt, _, _) = field(1, 3, 0);
        let e = effect_of(&f, dt);
        let ctx = ctx_for(e, &ev);
        assert!(asks(&mut f, &ctx, false, None), "one of theirs is enough");
    }

    /// **Destroy, then the offer, then the Set** — the whole body, end to
    /// end through three suspensions.
    #[test]
    fn it_destroys_then_offers_and_sets() {
        let (mut f, dt, theirs, in_hand) = field(2, 0, 2);
        let e = effect_of(&f, dt);
        let run = resolve(&mut f, e, 0, true);
        assert_eq!(
            f.cards[theirs[0]].current.location,
            location::GRAVE,
            "the chosen Spell/Trap is destroyed"
        );
        assert_eq!(
            run.yes_no,
            vec![(0, api::stringid(CODE, 0))],
            "the controller is asked once, with the card's own prompt"
        );
        assert_eq!(run.offered.len(), 2, "a target, then a card to Set");
        let mut want = in_hand.clone();
        want.sort_unstable();
        assert_eq!(run.offered[1], want, "the Set offer is the hand");
        assert_eq!(
            f.cards[in_hand[0]].current.location,
            location::SZONE,
            "and the chosen one is Set"
        );
        assert_eq!(
            f.cards[in_hand[0]].current.position,
            position::FACEDOWN,
            "Set, not played"
        );
        assert_eq!(
            f.cards[in_hand[1]].current.location,
            location::HAND,
            "only one"
        );
    }

    /// **Declining the offer keeps the hand.** The destroy still happens
    /// — the two halves are not one action.
    #[test]
    fn declining_the_offer_leaves_the_hand_alone() {
        let (mut f, dt, theirs, in_hand) = field(1, 0, 2);
        let e = effect_of(&f, dt);
        let run = resolve(&mut f, e, 0, false);
        assert_eq!(f.cards[theirs[0]].current.location, location::GRAVE);
        assert_eq!(run.yes_no.len(), 1, "asked");
        assert_eq!(run.offered.len(), 1, "but never shown the hand");
        for &h in &in_hand {
            assert_eq!(f.cards[h].current.location, location::HAND);
        }
    }

    /// **An empty hand is never asked at all.** `#g > 0` guards the
    /// question, so a controller with nothing to Set is not prompted.
    #[test]
    fn an_empty_hand_is_not_asked() {
        let (mut f, dt, theirs, _) = field(1, 0, 0);
        let e = effect_of(&f, dt);
        let run = resolve(&mut f, e, 0, true);
        assert_eq!(f.cards[theirs[0]].current.location, location::GRAVE);
        assert!(run.yes_no.is_empty(), "nothing to Set, so nothing to ask");
    }

    /// **The selection is one-sided too**, not just the existence check.
    /// A row of the activating player's own Spell/Traps is never offered,
    /// and neither is the card itself.
    #[test]
    fn the_selection_offers_only_the_opponents_side() {
        let (mut f, _, theirs, _) = field(2, 2, 0);
        let e = effect_of(&f, f.players[0].szone[0].expect("the tornado"));
        let run = resolve(&mut f, e, 0, false);
        let mut want = theirs.clone();
        want.sort_unstable();
        assert_eq!(run.offered[0], want, "only theirs");
        assert_eq!(run.asked[0], (0, 1, 1), "the controller picks one");
    }

    /// **The recorded operation names player zero**, which is the
    /// script's literal — Mystical Space Typhoon passes `tp` in the same
    /// slot, so the two are easy to conflate.
    #[test]
    fn the_recorded_operation_names_player_zero() {
        let (mut f, dt, theirs, _) = field(2, 0, 0);
        let e = effect_of(&f, dt);
        resolve(&mut f, e, 1, false);
        let op = f.core.current_chain[0]
            .opinfos
            .get(&category::DESTROY)
            .cloned()
            .expect("a DESTROY operation");
        assert_eq!(op.cards, Some(vec![theirs[1]]), "the one that was picked");
        assert_eq!(op.count, 1);
        assert_eq!(op.player, 0, "zero, not the activating player");
    }

    /// **The recorded player is zero even when the activating player is
    /// not.** With player 1 holding the tornado, `tp` is 1 and the slot
    /// still reads zero — which is the only board on which the script's
    /// literal and `tp` can be told apart. Mystical Space Typhoon passes
    /// `tp` in the same slot, so the two are easy to conflate, and a test
    /// run only as player 0 cannot see the difference.
    #[test]
    fn the_recorded_player_is_zero_even_for_player_one() {
        let (mut f, dt, theirs, _) = field_as(1, 2, 0, 0);
        let e = effect_of(&f, dt);
        resolve_as(&mut f, 1, e, 0, false);
        let op = f.core.current_chain[0]
            .opinfos
            .get(&category::DESTROY)
            .cloned()
            .expect("a DESTROY operation");
        assert_eq!(op.cards, Some(vec![theirs[0]]));
        assert_eq!(op.player, 0, "the script's literal, not the player");
    }

    /// **A destruction that does not happen earns no Set.** The `~= 0` on
    /// `Duel.Destroy` is the guard; a card already on its way out is not
    /// destroyed again, and the count comes back zero.
    #[test]
    fn a_destruction_that_did_nothing_earns_no_offer() {
        let (mut f, dt, theirs, in_hand) = field(1, 0, 2);
        let e = effect_of(&f, dt);
        // Already being destroyed by something else: the batch is empty
        // and the count is zero.
        f.cards[theirs[0]].set_status(status::DESTROY_CONFIRMED, true);
        let run = resolve(&mut f, e, 0, true);
        assert!(
            run.yes_no.is_empty(),
            "nothing was destroyed, so nothing is offered: {:?}",
            run.yes_no
        );
        for &h in &in_hand {
            assert_eq!(f.cards[h].current.location, location::HAND);
        }
    }

    /// **A target that lost its relation is spared**, and the second half
    /// never runs either — the whole body sits inside that `if`.
    #[test]
    fn a_target_that_lost_its_relation_is_spared() {
        let (mut f, dt, theirs, in_hand) = field(1, 0, 2);
        let e = effect_of(&f, dt);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        ch.target_cards = vec![theirs[0]];
        f.core.current_chain.push(ch);
        // No relation was ever recorded on the card.
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
                Status::Awaiting => panic!("nothing should be asked"),
                _ => break,
            }
        }
        assert_eq!(
            f.cards[theirs[0]].current.location,
            location::SZONE,
            "not destroyed"
        );
        for &h in &in_hand {
            assert_eq!(f.cards[h].current.location, location::HAND, "and no Set");
        }
    }

    /// **The hand scan is `IsSSetable`, not the whole hand.** A monster
    /// cannot be Set in a Spell/Trap Zone, so it is not offered — and
    /// with only monsters in hand the question is never asked.
    #[test]
    fn the_hand_scan_takes_only_what_can_be_set() {
        let (mut f, dt, _, in_hand) = field(1, 0, 1);
        let monster = put(&mut f, 0, 9_100, card_type::MONSTER, location::HAND, 1);
        let e = effect_of(&f, dt);
        let run = resolve(&mut f, e, 0, true);
        assert_eq!(run.offered.len(), 2, "a target, then the hand");
        assert_eq!(
            run.offered[1],
            vec![in_hand[0]],
            "the Trap only; a monster is not Settable"
        );
        assert_eq!(f.cards[monster].current.location, location::HAND);

        // And with nothing but monsters, no question at all.
        let (mut f, dt, _, _) = field(1, 0, 0);
        put(&mut f, 0, 9_101, card_type::MONSTER, location::HAND, 0);
        let e = effect_of(&f, dt);
        let run = resolve(&mut f, e, 0, true);
        assert!(run.yes_no.is_empty(), "a hand of monsters is not asked");
    }

    /// **`IsSSetable` honours the field.** With the Spell/Trap row full
    /// there is nowhere to put a card, so the hand offers nothing however
    /// many Traps it holds. This is the state a real duel spends most of
    /// its time in, which is why the `ignore_field` argument matters.
    #[test]
    fn a_full_row_offers_nothing() {
        let (mut f, dt, _, _) = field(1, 4, 3);
        let e = effect_of(&f, dt);
        let run = resolve(&mut f, e, 0, true);
        assert!(
            run.yes_no.is_empty(),
            "five seats taken, so nothing can be Set: {:?}",
            run.yes_no
        );
    }

    /// **The Set half breaks the timing window**, asks the *controller*,
    /// prompts with the Set message, and does not reveal the card.
    #[test]
    fn the_set_half_breaks_the_window_and_asks_quietly() {
        let (mut f, dt, _, _) = field(1, 0, 2);
        let e = effect_of(&f, dt);
        f.core.hint_timing = [timing::MAIN_END, timing::MAIN_END];
        let run = resolve(&mut f, e, 0, true);
        assert_eq!(
            f.core.hint_timing[0] & timing::MAIN_END,
            0,
            "Duel.BreakEffect cleared the window"
        );
        assert_eq!(run.asked[1], (0, 1, 1), "the controller chooses one card");
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::Hint { kind, player: 0, value }
                    if *kind == hint::SELECTMSG && *value == hintmsg::SET
            )),
            "prompted with the Set message, not the destroy one"
        );
        assert!(
            !f.messages
                .iter()
                .any(|m| matches!(m, Message::ConfirmCards { .. })),
            "a Set card is not revealed: the export is called with confirm = false"
        );
    }
}
