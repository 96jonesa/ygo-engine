//! Tribe-Infecting Virus — `c33184167.lua`.
//!
//! The twenty-seventh card, and the pool's first **ignition** effect: a
//! monster that does something from the field because its controller
//! chose to, rather than in response to anything. It discards a card,
//! names a race, and destroys every face-up monster of that race on
//! either side.
//!
//! ## An ignition effect is a range, not an event
//!
//! `EFFECT_TYPE_IGNITION` with `SetRange(LOCATION_MZONE)` and no
//! `SetCode` at all. Where every card so far has said *when* it may be
//! used, this one says *where it must be* — the Main Phase menu offers it
//! because the card is in the monster row, and nothing raises an event.
//!
//! ## The offer is built from what is on the board
//!
//! ```lua
//! for tc in aux.Next(g) do race = (race | tc:GetRace()) end
//! local arc = Duel.AnnounceRace(tp,1,race)
//! ```
//!
//! The races available to name are exactly those **present face-up on
//! the field**, folded together with `|`. So the question is different
//! every time it is asked, and a board of nothing but Warriors offers
//! Warrior alone. `AnnounceRace` then caps the count at the number of
//! races actually on offer, so a single-race board cannot be asked for
//! two.
//!
//! ## The answer is carried on the effect, not in a variable
//!
//! `e:SetLabel(arc)` in the target, `e:GetLabel()` in the operation. The
//! two halves run at different times with a chain in between, so the
//! label is how the declared race survives the gap — the same mechanism
//! Sangan uses to remember a card's name.
//!
//! ## The scan is re-run at resolution
//!
//! The target records what *would* be destroyed; the operation scans
//! again with the same race. A monster that arrived or flipped in between
//! is destroyed too, and one that left is not — the recorded group is an
//! announcement, not a promise.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, CardId};
use crate::field::Field;
use crate::host_question::hint;
use crate::script_api as api;

pub const CODE: u32 = 33_184_167;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // destroy
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::DESTROY);
    api::set_type(f, e1, effect_type::IGNITION);
    api::set_range(f, e1, u16::from(location::MZONE));
    api::set_cost(f, e1, cost);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, operation);
    api::register_effect(f, c, e1, false);
}

const MZONE: u32 = location::MZONE as u32;
const HAND: u32 = location::HAND as u32;

fn discardable(f: &mut Field, c: CardId) -> bool {
    api::is_discardable(f, c, None, None)
}

fn faceup(f: &mut Field, c: CardId) -> bool {
    api::is_faceup(f, c)
}

fn cost(f: &mut Field, ctx: &Ctx, chk: bool) -> bool {
    let tp = ctx.player;
    if !chk {
        return api::is_existing_matching_card(
            f,
            Some(&discardable),
            tp,
            HAND,
            0,
            1,
            api::Except::None,
        );
    }
    api::discard_hand(
        f,
        tp,
        Some(&discardable),
        1,
        1,
        reason::COST | reason::DISCARD,
        api::Except::None,
    );
    true
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        return api::yes(api::is_existing_matching_card(
            f,
            Some(&faceup),
            tp,
            MZONE,
            MZONE,
            1,
            api::Except::None,
        ));
    }
    let g = api::get_matching_group(f, Some(&faceup), tp, MZONE, MZONE, api::Except::None);
    // Only the races actually on the board may be named.
    let mut races = 0u64;
    for &tc in &g {
        races |= api::get_race(f, tc);
    }
    api::hint(f, hint::SELECTMSG, tp, 0);
    api::announce_race(f, tp, 1, races);
    let e = ctx.reason_effect;
    api::suspend(move |f, _ctx| {
        let named = api::resumed_mask(f);
        // Carried on the effect: the operation runs after the chain.
        api::set_label(f, e, vec![named as i64]);
        let dg: Vec<CardId> = g
            .iter()
            .copied()
            .filter(|&c| api::is_race(f, c, named))
            .collect();
        let n = dg.len() as u8;
        api::set_operation_info(f, 0, category::DESTROY, Some(dg), n, 0, 0);
        api::yes(true)
    })
}

fn operation(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let named = f
        .effects
        .get(ctx.reason_effect)
        .map_or(0, |e| api::get_label(e) as u64);
    // Scanned again: the board may have changed since the announcement.
    let g: Vec<CardId> =
        api::get_matching_group(f, Some(&faceup), tp, MZONE, MZONE, api::Except::None)
            .into_iter()
            .filter(|&c| api::is_race(f, c, named))
            .collect();
    api::destroy(f, g, reason::EFFECT);
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, race, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::event::{code, Event};
    use crate::field::Message;
    use crate::position;
    use crate::processor::{Kind, Status};

    fn monster(f: &mut Field, owner: u8, seat: u32, race_: u64, faceup: bool) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 7_000 + seat + u32::from(owner) * 100,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
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
        f.add_card(owner, id, location::MZONE, seat, false);
        f.cards[id].current.position = if faceup {
            position::FACEUP_ATTACK
        } else {
            position::FACEDOWN_DEFENSE
        };
        id
    }

    fn in_hand(f: &mut Field, owner: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 8_000 + seq,
                type_: card_type::MONSTER | card_type::NORMAL,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, location::HAND, seq, false);
        id
    }

    /// `tp`'s Main Phase 1 with the virus face-up in its own row, plus
    /// `others` as `(owner, race, face-up)` and `hand` cards to discard.
    fn field_as(
        tp: u8,
        others: &[(u8, u64, bool)],
        hand: u32,
    ) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut v = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        v.current.controller = tp;
        v.set_status(status::EFFECT_ENABLED, true);
        let tiv = f.new_card(v);
        f.add_card(tp, tiv, location::MZONE, 0, false);
        f.cards[tiv].current.position = position::FACEUP_ATTACK;
        f.initialize_card(tiv);
        let board = others
            .iter()
            .enumerate()
            .map(|(i, &(owner, r, up))| monster(&mut f, owner, i as u32 + 1, r, up))
            .collect();
        let held = (0..hand).map(|i| in_hand(&mut f, tp, i)).collect();
        (f, tiv, board, held)
    }

    fn field(others: &[(u8, u64, bool)], hand: u32) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        field_as(0, others, hand)
    }

    fn effect_of(f: &Field, tiv: CardId) -> crate::event::EffectId {
        f.cards[tiv].field_effect.equal_range(0)[0]
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

    struct Run {
        announces: Vec<(u8, u8, u64)>,
        offered: Vec<Vec<CardId>>,
    }

    /// Drive cost, target and operation, naming `pick` when asked for a
    /// race and taking the first card when asked to discard.
    fn resolve_as(f: &mut Field, tp: u8, e: crate::event::EffectId, pick: u64) -> Run {
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.core.sub_solving_event.push_back(Event::new(0));
        f.emplace(Kind::ExecuteTarget {
            resume: None,
            effect: e,
            player: tp,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        let mut run = Run {
            announces: Vec::new(),
            offered: Vec::new(),
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
                        f.core.sub_solving_event.push_back(Event::new(0));
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
                    Some(Message::AnnounceRace {
                        player,
                        count,
                        available,
                    }) => {
                        run.announces.push((*player, *count, *available));
                        f.core.returns.set_u64(0, pick);
                    }
                    Some(Message::SelectCard { min, cards, .. }) => {
                        let min = usize::from(*min);
                        run.offered.push(cards.clone());
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, i as i32);
                        }
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        run
    }

    fn resolve(f: &mut Field, e: crate::event::EffectId, pick: u64) -> Run {
        resolve_as(f, 0, e, pick)
    }

    /// **One printed ignition effect, ranged to the monster row.**
    ///
    /// An ignition effect says *where it must be*, not when it fires —
    /// there is no `SetCode` at all.
    #[test]
    fn the_script_registers_a_ranged_ignition_effect() {
        let (f, tiv, _, _) = field(&[], 0);
        let ids = f.cards[tiv].field_effect.equal_range(0);
        assert_eq!(ids.len(), 1, "one effect, registered under no event");
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::IGNITION));
        assert!(!e.is_type(effect_type::ACTIVATE), "not an activation");
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert_eq!(e.range, u16::from(location::MZONE), "on the field");
        assert_eq!(e.category, category::DESTROY);
        assert!(e.cost.is_some());
        assert_eq!(e.description, api::stringid(CODE, 0));
    }

    /// **The cost wants a discardable card in hand**, and discards
    /// exactly one as a cost.
    #[test]
    fn the_cost_discards_one_from_hand() {
        let ev = Event::new(0);
        let (mut f, tiv, _, _) = field(&[], 0);
        let e = effect_of(&f, tiv);
        let ctx = ctx_for(e, &ev, 0);
        assert!(!cost(&mut f, &ctx, false), "an empty hand cannot pay");

        let (mut f, tiv, _, held) = field(&[], 2);
        let e = effect_of(&f, tiv);
        let ctx = ctx_for(e, &ev, 0);
        assert!(cost(&mut f, &ctx, false), "two in hand");
        assert!(cost(&mut f, &ctx, true));
        match &f.core.subunits[0].kind {
            Kind::DiscardHand {
                playerid,
                min,
                max,
                reason,
                ..
            } => {
                assert_eq!(*playerid, 0);
                assert_eq!((*min, *max), (1, 1), "exactly one");
                assert!(reason & crate::card::reason::COST != 0, "as a cost");
                assert!(reason & crate::card::reason::DISCARD != 0);
            }
            other => panic!("expected a DiscardHand: {other:?}"),
        }
        assert_eq!(held.len(), 2);
    }

    /// **The cost may only discard what is discardable.** A hand of
    /// cards that cannot be used as a cost cannot pay, and the offer the
    /// discard makes carries the same filter.
    #[test]
    fn the_cost_offers_only_discardable_cards() {
        let ev = Event::new(0);
        let (mut f, tiv, _, held) = field(&[], 2);
        // Both cards refuse to be used as a cost.
        for &c in &held {
            let mut ban = crate::effect::Effect::new(effect_type::SINGLE, code::CANNOT_USE_AS_COST);
            ban.owner = Some(c);
            ban.handler = Some(c);
            let ban = f.new_effect(ban);
            f.cards[c]
                .single_effect
                .insert(code::CANNOT_USE_AS_COST, ban);
        }
        let e = effect_of(&f, tiv);
        let ctx = ctx_for(e, &ev, 0);
        assert!(
            !cost(&mut f, &ctx, false),
            "a hand that cannot be used as a cost cannot pay"
        );

        // With one ordinary card among them, only that one is offered.
        let ok = in_hand(&mut f, 0, 9);
        assert!(cost(&mut f, &ctx, true));
        assert_eq!(
            f.core.select_cards,
            vec![ok],
            "only the card that may be a cost is offered"
        );
    }

    /// **The target scan reaches both monster rows.** A board where the
    /// only other face-up monster is the opponent's is still a legal
    /// activation, and that monster's race is on offer.
    #[test]
    fn the_target_scan_reaches_both_rows() {
        let (mut f, tiv, _, _) = field(&[(1, race::DRAGON, true)], 1);
        let e = effect_of(&f, tiv);
        let run = resolve(&mut f, e, race::DRAGON);
        assert_eq!(
            run.announces[0].2 & race::DRAGON,
            race::DRAGON,
            "the opponent's Dragon is in the offer"
        );
    }

    /// **Only races on the board are offered**, folded together, and
    /// only one may be named.
    #[test]
    fn the_offer_is_the_races_on_the_board() {
        let (mut f, tiv, _, _) = field(
            &[
                (0, race::WARRIOR, true),
                (1, race::SPELLCASTER, true),
                (1, race::DRAGON, false),
            ],
            1,
        );
        let e = effect_of(&f, tiv);
        let run = resolve(&mut f, e, race::WARRIOR);
        assert_eq!(run.announces.len(), 1, "asked once");
        let (player, count, available) = run.announces[0];
        assert_eq!(player, 0, "the controller names it");
        assert_eq!(count, 1, "one race");
        // The virus itself is Aqua and face-up, so it is in the fold too.
        assert_eq!(
            available,
            race::WARRIOR | race::SPELLCASTER | race::AQUA,
            "face-up races only — the face-down Dragon is not offered"
        );
        assert_eq!(available & race::DRAGON, 0);
    }

    /// **It destroys every face-up monster of the named race, on both
    /// sides**, and nothing else.
    #[test]
    fn it_destroys_that_race_on_both_sides() {
        let (mut f, tiv, board, _) = field(
            &[
                (0, race::WARRIOR, true),
                (1, race::WARRIOR, true),
                (1, race::SPELLCASTER, true),
                (1, race::WARRIOR, false),
            ],
            1,
        );
        let e = effect_of(&f, tiv);
        resolve(&mut f, e, race::WARRIOR);
        assert_eq!(
            f.cards[board[0]].current.location,
            location::GRAVE,
            "my own Warrior goes too"
        );
        assert_eq!(f.cards[board[1]].current.location, location::GRAVE);
        assert_eq!(
            f.cards[board[2]].current.location,
            location::MZONE,
            "a Spellcaster is not a Warrior"
        );
        assert_eq!(
            f.cards[board[3]].current.location,
            location::MZONE,
            "and a face-down Warrior is not face-up"
        );
        assert!(f.cards[board[0]].reason & reason::EFFECT != 0);
        assert_eq!(
            f.cards[tiv].current.location,
            location::MZONE,
            "the virus is Aqua, so it survives naming Warrior"
        );
    }

    /// **Naming its own race destroys the virus too.**
    #[test]
    fn naming_its_own_race_takes_it_with_them() {
        let (mut f, tiv, board, _) = field(&[(1, race::AQUA, true)], 1);
        let e = effect_of(&f, tiv);
        resolve(&mut f, e, race::AQUA);
        assert_eq!(f.cards[tiv].current.location, location::GRAVE);
        assert_eq!(f.cards[board[0]].current.location, location::GRAVE);
    }

    /// **The named race is carried on the effect's label**, which is how
    /// it survives the gap between the two halves.
    #[test]
    fn the_named_race_is_carried_on_the_label() {
        let (mut f, tiv, _, _) = field(&[(1, race::SPELLCASTER, true)], 1);
        let e = effect_of(&f, tiv);
        resolve(&mut f, e, race::SPELLCASTER);
        let label = api::get_label(f.effects.get(e).unwrap());
        assert_eq!(
            label as u64,
            race::SPELLCASTER,
            "the answer is on the effect, not in a local"
        );
    }

    /// **The recorded operation names what would be destroyed**, by the
    /// race that was announced.
    #[test]
    fn the_recorded_operation_names_that_race() {
        let (mut f, tiv, board, _) = field(
            &[
                (0, race::WARRIOR, true),
                (1, race::WARRIOR, true),
                (1, race::DRAGON, true),
            ],
            1,
        );
        let e = effect_of(&f, tiv);
        resolve(&mut f, e, race::WARRIOR);
        let op = f.core.current_chain[0]
            .opinfos
            .get(&category::DESTROY)
            .cloned()
            .expect("a DESTROY operation");
        let mut want = vec![board[0], board[1]];
        want.sort_unstable();
        let mut got = op.cards.clone().unwrap_or_default();
        got.sort_unstable();
        assert_eq!(got, want, "the two Warriors, not the Dragon");
        assert_eq!(op.count, 2);
    }

    /// **The board is scanned again at resolution.** A monster of the
    /// named race that arrives after the announcement is destroyed too —
    /// the recorded group is an announcement, not a promise.
    #[test]
    fn a_monster_that_arrived_afterwards_is_destroyed_too() {
        let (mut f, tiv, board, _) = field(&[(1, race::WARRIOR, true)], 1);
        let e = effect_of(&f, tiv);
        // Run the target half only, then add a Warrior, then resolve.
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.sub_solving_event.push_back(Event::new(0));
        f.emplace(Kind::ExecuteTarget {
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
                Status::Awaiting => match f.messages.last() {
                    Some(Message::AnnounceRace { .. }) => {
                        f.core.returns.set_u64(0, race::WARRIOR);
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        let latecomer = monster(&mut f, 1, 4, race::WARRIOR, true);
        f.core.sub_solving_event.push_back(Event::new(0));
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
        assert_eq!(f.cards[board[0]].current.location, location::GRAVE);
        assert_eq!(
            f.cards[latecomer].current.location,
            location::GRAVE,
            "it arrived after the announcement and is destroyed anyway"
        );
    }
}
