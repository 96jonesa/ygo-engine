//! Nobleman of Crossout — `c71044499.lua`.
//!
//! The twenty-third card. It destroys a face-down monster **to the
//! banished pile** rather than the graveyard, and if that monster turns
//! out to be a Flip effect, banishes every copy of it from both decks
//! and shows both decks to their owners before shuffling.
//!
//! ## Destroyed *and* banished is one action, not two
//!
//! ```lua
//! Duel.Destroy(tc,REASON_EFFECT,LOCATION_REMOVED)
//! ```
//!
//! `Duel.Destroy`'s third argument is the destination, and it defaults to
//! the graveyard. Sending a destroyed card straight to the banished pile
//! is not the same as destroying it and then banishing it: it leaves as a
//! destruction, with one set of triggers, and never touches a graveyard
//! on the way. The port's `destroy` had hard-coded the graveyard because
//! nothing had needed otherwise.
//!
//! ## The deck sweep is conditional on two things
//!
//! `if Duel.Destroy(...) ~= 0 and tc:IsType(TYPE_FLIP)` — the
//! destruction has to have *happened*, and the card has to have been a
//! Flip monster. A face-down monster that turns out to be something else
//! is simply gone, and both decks are left alone.
//!
//! The type is read from the card **after** it has been destroyed, which
//! is the only time anyone may look at it: that is what the card is for.
//!
//! ## Both decks are shown, each to a different player
//!
//! ```lua
//! g=Duel.GetFieldGroup(tp,0,LOCATION_DECK)   Duel.ConfirmCards(tp,g)
//! g=Duel.GetFieldGroup(tp,LOCATION_DECK,0)   Duel.ConfirmCards(1-tp,g)
//! ```
//!
//! The masks are swapped between the two lines, and so are the players.
//! The activating player is shown the *opponent's* deck and the opponent
//! is shown the activating player's — each sees the deck that was just
//! searched through, which is the point of the reveal. Reading both
//! lines the same way round would show each player their own deck.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::{location, position};
use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 71_044_499;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::DESTROY | category::REMOVE);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_property(f, e1, flag::CARD_TARGET, 0);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

const MZONE: u32 = location::MZONE as u32;
const DECK: u32 = location::DECK as u32;

fn filter(f: &mut Field, c: CardId) -> bool {
    api::is_facedown(f, c) && api::is_able_to_remove(f, c, None, None, None)
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if let Some(chkc) = chkc {
        return api::yes(api::is_location(f, chkc, u16::from(location::MZONE)) && filter(f, chkc));
    }
    if !chk {
        return api::yes(api::is_existing_target(
            f,
            Some(&filter),
            tp,
            MZONE,
            MZONE,
            1,
            api::Except::None,
        ));
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::DESTROY);
    api::select_target(
        f,
        tp,
        Some(&filter),
        tp,
        MZONE,
        MZONE,
        1,
        1,
        api::Except::None,
    );
    api::suspend(move |f, _| {
        if let Some(g) = api::selected_targets(f) {
            // Two operations over the same card: it is destroyed, and it
            // is banished, and the opponent is told both.
            api::set_operation_info(f, 0, category::DESTROY, Some(g.clone()), 1, 0, 0);
            api::set_operation_info(f, 0, category::REMOVE, Some(g), 1, 0, 0);
        }
        api::yes(true)
    })
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if !api::is_facedown(f, tc) || !api::is_relate_to_effect(f, tc, ctx.reason_effect) {
        return api::done();
    }
    // Destroyed straight to the banished pile, not to a graveyard.
    api::destroy_to(f, vec![tc], reason::EFFECT, u16::from(location::REMOVED));
    api::suspend(move |f, ctx| {
        if api::resumed_value(f) == 0 {
            return api::done();
        }
        // Only now may anyone read what it was.
        if !api::is_type(f, tc, crate::card::card_type::FLIP) {
            return api::done();
        }
        let tp = ctx.player;
        let wanted = api::get_code(f, tc);
        let same_name = move |f: &mut Field, c: CardId| api::is_code(f, c, wanted);
        let g = api::get_matching_group(f, Some(&same_name), tp, DECK, DECK, api::Except::None);
        api::remove(f, g, position::FACEUP, reason::EFFECT);
        api::suspend(move |f, ctx| {
            let tp = ctx.player;
            // Each player is shown the deck that was just gone through —
            // which is the *other* one. The masks and the players are
            // both swapped between these two lines.
            let theirs = api::get_field_group(f, tp, 0, DECK);
            api::confirm_cards(f, tp, theirs);
            let mine = api::get_field_group(f, tp, DECK, 0);
            api::confirm_cards(f, 1 - tp, mine);
            api::shuffle_deck(f, tp);
            api::shuffle_deck(f, 1 - tp);
            api::done()
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
        f.cards[id].current.position = if loc == location::MZONE {
            position::FACEDOWN_DEFENSE
        } else {
            position::FACEUP
        };
        let fid = f.next_field_id_raw();
        f.cards[id].fieldid = fid;
        f.cards[id].fieldid_r = fid;
        id
    }

    /// `tp`'s Main Phase 1 with the Spell face-up in its row, `theirs`
    /// face-down monsters of the opponent's as `(code, type)`, and decks
    /// holding `deck` copies per player as `(code, type)`.
    fn field_as(
        tp: u8,
        theirs: &[(u32, u32)],
        deck: &[(u32, u32)],
    ) -> (Field, CardId, Vec<CardId>, Vec<CardId>, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let nc = put(&mut f, tp, CODE, card_type::SPELL, location::SZONE, 0);
        f.initialize_card(nc);
        let victims = theirs
            .iter()
            .enumerate()
            .map(|(i, &(code_, ty))| put(&mut f, 1 - tp, code_, ty, location::MZONE, i as u32))
            .collect();
        // The two decks hold the *same names* where the card cares about
        // names, and a distinct marker each where the tests need to tell
        // one deck's reveal from the other's.
        let mut mine_deck: Vec<CardId> = Vec::new();
        for &(code_, ty) in deck {
            mine_deck.push(put(&mut f, tp, code_, ty, location::DECK, 0));
        }
        mine_deck.push(put(&mut f, tp, 6_660, PLAIN, location::DECK, 0));
        let mut their_deck: Vec<CardId> = Vec::new();
        for &(code_, ty) in deck {
            their_deck.push(put(&mut f, 1 - tp, code_, ty, location::DECK, 0));
        }
        their_deck.push(put(&mut f, 1 - tp, 6_661, PLAIN, location::DECK, 0));
        (f, nc, victims, mine_deck, their_deck)
    }

    fn effect_of(f: &Field, nc: CardId) -> crate::event::EffectId {
        f.cards[nc].field_effect.equal_range(code::FREE_CHAIN)[0]
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

    fn asks(f: &mut Field, e: crate::event::EffectId, tp: u8, chkc: Option<CardId>) -> bool {
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
        confirms: Vec<(u8, Vec<u32>)>,
    }

    fn resolve_as(f: &mut Field, tp: u8, e: crate::event::EffectId, choice: usize) -> Run {
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
            confirms: Vec::new(),
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
            if let Message::ConfirmCards { player, codes } = m {
                run.confirms.push((*player, codes.clone()));
            }
        }
        run
    }

    fn resolve(f: &mut Field, e: crate::event::EffectId, choice: usize) -> Run {
        resolve_as(f, 0, e, choice)
    }

    const FLIP: u32 = card_type::MONSTER | card_type::EFFECT | card_type::FLIP;
    const PLAIN: u32 = card_type::MONSTER | card_type::NORMAL;

    /// **One printed activate effect that targets**, declaring both a
    /// destroy and a banish.
    #[test]
    fn the_script_registers_a_targeting_activate_effect() {
        let (f, nc, _, _, _) = field_as(0, &[], &[]);
        let ids = f.cards[nc].field_effect.equal_range(code::FREE_CHAIN);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(e.is_flag(flag::CARD_TARGET));
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert_eq!(
            e.category,
            category::DESTROY | category::REMOVE,
            "it destroys *and* banishes, and says both"
        );
    }

    /// **The filter wants a face-down monster that can be banished**, and
    /// the third question adds the location.
    #[test]
    fn the_filter_wants_a_face_down_that_can_be_banished() {
        let (mut f, nc, theirs, _, _) = field_as(0, &[(7_001, PLAIN)], &[]);
        let faceup = put(&mut f, 1, 7_002, PLAIN, location::MZONE, 1);
        f.cards[faceup].current.position = position::FACEUP_ATTACK;
        // Unbanishable, by an effect that says so — which is what
        // `is_removeable` actually consults.
        let stuck = put(&mut f, 1, 7_003, PLAIN, location::MZONE, 2);
        f.cards[stuck].current.position = position::FACEDOWN_DEFENSE;
        let mut ban = crate::effect::Effect::new(effect_type::SINGLE, code::CANNOT_REMOVE);
        ban.owner = Some(stuck);
        ban.handler = Some(stuck);
        let ban = f.new_effect(ban);
        f.cards[stuck]
            .single_effect
            .insert(code::CANNOT_REMOVE, ban);
        let e = effect_of(&f, nc);
        f.core.reason_player = 0;
        assert!(filter(&mut f, theirs[0]), "face-down and banishable");
        assert!(!filter(&mut f, faceup), "a face-up monster is not a target");
        assert!(!filter(&mut f, stuck), "and it has to be banishable");
        // The third question adds the location on top of the filter.
        assert!(asks(&mut f, e, 0, Some(theirs[0])));
        assert!(!asks(&mut f, e, 0, Some(faceup)));
        let in_hand = put(&mut f, 1, 7_004, PLAIN, location::HAND, 0);
        f.cards[in_hand].current.position = position::FACEDOWN;
        assert!(
            !asks(&mut f, e, 0, Some(in_hand)),
            "a face-down card in a hand is the wrong location"
        );
    }

    /// **The scan reaches both sides.** A face-down monster of the
    /// activating player's own is a legal target — the masks are
    /// `MZONE, MZONE`, not one-sided like Mirror Force's.
    #[test]
    fn the_scan_reaches_both_monster_rows() {
        let (mut f, nc, _, _, _) = field_as(0, &[], &[]);
        let mine = put(&mut f, 0, 7_010, PLAIN, location::MZONE, 0);
        f.cards[mine].current.position = position::FACEDOWN_DEFENSE;
        let e = effect_of(&f, nc);
        assert!(
            asks(&mut f, e, 0, None),
            "my own face-down counts: this card is not one-sided"
        );
        assert!(asks(&mut f, e, 0, Some(mine)));
    }

    /// **The scan reaches the opponent's row too.** Only their face-down
    /// on the board, and it is still a legal activation — the `o` mask is
    /// `MZONE`, not zero.
    #[test]
    fn the_scan_reaches_the_opponents_row() {
        let (mut f, nc, theirs, _, _) = field_as(0, &[(7_020, PLAIN)], &[]);
        let e = effect_of(&f, nc);
        assert!(
            asks(&mut f, e, 0, None),
            "the only face-down is the opponent's, and it counts"
        );
        assert!(asks(&mut f, e, 0, Some(theirs[0])));
    }

    /// **The prompt is the destroy message, to the activating player.**
    #[test]
    fn the_prompt_is_the_destroy_message() {
        let (mut f, nc, _, _, _) = field_as(0, &[(7_030, PLAIN)], &[]);
        let e = effect_of(&f, nc);
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

    /// **Both operations are recorded over the same card** — it is
    /// destroyed *and* banished, and the opponent is told both.
    #[test]
    fn both_operations_are_recorded() {
        let (mut f, nc, theirs, _, _) = field_as(0, &[(7_040, PLAIN)], &[]);
        let e = effect_of(&f, nc);
        resolve(&mut f, e, 0);
        let link = &f.core.current_chain[0];
        for cat in [category::DESTROY, category::REMOVE] {
            let op = link
                .opinfos
                .get(&cat)
                .cloned()
                .unwrap_or_else(|| panic!("an operation for {cat:#x}"));
            assert_eq!(op.cards, Some(vec![theirs[0]]));
            assert_eq!(op.count, 1);
            assert_eq!(op.player, 0);
        }
    }

    /// **A target that lost its relation is spared**, which is a
    /// different question from the face-down re-check beside it: this one
    /// is still face-down.
    #[test]
    fn a_target_that_lost_its_relation_is_spared() {
        let (mut f, nc, theirs, _, _) = field_as(0, &[(7_050, PLAIN)], &[]);
        let e = effect_of(&f, nc);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        ch.target_cards = vec![theirs[0]];
        f.core.current_chain.push(ch);
        // Still face-down; simply never related to this chain.
        assert!(api::is_facedown(&f, theirs[0]));
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
        assert_eq!(f.cards[theirs[0]].current.location, location::MZONE);
    }

    /// **A destruction that did not happen sweeps nothing.** The `~= 0`
    /// on `Duel.Destroy` is the guard; a monster immune to the effect
    /// stays put, and both decks are left alone even though it was a
    /// Flip monster.
    #[test]
    fn a_destruction_that_did_nothing_sweeps_nothing() {
        fn immune_to_everything(_: &crate::effect::Effect, _: &Field, _: &Ctx) -> i64 {
            1
        }
        let (mut f, nc, theirs, mine_deck, their_deck) =
            field_as(0, &[(7_060, FLIP)], &[(7_060, FLIP)]);
        let mut immunity = crate::effect::Effect::new(effect_type::SINGLE, code::IMMUNE_EFFECT);
        immunity.owner = Some(theirs[0]);
        immunity.handler = Some(theirs[0]);
        immunity.flag[0] |= crate::effect::flag::FUNC_VALUE;
        immunity.value_fn = Some(immune_to_everything);
        let immunity = f.new_effect(immunity);
        f.cards[theirs[0]].immune_effect.push(immunity);

        let e = effect_of(&f, nc);
        let run = resolve(&mut f, e, 0);
        assert_eq!(
            f.cards[theirs[0]].current.location,
            location::MZONE,
            "the destroy was refused"
        );
        assert_eq!(f.cards[mine_deck[0]].current.location, location::DECK);
        assert_eq!(f.cards[their_deck[0]].current.location, location::DECK);
        assert!(run.confirms.is_empty(), "and nothing was revealed");
    }

    /// **A monster with an effect but no flip effect sweeps nothing.**
    /// The test is `TYPE_FLIP`, not "has an effect".
    #[test]
    fn an_effect_monster_that_is_not_a_flip_sweeps_nothing() {
        let effect_only = card_type::MONSTER | card_type::EFFECT;
        let (mut f, nc, theirs, mine_deck, their_deck) =
            field_as(0, &[(7_070, effect_only)], &[(7_070, effect_only)]);
        let e = effect_of(&f, nc);
        let run = resolve(&mut f, e, 0);
        assert_eq!(f.cards[theirs[0]].current.location, location::REMOVED);
        assert_eq!(
            f.cards[mine_deck[0]].current.location,
            location::DECK,
            "an effect monster is not a Flip monster"
        );
        assert_eq!(f.cards[their_deck[0]].current.location, location::DECK);
        assert!(run.confirms.is_empty());
    }

    /// **A face-down that is not a Flip monster is simply banished**, and
    /// neither deck is touched.
    #[test]
    fn a_plain_face_down_is_banished_and_the_decks_are_left_alone() {
        let (mut f, nc, theirs, mine_deck, their_deck) =
            field_as(0, &[(7_100, PLAIN)], &[(7_100, PLAIN)]);
        let e = effect_of(&f, nc);
        let run = resolve(&mut f, e, 0);
        assert_eq!(
            f.cards[theirs[0]].current.location,
            location::REMOVED,
            "destroyed straight to the banished pile"
        );
        assert!(
            f.cards[theirs[0]].reason & reason::DESTROY != 0,
            "destroyed"
        );
        assert!(f.cards[theirs[0]].reason & reason::EFFECT != 0);
        for &c in mine_deck.iter().chain(their_deck.iter()) {
            assert_eq!(
                f.cards[c].current.location,
                location::DECK,
                "not a Flip monster, so no deck sweep"
            );
        }
        assert!(run.confirms.is_empty(), "and nothing is revealed");
    }

    /// **A Flip monster takes every copy of its name out of both
    /// decks**, and each player is shown the *other* player's deck.
    #[test]
    fn a_flip_monster_clears_both_decks_and_shows_them() {
        let (mut f, nc, theirs, mine_deck, their_deck) =
            field_as(0, &[(7_200, FLIP)], &[(7_200, FLIP), (7_201, PLAIN)]);
        let e = effect_of(&f, nc);
        let run = resolve(&mut f, e, 0);
        assert_eq!(f.cards[theirs[0]].current.location, location::REMOVED);
        assert_eq!(
            f.cards[mine_deck[0]].current.location,
            location::REMOVED,
            "my copy of the name goes too"
        );
        assert_eq!(
            f.cards[their_deck[0]].current.location,
            location::REMOVED,
            "and theirs"
        );
        assert_eq!(
            f.cards[mine_deck[1]].current.location,
            location::DECK,
            "a different name stays"
        );
        assert_eq!(f.cards[their_deck[1]].current.location, location::DECK);
        assert_eq!(
            f.cards[mine_deck[0]].current.position,
            position::FACEUP,
            "banished face-up, so both players see what went"
        );

        // Two reveals, and each player is shown the deck that is *not*
        // theirs.
        assert_eq!(
            run.confirms.len(),
            2,
            "both decks, once each: {:?}",
            run.confirms
        );
        let shown_to_zero: Vec<u32> = run
            .confirms
            .iter()
            .filter(|(p, _)| *p == 0)
            .flat_map(|(_, c)| c.clone())
            .collect();
        let shown_to_one: Vec<u32> = run
            .confirms
            .iter()
            .filter(|(p, _)| *p == 1)
            .flat_map(|(_, c)| c.clone())
            .collect();
        // Each deck carries a distinct marker, so the two reveals cannot
        // be told apart by accident.
        assert!(
            shown_to_zero.contains(&6_661) && !shown_to_zero.contains(&6_660),
            "player 0 is shown player 1's remaining deck: {shown_to_zero:?}"
        );
        assert!(
            shown_to_one.contains(&6_660) && !shown_to_one.contains(&6_661),
            "and player 1 is shown player 0's: {shown_to_one:?}"
        );
    }

    /// **Both decks are shuffled even when the sweep found nothing**,
    /// which is the only board on which the script's explicit
    /// `Duel.ShuffleDeck` calls are doing any work.
    ///
    /// Taking a card *out of* a deck already sets `shuffle_deck_check`
    /// (`field.cpp:223`, mirrored in `movement.rs`), and the executor
    /// shuffles every flagged deck when `check_level` falls to zero. So
    /// on a board where copies were found, the announcement happens
    /// whether the card asks for it or not. Here there are no copies to
    /// find, nothing leaves either deck, no flag is set — and the two
    /// announcements are the card's own.
    #[test]
    fn both_decks_are_shuffled_even_with_nothing_to_sweep() {
        // A Flip monster on the board and **no copies of its name** in
        // either deck: the sweep runs and removes nothing.
        let (mut f, nc, theirs, mine_deck, their_deck) = field_as(0, &[(7_310, FLIP)], &[]);
        let e = effect_of(&f, nc);
        resolve(&mut f, e, 0);
        assert_eq!(f.cards[theirs[0]].current.location, location::REMOVED);
        for &c in mine_deck.iter().chain(their_deck.iter()) {
            assert_eq!(
                f.cards[c].current.location,
                location::DECK,
                "nothing left either deck"
            );
        }
        let shuffles: Vec<u8> = f
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::ShuffleDeck { player } => Some(*player),
                _ => None,
            })
            .collect();
        assert!(
            shuffles.contains(&0),
            "the activating player's, announced by the card: {shuffles:?}"
        );
        assert!(shuffles.contains(&1), "and the opponent's: {shuffles:?}");
    }

    /// **Both decks are shuffled** on an ordinary board too.
    #[test]
    fn both_decks_are_shuffled() {
        // A survivor in each deck: `field::shuffle` returns early on an
        // empty pile, so a deck the sweep emptied announces nothing — and
        // that is the reference's behaviour, not a gap.
        let (mut f, nc, _, _, _) = field_as(0, &[(7_300, FLIP)], &[(7_300, FLIP), (7_301, PLAIN)]);
        let e = effect_of(&f, nc);
        resolve(&mut f, e, 0);
        let shuffles: Vec<u8> = f
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::ShuffleDeck { player } => Some(*player),
                _ => None,
            })
            .collect();
        assert!(
            shuffles.contains(&0),
            "the activating player's: {shuffles:?}"
        );
        assert!(shuffles.contains(&1), "and the opponent's: {shuffles:?}");
    }

    /// **A target that stopped being face-down is spared.** The re-check
    /// at resolution is the script's, and it is a different question from
    /// the relation check beside it.
    #[test]
    fn a_target_turned_face_up_is_spared() {
        let (mut f, nc, theirs, _, _) = field_as(0, &[(7_400, PLAIN)], &[]);
        let e = effect_of(&f, nc);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        ch.target_cards = vec![theirs[0]];
        f.core.current_chain.push(ch);
        f.cards[theirs[0]].create_chain_relation(e, 11);
        // Flipped up while the chain sat there.
        f.cards[theirs[0]].current.position = position::FACEUP_ATTACK;
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
            f.cards[theirs[0]].current.location,
            location::MZONE,
            "no longer face-down, so nothing happens"
        );
    }
}
