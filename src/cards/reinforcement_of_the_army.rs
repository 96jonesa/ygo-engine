//! Reinforcement of the Army — `c32807846.lua`.
//!
//! The twentieth card, and the first to **search a deck**. Everything
//! before it worked on cards both players could already see; this one
//! reaches into hidden information, picks one card out of it, and shows
//! the result to the opponent.
//!
//! ## `SelectMatchingCard`, not `SelectTarget`
//!
//! ```lua
//! local g=Duel.SelectMatchingCard(tp,s.filter,tp,LOCATION_DECK,0,1,1,nil)
//! ```
//!
//! A card in a deck **cannot be targeted** — the targeting scan asks
//! `is_capable_be_effect_target`, which a card nobody can see does not
//! pass. So a search uses the non-targeting scan and a plain selection,
//! and the effect carries no `EFFECT_FLAG_CARD_TARGET` at all. Mystical
//! Space Typhoon's shape would find nothing here.
//!
//! ## The operation info names a place, not a card
//!
//! ```lua
//! Duel.SetOperationInfo(0,CATEGORY_TOHAND,nil,1,tp,LOCATION_DECK)
//! ```
//!
//! `nil` cards, count 1, and the **location** in the parameter slot.
//! That is the only honest thing it can say: at the point the chain link
//! is built nobody has looked in the deck yet, so the announcement is
//! "one card, from that player's deck" rather than a named card. Every
//! other card in the pool so far has been able to name its cards.
//!
//! ## The filter is three clauses and one of them has a trap
//!
//! `IsLevelBelow(4)` is **not** `level <= 4`: the reference guards
//! `plvl > 0`, so a level-zero card never passes. In a deck full of
//! Spells and Traps that guard is doing real work, and dropping it would
//! make every Spell a legal search.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::{race, reason};
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 32_807_846;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::TOHAND | category::SEARCH);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

const DECK: u32 = location::DECK as u32;

fn filter(f: &mut Field, c: CardId) -> bool {
    api::is_level_below(f, c, 4)
        && api::is_race(f, c, race::WARRIOR)
        && api::is_able_to_hand(f, c, None)
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        return api::yes(api::is_existing_matching_card(
            f,
            Some(&filter),
            tp,
            DECK,
            0,
            1,
            api::Except::None,
        ));
    }
    // No cards named: the deck has not been looked in yet.
    api::set_operation_info(f, 0, category::TOHAND, None, 1, tp, DECK as i32);
    api::yes(true)
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    api::hint(f, hint::SELECTMSG, tp, hintmsg::ATOHAND);
    api::select_matching_card(f, tp, Some(&filter), tp, DECK, 0, 1, 1, api::Except::None);
    api::suspend(move |f, _ctx| {
        let g = api::group_selected(f);
        if g.is_empty() {
            return api::done();
        }
        api::send_to_hand(f, g.clone(), None, reason::EFFECT);
        api::suspend(move |f, ctx| {
            // Only once it has arrived, as Magician of Faith's does.
            api::confirm_cards(f, 1 - ctx.player, g.clone());
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
    use crate::effect::flag;
    use crate::event::Event;
    use crate::field::Message;
    use crate::position;
    use crate::processor::{Kind, Status};

    /// A monster for a deck: `(level, race)` is what the filter reads.
    fn deck_monster(f: &mut Field, owner: u8, code_: u32, level: u32, race_: u64) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER | card_type::NORMAL,
                level,
                attack: 1000,
                defense: 1000,
                race: race_,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, location::DECK, 0, false);
        id
    }

    fn put_spell(f: &mut Field, owner: u8, code_: u32, loc: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::SPELL,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, 0, false);
        f.cards[id].current.position = position::FACEUP;
        let fid = f.next_field_id_raw();
        f.cards[id].fieldid = fid;
        f.cards[id].fieldid_r = fid;
        id
    }

    /// `tp`'s Main Phase 1 with the card face-up in its own row — where it
    /// sits once activated — and a deck of `mine` for `tp` plus `theirs`
    /// for the opponent, each `(level, race)`.
    fn field_as(
        tp: u8,
        mine: &[(u32, u64)],
        theirs: &[(u32, u64)],
    ) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let ra = put_spell(&mut f, tp, CODE, location::SZONE);
        f.initialize_card(ra);
        let ours = mine
            .iter()
            .enumerate()
            .map(|(i, &(lv, r))| deck_monster(&mut f, tp, 7_000 + i as u32, lv, r))
            .collect();
        let others = theirs
            .iter()
            .enumerate()
            .map(|(i, &(lv, r))| deck_monster(&mut f, 1 - tp, 7_500 + i as u32, lv, r))
            .collect();
        (f, ra, ours, others)
    }

    fn field(
        mine: &[(u32, u64)],
        theirs: &[(u32, u64)],
    ) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        field_as(0, mine, theirs)
    }

    fn effect_of(f: &Field, ra: CardId) -> crate::event::EffectId {
        f.cards[ra].field_effect.equal_range(code::FREE_CHAIN)[0]
    }

    fn asks(f: &mut Field, e: crate::event::EffectId, tp: u8, chk: bool) -> bool {
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = Ctx {
            reason_effect: e,
            player: tp,
            event: &ev,
            card: None,
            args: &[],
        };
        f.core.reason_effect = Some(e);
        f.core.reason_player = tp;
        let answer = target(f, &ctx, chk, None).finished().unwrap_or(0) != 0;
        f.core.reason_effect = None;
        answer
    }

    struct Run {
        offered: Vec<Vec<CardId>>,
        asked: Vec<(u8, u8, u8)>,
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

    const W: u64 = race::WARRIOR;
    const S: u64 = race::SPELLCASTER;

    /// **One printed activate effect that does *not* target**, with both
    /// halves of its category.
    ///
    /// The absence of `CARD_TARGET` is the point: a card in a deck cannot
    /// be targeted, so a search must not claim to.
    #[test]
    fn the_script_registers_a_non_targeting_activate_effect() {
        let (f, ra, _, _) = field(&[], &[]);
        let ids = f.cards[ra].field_effect.equal_range(code::FREE_CHAIN);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert!(
            !e.is_flag(flag::CARD_TARGET),
            "a card in a deck cannot be targeted"
        );
        assert_eq!(
            e.category,
            category::TOHAND | category::SEARCH,
            "both halves"
        );
        assert_eq!(e.handler, Some(ra));
    }

    /// **The filter is three clauses**, each refused on its own, with the
    /// positive that shows the test is not vacuous.
    #[test]
    fn the_filter_wants_a_small_warrior_that_can_be_taken() {
        let (mut f, _, _, _) = field(&[], &[]);
        let ok = deck_monster(&mut f, 0, 8_001, 4, W);
        let big = deck_monster(&mut f, 0, 8_002, 5, W);
        let wrong_race = deck_monster(&mut f, 0, 8_003, 4, S);
        let stuck = deck_monster(&mut f, 0, 8_004, 4, W);
        f.cards[stuck].set_status(status::LEAVE_CONFIRMED, true);
        let spell = put_spell(&mut f, 0, 8_005, location::DECK);
        assert!(filter(&mut f, ok), "a level 4 Warrior");
        assert!(!filter(&mut f, big), "level 5 is not below 4");
        assert!(
            !filter(&mut f, wrong_race),
            "a Spellcaster is not a Warrior"
        );
        assert!(!filter(&mut f, stuck), "and it has to be able to come back");
        assert!(
            !filter(&mut f, spell),
            "a Spell has no level, and level zero is never below anything"
        );
    }

    /// **A level-1 Warrior passes and a level-4 one passes** — the bound
    /// is inclusive, which `IsLevelBelow` spells `<=`.
    #[test]
    fn the_level_bound_is_inclusive() {
        let (mut f, _, _, _) = field(&[], &[]);
        for lv in 1..=4 {
            let c = deck_monster(&mut f, 0, 8_100 + lv, lv, W);
            assert!(filter(&mut f, c), "level {lv} is below or equal to 4");
        }
        let five = deck_monster(&mut f, 0, 8_200, 5, W);
        assert!(!filter(&mut f, five));
    }

    /// **The scan is one-sided**: only its controller's deck. A deck full
    /// of the opponent's Warriors offers nothing.
    #[test]
    fn the_scan_reaches_only_its_own_deck() {
        let (mut f, ra, _, _) = field(&[], &[(4, W), (3, W)]);
        let e = effect_of(&f, ra);
        assert!(
            !asks(&mut f, e, 0, false),
            "all the Warriors are the opponent's"
        );
        let (mut f, ra, _, _) = field(&[(4, W)], &[]);
        let e = effect_of(&f, ra);
        assert!(asks(&mut f, e, 0, false), "one of mine is enough");
    }

    /// **It is refused when the deck holds nothing that matches**, which
    /// is what stops it being activated for no effect.
    #[test]
    fn it_is_refused_with_no_matching_monster() {
        let (mut f, ra, _, _) = field(&[(5, W), (4, S)], &[]);
        let e = effect_of(&f, ra);
        assert!(!asks(&mut f, e, 0, false), "too big, or the wrong race");
    }

    /// **The recorded operation names a place, not a card.** No cards,
    /// one of them, the controller, and the deck — because at the point
    /// the link is built nobody has looked in the deck yet.
    #[test]
    fn the_recorded_operation_names_the_deck_not_a_card() {
        for tp in [0u8, 1] {
            let (mut f, ra, _, _) = field_as(tp, &[(4, W), (3, W)], &[]);
            let e = effect_of(&f, ra);
            resolve_as(&mut f, tp, e, 0);
            let op = f.core.current_chain[0]
                .opinfos
                .get(&category::TOHAND)
                .cloned()
                .unwrap_or_else(|| panic!("a TOHAND operation for tp={tp}"));
            assert_eq!(op.cards, None, "no card can be named yet");
            assert_eq!(op.count, 1, "exactly one will be taken");
            assert_eq!(op.player, tp, "from this player's deck");
            assert_eq!(op.param, DECK as i32, "and the deck is where from");
        }
    }

    /// **The selection offers only what the filter allows, from its own
    /// deck, to the controller, one card exactly.**
    #[test]
    fn the_selection_offers_the_matching_warriors() {
        let (mut f, ra, mine, theirs) = field(&[(4, W), (5, W), (3, S), (2, W)], &[(1, W)]);
        let e = effect_of(&f, ra);
        let run = resolve(&mut f, e, 0);
        // Membership is this test's business; the *order* is the
        // `SelectCard` processor's, and the differential harness is what
        // checks it against ocgcore. Pinning it here would only restate
        // the port's own sort back at itself.
        let mut offered = run.offered[0].clone();
        offered.sort_unstable();
        let mut want = vec![mine[0], mine[3]];
        want.sort_unstable();
        assert_eq!(offered, want, "the level 4 and the level 2 Warriors");
        assert!(!run.offered[0].contains(&mine[1]), "not the level 5");
        assert!(!run.offered[0].contains(&mine[2]), "not the Spellcaster");
        assert!(!run.offered[0].contains(&theirs[0]), "not the opponent's");
        assert_eq!(run.asked[0], (0, 1, 1), "the controller takes one");
    }

    /// **The prompt is the add-to-hand message, to the controller.**
    #[test]
    fn the_prompt_is_the_add_to_hand_message() {
        let (mut f, ra, _, _) = field(&[(4, W)], &[]);
        let e = effect_of(&f, ra);
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

    /// **The chosen monster goes to its owner's hand and is revealed**,
    /// and the rest of the deck stays put.
    #[test]
    fn it_takes_the_chosen_warrior_and_shows_it() {
        let (mut f, ra, mine, _) = field(&[(4, W), (2, W)], &[]);
        let e = effect_of(&f, ra);
        let run = resolve(&mut f, e, 1);
        let taken = run.offered[0][1];
        let left = *mine
            .iter()
            .find(|&&c| c != taken && f.cards[c].data.race == W)
            .expect("the other Warrior");
        assert_eq!(f.cards[taken].current.location, location::HAND);
        assert_eq!(f.cards[taken].current.controller, 0, "its owner's hand");
        assert!(
            f.cards[taken].reason & reason::EFFECT != 0,
            "by an effect, not paid as a cost"
        );
        assert_eq!(f.cards[left].current.location, location::DECK, "only one");
        let code_of = f.cards[taken].data.code;
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::ConfirmCards { player: 1, codes } if codes == &vec![code_of]
            )),
            "shown to the opponent"
        );
    }

    /// **The reveal comes after the search**, which the message stream
    /// shows: the card's move into the hand precedes the reveal.
    ///
    /// The same rule as Magician of Faith's — `ConfirmCards` raises
    /// `EVENT_TOHAND_CONFIRM` only for a card already in a hand — and the
    /// same reason it needs a test: the reveal message and the final
    /// board are identical either way.
    #[test]
    fn the_reveal_comes_after_the_search() {
        let (mut f, ra, mine, _) = field(&[(4, W)], &[]);
        let code_of = f.cards[mine[0]].data.code;
        let e = effect_of(&f, ra);
        resolve(&mut f, e, 0);
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
        assert!(moved < revealed, "moved at {moved}, revealed at {revealed}");
    }

    /// **The reveal names the opponent for either player.** `1 - tp`
    /// reads the same as `tp` on a board where the activating player is 0.
    #[test]
    fn the_reveal_names_the_opponent_for_either_player() {
        let (mut f, ra, mine, _) = field_as(1, &[(4, W)], &[]);
        let e = effect_of(&f, ra);
        resolve_as(&mut f, 1, e, 0);
        let code_of = f.cards[mine[0]].data.code;
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::ConfirmCards { player: 0, codes } if codes == &vec![code_of]
            )),
            "player 1 searched, so player 0 is shown the card"
        );
    }

    /// **A searched card goes to the *searcher's* hand, even when
    /// someone else owns it** — which is the opposite of Magician of
    /// Faith, from the same `nil` argument.
    ///
    /// `field::send_to` has a special case for exactly this shape:
    ///
    /// ```cpp
    /// // send to hand from deck and playerid not given => send to the hand of controler
    /// if(p == PLAYER_NONE && (destination & LOCATION_HAND)
    ///    && (pcard->current.location & LOCATION_DECK)
    ///    && pcard->current.controler == reason_player)
    ///     p = reason_player;
    /// ```
    ///
    /// So `nil` means "the owner's hand" everywhere *except* a search out
    /// of a deck, where it means the searcher's. Magician of Faith takes
    /// from a graveyard and so gets the owner's hand; this card takes
    /// from a deck and gets its own. Reading `nil` as one rule would make
    /// one of the two cards wrong, and nothing on an ordinary board would
    /// show it.
    #[test]
    fn a_searched_card_goes_to_the_searchers_hand() {
        let (mut f, ra, _, _) = field(&[], &[]);
        let mut owned_by_one = Card::with_data(
            CardData {
                code: 8_800,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                race: W,
                ..Default::default()
            },
            1,
        );
        owned_by_one.current.controller = 0;
        owned_by_one.set_status(status::EFFECT_ENABLED, true);
        let w = f.new_card(owned_by_one);
        f.add_card(0, w, location::DECK, 0, false);
        let e = effect_of(&f, ra);
        let run = resolve(&mut f, e, 0);
        assert_eq!(run.offered[0], vec![w], "in my deck, so reachable");
        assert_eq!(f.cards[w].current.location, location::HAND);
        assert_eq!(
            f.cards[w].current.controller, 0,
            "the searcher's hand, by the deck-to-hand special case"
        );
        assert_eq!(f.cards[w].owner, 1, "even though player 1 owns it");
        assert!(f.players[1].hand.is_empty());
    }

    /// **A resolution that finds nothing does nothing**, and in
    /// particular reveals nothing.
    ///
    /// Unreachable through `target`, which refuses the activation when
    /// the deck holds no match — but reachable in a duel, because the
    /// deck can change between activation and resolution. The guard is
    /// the script's `if #g>0`, and without it the card would try to send
    /// an empty group and then reveal it.
    #[test]
    fn a_resolution_that_finds_nothing_reveals_nothing() {
        let (mut f, ra, mine, _) = field(&[(4, W)], &[]);
        let e = effect_of(&f, ra);
        // Gone between activation and resolution.
        f.cards[mine[0]].set_status(status::LEAVE_CONFIRMED, true);
        let run = resolve(&mut f, e, 0);
        assert!(
            run.offered.iter().all(Vec::is_empty),
            "nothing to offer: {:?}",
            run.offered
        );
        assert_eq!(
            f.cards[mine[0]].current.location,
            location::DECK,
            "it stayed put"
        );
        assert!(
            !f.messages
                .iter()
                .any(|m| matches!(m, Message::ConfirmCards { .. })),
            "and nothing was revealed"
        );
        assert!(
            !f.messages.iter().any(|m| matches!(m, Message::Move { .. })),
            "nor moved"
        );
    }
}
