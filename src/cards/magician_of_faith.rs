//! Magician of Faith — `c31560081.lua`.
//!
//! The nineteenth card, and the first to bring something **back** —
//! everything in the pool so far has destroyed, drawn, banished, moved or
//! Set, and this one takes a Spell out of the graveyard and returns it to
//! the hand, then shows it to the opponent.
//!
//! ## The graveyard is a targeting location like any other
//!
//! `Duel.SelectTarget(tp, s.filter, tp, LOCATION_GRAVE, 0, 1, 1, nil)` —
//! the same export the field-targeting cards use, with `LOCATION_GRAVE`
//! in the `s` slot and **zero** in the `o` slot. One-sided: it can only
//! reach its own graveyard. The targeting scan needed nothing new to
//! reach there; only the masks change.
//!
//! ## `IsAbleToHand` is part of the filter, not a separate check
//!
//! `c:IsSpell() and c:IsAbleToHand()`. A Spell that cannot be returned is
//! not a legal target at all, so it never appears in the selection rather
//! than being chosen and then quietly failing. The same pairing appears
//! in `chkc`, which re-asks the whole filter alongside the location and
//! the controller.
//!
//! ## `chk == 0` is unconditional, and that is deliberate
//!
//! `return true`, with no `IsExistingTarget` — the same shape as
//! Man-Eater Bug and for the same reason: a flip effect has already
//! happened by the time it is asked, so there is nothing to refuse. An
//! empty graveyard simply produces a selection that finds nobody.
//!
//! ## The reveal has to wait for the return
//!
//! ```lua
//! Duel.SendtoHand(tc,nil,REASON_EFFECT)
//! Duel.ConfirmCards(1-tp,tc)
//! ```
//!
//! `Duel.SendtoHand` yields, so in the reference the card is **in the
//! hand** by the time `ConfirmCards` runs. That ordering is not
//! decoration: `ConfirmCards` raises `EVENT_TOHAND_CONFIRM` only for a
//! card whose location is the hand, so a port that queued the send and
//! revealed immediately would reveal a card still in the graveyard and
//! raise the wrong event. Hence the suspension between the two.
//!
//! The `nil` second argument is the destination hand, and it means **the
//! owner's**, not the controller's.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, CardId};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 31_560_081;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // flip
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::TOHAND);
    api::set_property(f, e1, flag::CARD_TARGET, 0);
    api::set_type(f, e1, effect_type::SINGLE | effect_type::FLIP);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, operation);
    api::register_effect(f, c, e1, false);
}

const GRAVE: u32 = location::GRAVE as u32;

fn filter(f: &mut Field, c: CardId) -> bool {
    api::is_spell(f, c) && api::is_able_to_hand(f, c, None)
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if let Some(chkc) = chkc {
        return api::yes(
            api::is_location(f, chkc, u16::from(location::GRAVE))
                && api::is_controler(f, chkc, tp)
                && filter(f, chkc),
        );
    }
    if !chk {
        return api::yes(true);
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::ATOHAND);
    api::select_target(f, tp, Some(&filter), tp, GRAVE, 0, 1, 1, api::Except::None);
    api::suspend(move |f, _| {
        if let Some(g) = api::selected_targets(f) {
            let n = g.len() as u8;
            api::set_operation_info(f, 0, category::TOHAND, Some(g), n, 0, 0);
        }
        api::yes(true)
    })
}

fn operation(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if !api::is_relate_to_effect(f, tc, ctx.reason_effect) {
        return api::done();
    }
    api::send_to_hand(f, vec![tc], None, reason::EFFECT);
    api::suspend(move |f, ctx| {
        // Only now is it in the hand, which is what decides whether the
        // reveal raises `EVENT_TOHAND_CONFIRM` as well as `EVENT_CONFIRM`.
        api::confirm_cards(f, 1 - ctx.player, vec![tc]);
        api::done()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::{code, Event};
    use crate::field::Message;
    use crate::position;
    use crate::processor::{Kind, Status};

    fn put(f: &mut Field, owner: u8, code_: u32, type_: u32, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_,
                level: 4,
                attack: 1000,
                defense: 1000,
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

    /// `tp`'s Main Phase 1, the magician face-up in `tp`'s monster row —
    /// where it sits once flipped — with `spells` Spells in `tp`'s
    /// graveyard and `theirs` Spells in the opponent's.
    fn field_as(tp: u8, spells: u32, theirs: u32) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mof = put(
            &mut f,
            tp,
            CODE,
            card_type::MONSTER | card_type::EFFECT | card_type::FLIP,
            location::MZONE,
            0,
        );
        f.initialize_card(mof);
        let mine = (0..spells)
            .map(|i| put(&mut f, tp, 6_000 + i, card_type::SPELL, location::GRAVE, i))
            .collect();
        let others = (0..theirs)
            .map(|i| {
                put(
                    &mut f,
                    1 - tp,
                    6_500 + i,
                    card_type::SPELL,
                    location::GRAVE,
                    i,
                )
            })
            .collect();
        (f, mof, mine, others)
    }

    fn field(spells: u32, theirs: u32) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        field_as(0, spells, theirs)
    }

    fn effect_of(f: &Field, mof: CardId) -> crate::event::EffectId {
        f.cards[mof].single_effect.equal_range(code::FLIP).to_vec()[0]
    }

    fn ctx_for(e: crate::event::EffectId, ev: &Event, tp: u8) -> Ctx<'_> {
        Ctx {
            reason_effect: e,
            player: tp,
            event: ev,
            card: None,
            args: &[],
        }
    }

    /// `target`, asked as a plain yes/no, with the executor's bookkeeping
    /// set up the way a real call has it.
    fn asks(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> bool {
        f.core.reason_effect = Some(ctx.reason_effect);
        f.core.reason_player = ctx.player;
        let answer = target(f, ctx, chk, chkc).finished().unwrap_or(0) != 0;
        f.core.reason_effect = None;
        answer
    }

    /// What one driven resolution saw.
    struct Run {
        offered: Vec<Vec<CardId>>,
        /// `(player, min, max)` of each card question.
        asked: Vec<(u8, u8, u8)>,
    }

    fn resolve_as(f: &mut Field, tp: u8, e: crate::event::EffectId, choice: usize) -> Run {
        let mut ch = Chain::new(e, Event::new(code::FLIP));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.core.sub_solving_event.push_back(Event::new(code::FLIP));
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
            asked: Vec::new(),
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
                        f.core.sub_solving_event.push_back(Event::new(code::FLIP));
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
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        run
    }

    fn resolve(f: &mut Field, e: crate::event::EffectId, choice: usize) -> Run {
        resolve_as(f, 0, e, choice)
    }

    /// **One printed flip effect that targets**, with the to-hand
    /// category and its own description.
    #[test]
    fn the_script_registers_a_targeting_flip_effect() {
        let (f, mof, _, _) = field(0, 0);
        let ids = f.cards[mof].single_effect.equal_range(code::FLIP).to_vec();
        assert_eq!(ids.len(), 1, "one single effect");
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::SINGLE));
        assert!(e.is_type(effect_type::FLIP), "it fires on being flipped");
        assert!(e.is_flag(flag::CARD_TARGET), "it targets");
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert_eq!(e.category, category::TOHAND);
        assert_eq!(e.description, api::stringid(CODE, 0));
        assert_eq!(e.handler, Some(mof));
    }

    /// **The filter is a Spell that can actually come back.** A Trap is
    /// not a Spell, a monster is not, and a Spell that may not be
    /// returned is filtered out rather than offered and then failing.
    #[test]
    fn the_filter_wants_a_returnable_spell() {
        let (mut f, _, mine, _) = field(1, 0);
        let trap = put(&mut f, 0, 7_001, card_type::TRAP, location::GRAVE, 1);
        let monster = put(&mut f, 0, 7_002, card_type::MONSTER, location::GRAVE, 2);
        let stuck = put(&mut f, 0, 7_003, card_type::SPELL, location::GRAVE, 3);
        f.cards[stuck].set_status(status::LEAVE_CONFIRMED, true);
        assert!(filter(&mut f, mine[0]), "an ordinary Spell in the grave");
        assert!(!filter(&mut f, trap), "a Trap is not a Spell");
        assert!(!filter(&mut f, monster), "nor is a monster");
        assert!(
            !filter(&mut f, stuck),
            "a Spell already on its way out cannot be returned"
        );
    }

    /// **The third question is three clauses**, each refused on its own,
    /// and the positive that shows the test is not vacuous.
    #[test]
    fn the_third_question_wants_my_graveyard() {
        let (mut f, mof, mine, theirs) = field(1, 1);
        let on_field = put(&mut f, 0, 7_010, card_type::SPELL, location::SZONE, 0);
        let trap = put(&mut f, 0, 7_011, card_type::TRAP, location::GRAVE, 2);
        let e = effect_of(&f, mof);
        let ev = Event::new(code::FLIP);
        let ctx = ctx_for(e, &ev, 0);
        assert!(asks(&mut f, &ctx, false, Some(mine[0])), "my own grave");
        assert!(
            !asks(&mut f, &ctx, false, Some(theirs[0])),
            "the opponent's graveyard is out of reach"
        );
        assert!(
            !asks(&mut f, &ctx, false, Some(on_field)),
            "a Spell on the field is the wrong location"
        );
        assert!(!asks(&mut f, &ctx, false, Some(trap)), "and not a Trap");
    }

    /// **`chk == 0` is unconditional**, exactly as the script writes it:
    /// an empty graveyard still answers yes, because a flip effect has
    /// already happened by the time it is asked.
    #[test]
    fn the_scan_is_never_refused() {
        let (mut f, mof, _, _) = field(0, 0);
        let e = effect_of(&f, mof);
        let ev = Event::new(code::FLIP);
        let ctx = ctx_for(e, &ev, 0);
        assert!(
            asks(&mut f, &ctx, false, None),
            "yes even with nothing to take"
        );
        let (mut f, mof, _, _) = field(2, 0);
        let e = effect_of(&f, mof);
        let ctx = ctx_for(e, &ev, 0);
        assert!(asks(&mut f, &ctx, false, None));
    }

    /// **The selection reaches only my graveyard**, and offers only what
    /// the filter allows — asked of the controller, one card exactly.
    #[test]
    fn the_selection_is_my_graveyard_only() {
        let (mut f, mof, mine, theirs) = field(2, 2);
        put(&mut f, 0, 7_020, card_type::TRAP, location::GRAVE, 4);
        let e = effect_of(&f, mof);
        let run = resolve(&mut f, e, 0);
        let mut offered = run.offered[0].clone();
        offered.sort_unstable();
        let mut want = mine.clone();
        want.sort_unstable();
        assert_eq!(offered, want, "my Spells only");
        for t in &theirs {
            assert!(!run.offered[0].contains(t), "not the opponent's");
        }
        assert_eq!(run.asked[0], (0, 1, 1), "the controller picks exactly one");
    }

    /// **The prompt is the add-to-hand message**, to the controller.
    #[test]
    fn the_prompt_is_the_add_to_hand_message() {
        let (mut f, mof, _, _) = field(2, 0);
        let e = effect_of(&f, mof);
        resolve(&mut f, e, 0);
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::Hint { kind, player: 0, value }
                    if *kind == hint::SELECTMSG && *value == hintmsg::ATOHAND
            )),
            "HINTMSG_ATOHAND to the activating player"
        );
    }

    /// **The recorded operation is a to-hand for player zero** — the
    /// script's literal `0`, which is only distinguishable from `tp` when
    /// the activating player is player 1.
    #[test]
    fn the_recorded_operation_is_a_to_hand_for_player_zero() {
        for tp in [0u8, 1] {
            let (mut f, mof, mine, _) = field_as(tp, 2, 0);
            let e = effect_of(&f, mof);
            let run = resolve_as(&mut f, tp, e, 1);
            // Which card that was is the *offer's* order, not sorted
            // order: the graveyard scan hands them over as the pile holds
            // them, and a test that assumed otherwise would be asserting
            // about the wrong card.
            let picked = run.offered[0][1];
            assert!(mine.contains(&picked));
            let op = f.core.current_chain[0]
                .opinfos
                .get(&category::TOHAND)
                .cloned()
                .unwrap_or_else(|| panic!("a TOHAND operation for tp={tp}"));
            assert_eq!(op.cards, Some(vec![picked]), "the one that was picked");
            assert_eq!(op.count, 1);
            assert_eq!(op.player, 0, "the script's literal, not tp={tp}");
        }
    }

    /// **The Spell comes back to its owner's hand, face-up, and is shown
    /// to the opponent.**
    #[test]
    fn it_returns_the_spell_and_reveals_it() {
        let (mut f, mof, mine, _) = field(2, 0);
        let e = effect_of(&f, mof);
        let run = resolve(&mut f, e, 0);
        let taken = run.offered[0][0];
        let left = *mine.iter().find(|&&c| c != taken).expect("the other one");
        assert_eq!(f.cards[taken].current.location, location::HAND);
        assert_eq!(f.cards[taken].current.controller, 0, "its owner's hand");
        assert_eq!(
            f.cards[taken].current.position,
            position::FACEDOWN,
            "face-down in the hand: `Duel.SendtoHand` asks for face-up, and              `add_card` overrules it for the hand unless the card is public"
        );
        assert!(
            f.cards[taken].reason & reason::EFFECT != 0,
            "returned by an effect, not paid as a cost"
        );
        assert_eq!(
            f.cards[left].current.location,
            location::GRAVE,
            "only the one"
        );
        let code_of = f.cards[taken].data.code;
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::ConfirmCards { player: 1, codes } if codes == &vec![code_of]
            )),
            "revealed to the opponent, not to its controller"
        );
        assert_eq!(f.cards[mof].current.location, location::MZONE);
    }

    /// **The reveal names the opponent even when the opponent is player
    /// zero.** `1 - tp`, which reads the same as `tp` on a board where
    /// the activating player is 0.
    #[test]
    fn the_reveal_names_the_opponent_for_either_player() {
        let (mut f, mof, mine, _) = field_as(1, 1, 0);
        let e = effect_of(&f, mof);
        resolve_as(&mut f, 1, e, 0);
        let code_of = f.cards[mine[0]].data.code;
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::ConfirmCards { player: 0, codes } if codes == &vec![code_of]
            )),
            "player 1 activated, so player 0 is shown the card"
        );
    }

    /// **A target that lost its relation is left alone**, and nothing is
    /// revealed either — the whole body sits inside that `if`.
    #[test]
    fn a_target_that_lost_its_relation_is_spared() {
        let (mut f, mof, mine, _) = field(1, 0);
        let e = effect_of(&f, mof);
        let mut ch = Chain::new(e, Event::new(code::FLIP));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        ch.target_cards = vec![mine[0]];
        f.core.current_chain.push(ch);
        f.core.sub_solving_event.push_back(Event::new(code::FLIP));
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
        assert_eq!(f.cards[mine[0]].current.location, location::GRAVE);
        assert!(
            !f.messages
                .iter()
                .any(|m| matches!(m, Message::ConfirmCards { .. })),
            "and nothing was revealed"
        );
    }

    /// **It goes to the *owner's* hand, not the controller's.**
    /// `Duel.SendtoHand(tc, nil, ...)` — the `nil` is the destination
    /// hand, and it means the owner's. The two coincide on every ordinary
    /// board, which is why the distinction needs a board where they do
    /// not: a Spell owned by player 1 sitting in player 0's graveyard.
    #[test]
    fn the_spell_goes_home_to_its_owner() {
        let (mut f, mof, _, _) = field(0, 0);
        let mut owned_by_one = Card::with_data(
            CardData {
                code: 6_777,
                type_: card_type::SPELL,
                ..Default::default()
            },
            1,
        );
        owned_by_one.current.controller = 0;
        owned_by_one.set_status(status::EFFECT_ENABLED, true);
        let spell = f.new_card(owned_by_one);
        f.add_card(0, spell, location::GRAVE, 0, false);
        f.cards[spell].current.position = position::FACEUP;
        let e = effect_of(&f, mof);
        let run = resolve(&mut f, e, 0);
        assert_eq!(run.offered[0], vec![spell], "in my graveyard, so reachable");
        assert_eq!(f.cards[spell].current.location, location::HAND);
        assert_eq!(
            f.cards[spell].current.controller, 1,
            "player 1 owns it, so player 1's hand"
        );
        assert_eq!(
            f.players[0].hand.len(),
            0,
            "and not the activating player's"
        );
    }

    /// **The reveal happens after the return**, which the message stream
    /// shows directly: the card's `MSG_MOVE` into the hand comes before
    /// `MSG_CONFIRM_CARDS`, not after it.
    ///
    /// The ordering is not cosmetic. `Duel.ConfirmCards` raises
    /// `EVENT_TOHAND_CONFIRM` only for a card that is *in a hand* when it
    /// is revealed; reveal first and the card is still in the graveyard,
    /// so that event never fires. But the reveal message and the final
    /// board are identical either way — the order of the two messages is
    /// where the difference is visible at all.
    #[test]
    fn the_reveal_comes_after_the_return() {
        let (mut f, mof, mine, _) = field(1, 0);
        let code_of = f.cards[mine[0]].data.code;
        let e = effect_of(&f, mof);
        resolve(&mut f, e, 0);
        assert_eq!(f.cards[mine[0]].current.location, location::HAND);
        let moved = f
            .messages
            .iter()
            .position(|m| matches!(m, Message::Move { code, .. } if *code == code_of))
            .expect("the card moved");
        let revealed = f
            .messages
            .iter()
            .position(
                |m| matches!(m, Message::ConfirmCards { codes, .. } if codes == &vec![code_of]),
            )
            .expect("the card was revealed");
        assert!(
            moved < revealed,
            "returned at message {moved}, revealed at {revealed} — the reveal must wait"
        );
    }
}
