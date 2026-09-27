//! Sangan — `c26202165.lua`.
//!
//! The twenty-first card. A **mandatory trigger** that searches when the
//! monster reaches the graveyard *from the field*, and then — the part
//! that makes it more than a second Reinforcement of the Army — builds a
//! field effect at resolution that forbids activating the card it just
//! fetched, for the rest of the turn.
//!
//! ## Three firsts in one card
//!
//! **A trigger that reads where the card came from.** `EVENT_TO_GRAVE`
//! fires wherever the card was sent from — a discard from the hand, a
//! deck being milled — and `IsPreviousLocation(LOCATION_ONFIELD)` is what
//! narrows it to the one the card is about. Without it Sangan would
//! search on being discarded.
//!
//! **A count limit.** `SetCountLimit(1, id)` — once per turn, counted
//! against the card's *code*, so two copies of Sangan share the limit.
//!
//! **A value function that reads its own effect's label.** The lock it
//! builds is `EFFECT_CANNOT_ACTIVATE` whose value is the function
//!
//! ```lua
//! function s.aclimit(e,re,tp)
//!     return re:GetHandler():IsCode(e:GetLabel())
//! end
//! ```
//!
//! — `e` is the lock itself, carrying the fetched card's code in its
//! label; `re` is whatever is being activated. The port's value functions
//! could not see their own effect until this card; the signature now
//! carries it, which is what the reference has always passed.
//!
//! ## The lock is built only if the card actually arrived
//!
//! `if tc:IsLocation(LOCATION_HAND)` — a search whose card was
//! intercepted on the way to the hand locks nothing. That is a third
//! result read back, after the selection and the send.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, flag, Ctx, Effect, Yield};
use crate::event::{category, code, CardId};
use crate::field::{reset, Field};
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 26_202_165;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Search
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::TOHAND | category::SEARCH);
    api::set_type(f, e1, effect_type::SINGLE | effect_type::TRIGGER_F);
    api::set_code(f, e1, code::TO_GRAVE);
    api::set_count_limit(f, e1, 1, CODE, 0);
    api::set_condition(f, e1, condition);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, operation);
    api::register_effect(f, c, e1, false);
}

const DECK: u32 = location::DECK as u32;

fn condition(f: &mut Field, ctx: &Ctx) -> bool {
    let Some(handler) = api::get_handler(f, ctx.reason_effect) else {
        return false;
    };
    api::is_previous_location(f, handler, u16::from(location::ONFIELD))
}

fn filter(f: &mut Field, c: CardId) -> bool {
    api::is_attack_below(f, c, 1500) && api::is_monster(f, c) && api::is_able_to_hand(f, c, None)
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    if !chk {
        return api::yes(true);
    }
    api::set_operation_info(f, 0, category::TOHAND, None, 1, ctx.player, DECK as i32);
    api::yes(true)
}

/// `s.aclimit` — the lock's own test: is the thing being activated the
/// card this lock remembers?
fn aclimit(e: &Effect, f: &Field, ctx: &Ctx) -> i64 {
    let Some(handler) = api::get_handler(f, ctx.reason_effect) else {
        return 0;
    };
    let wanted = api::get_label(e) as u32;
    i64::from(api::is_code_readonly(f, handler, wanted))
}

fn operation(f: &mut Field, ctx: &Ctx) -> Yield {
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
            let tp = ctx.player;
            api::confirm_cards(f, 1 - tp, g.clone());
            // `g:GetFirst()`, and only if it really got there.
            let Some(&tc) = g.first() else {
                return api::done();
            };
            if !api::is_location(f, tc, u16::from(location::HAND)) {
                return api::done();
            }
            let Some(handler) = api::get_handler(f, ctx.reason_effect) else {
                return api::done();
            };
            let code_of = api::get_code(f, tc);
            let e1 = api::create_effect(f, handler);
            api::set_type(f, e1, effect_type::FIELD);
            api::set_property(f, e1, flag::PLAYER_TARGET, 0);
            api::set_code(f, e1, code::CANNOT_ACTIVATE);
            api::set_target_range(f, e1, 1, 0);
            api::set_value_fn(f, e1, aclimit);
            api::set_label(f, e1, vec![i64::from(code_of)]);
            api::set_reset(f, e1, reset::PHASE | u32::from(crate::duel::phases::END), 0);
            api::duel_register_effect(f, e1, tp);
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
    use crate::position;
    use crate::processor::{Kind, Status};

    fn deck_monster(f: &mut Field, owner: u8, code_: u32, attack: i32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
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
        f.add_card(owner, id, location::DECK, 0, false);
        id
    }

    /// Sangan in `tp`'s graveyard, having come **from the field**, with a
    /// deck of `mine` attacks for `tp` and `theirs` for the opponent.
    fn field_as(tp: u8, mine: &[i32], theirs: &[i32]) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut s = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        s.current.controller = tp;
        s.set_status(status::EFFECT_ENABLED, true);
        let san = f.new_card(s);
        f.add_card(tp, san, location::GRAVE, 0, false);
        f.cards[san].current.position = position::FACEUP;
        f.cards[san].previous.location = location::MZONE;
        f.cards[san].previous.controller = tp;
        f.initialize_card(san);
        let ours = mine
            .iter()
            .enumerate()
            .map(|(i, &atk)| deck_monster(&mut f, tp, 9_000 + i as u32, atk))
            .collect();
        let others = theirs
            .iter()
            .enumerate()
            .map(|(i, &atk)| deck_monster(&mut f, 1 - tp, 9_500 + i as u32, atk))
            .collect();
        (f, san, ours, others)
    }

    fn field(mine: &[i32], theirs: &[i32]) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        field_as(0, mine, theirs)
    }

    fn effect_of(f: &Field, san: CardId) -> crate::event::EffectId {
        f.cards[san].single_effect.equal_range(code::TO_GRAVE)[0]
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
        offered: Vec<Vec<CardId>>,
        asked: Vec<(u8, u8, u8)>,
    }

    fn resolve_as(f: &mut Field, tp: u8, e: crate::event::EffectId, choice: usize) -> Run {
        let mut ch = Chain::new(e, Event::new(code::TO_GRAVE));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.core
            .sub_solving_event
            .push_back(Event::new(code::TO_GRAVE));
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
                            .push_back(Event::new(code::TO_GRAVE));
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

    /// The lock this card leaves behind, if it left one.
    fn lock_of(f: &Field) -> Option<crate::event::EffectId> {
        f.field_effects
            .aura
            .equal_range(code::CANNOT_ACTIVATE)
            .first()
            .copied()
            .or_else(|| {
                f.field_effects
                    .continuous
                    .equal_range(code::CANNOT_ACTIVATE)
                    .first()
                    .copied()
            })
    }

    /// **One printed mandatory trigger on reaching the graveyard**, once
    /// per turn per *name*, with both halves of its category.
    #[test]
    fn the_script_registers_a_mandatory_trigger() {
        let (f, san, _, _) = field(&[], &[]);
        let ids = f.cards[san].single_effect.equal_range(code::TO_GRAVE);
        assert_eq!(ids.len(), 1, "one effect on EVENT_TO_GRAVE");
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::SINGLE));
        assert!(
            e.is_type(effect_type::TRIGGER_F),
            "mandatory: the controller may not decline it"
        );
        assert!(!e.is_type(effect_type::TRIGGER_O), "and not optional");
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert!(e.is_flag(flag::COUNT_LIMIT), "once per turn");
        assert_eq!((e.count_limit, e.count_limit_max), (1, 1));
        assert_eq!(e.count_code, CODE, "counted per name, so copies share it");
        assert_eq!(e.category, category::TOHAND | category::SEARCH);
        assert_eq!(e.description, api::stringid(CODE, 0));
        assert!(e.condition.is_some());
    }

    /// **The condition is where it came from, not where it is.** Sangan
    /// is in the graveyard by the time it asks; what decides is whether
    /// it got there from the field.
    #[test]
    fn the_condition_wants_it_to_have_left_the_field() {
        let (mut f, san, _, _) = field(&[], &[]);
        let e = effect_of(&f, san);
        let ev = Event::new(code::TO_GRAVE);
        let ctx = ctx_for(e, &ev, 0);
        assert!(condition(&mut f, &ctx), "it was on the field a moment ago");
        // Discarded from the hand instead.
        f.cards[san].previous.location = location::HAND;
        assert!(!condition(&mut f, &ctx), "a discard is not a death");
        // Milled from the deck.
        f.cards[san].previous.location = location::DECK;
        assert!(!condition(&mut f, &ctx));
        // And the Spell/Trap row counts as the field, as ONFIELD is both.
        f.cards[san].previous.location = location::SZONE;
        assert!(
            condition(&mut f, &ctx),
            "ONFIELD is the monster row and the row"
        );
        // An effect with no handler has no previous location to read, and
        // answers no rather than firing on nothing. Unreachable from the
        // processor, which only asks a registered effect — but it is the
        // arm the `let ... else` exists for, and a `true` there would fire
        // the search on every card that reached a graveyard.
        let mut orphan = Effect::new(effect_type::SINGLE | effect_type::TRIGGER_F, code::TO_GRAVE);
        orphan.owner = None;
        orphan.handler = None;
        let orphan = f.new_effect(orphan);
        assert!(
            api::get_handler(&f, orphan).is_none(),
            "the test is about the no-handler arm, so there must be none"
        );
        let ctx = ctx_for(orphan, &ev, 0);
        assert!(!condition(&mut f, &ctx), "no handler, no trigger");
    }

    /// **The filter is three clauses**, each refused on its own.
    #[test]
    fn the_filter_wants_a_small_monster_that_can_be_taken() {
        let (mut f, _, _, _) = field(&[], &[]);
        let ok = deck_monster(&mut f, 0, 9_900, 1500);
        let big = deck_monster(&mut f, 0, 9_901, 1600);
        let stuck = deck_monster(&mut f, 0, 9_902, 1000);
        f.cards[stuck].set_status(status::LEAVE_CONFIRMED, true);
        let mut spell = Card::with_data(
            CardData {
                code: 9_903,
                type_: card_type::SPELL,
                ..Default::default()
            },
            0,
        );
        spell.current.controller = 0;
        let sid = f.new_card(spell);
        f.add_card(0, sid, location::DECK, 0, false);
        assert!(filter(&mut f, ok), "exactly 1500 is below-or-equal");
        assert!(!filter(&mut f, big), "1600 is not");
        assert!(!filter(&mut f, stuck), "and it has to be able to come back");
        assert!(!filter(&mut f, sid), "a Spell is not a monster");
        // `IsAttackBelow` has a monster guard of its own, so a Spell is
        // refused twice over and cannot tell the two clauses apart. What
        // separates them is a card that is a monster **on its printed
        // line** and not in effect: `IsAttackBelow` accepts it on the
        // printed type, `IsMonster` reads the effective one and refuses.
        let printed_only = deck_monster(&mut f, 0, 9_904, 1000);
        f.cards[printed_only]
            .assume
            .insert(crate::card::assume::TYPE, u64::from(card_type::SPELL));
        assert!(
            api::is_attack_below(&mut f, printed_only, 1500),
            "printed as a monster, so the attack bound accepts it"
        );
        assert!(
            !api::is_monster(&mut f, printed_only),
            "but it is not one in effect"
        );
        assert!(!filter(&mut f, printed_only), "so the filter refuses it");
    }

    /// **The recorded operation names the searcher's deck**, with no
    /// card — nobody has looked in it when the link is built.
    #[test]
    fn the_recorded_operation_names_the_searchers_deck() {
        for tp in [0u8, 1] {
            let (mut f, san, _, _) = field_as(tp, &[1000, 1200], &[]);
            let e = effect_of(&f, san);
            resolve_as(&mut f, tp, e, 0);
            let op = f.core.current_chain[0]
                .opinfos
                .get(&category::TOHAND)
                .cloned()
                .unwrap_or_else(|| panic!("a TOHAND operation for tp={tp}"));
            assert_eq!(op.cards, None, "no card can be named yet");
            assert_eq!(op.count, 1);
            assert_eq!(op.player, tp, "the searcher's");
            assert_eq!(op.param, DECK as i32, "and the deck is where from");
        }
    }

    /// **The prompt is the add-to-hand message, to the searcher.**
    #[test]
    fn the_prompt_is_the_add_to_hand_message() {
        let (mut f, san, _, _) = field(&[1000], &[]);
        let e = effect_of(&f, san);
        resolve(&mut f, e, 0);
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::Hint { kind, player: 0, value }
                    if *kind == hint::SELECTMSG && *value == hintmsg::ATOHAND
            )),
            "HINTMSG_ATOHAND to the searcher"
        );
    }

    /// **The selection offers only my own deck's small monsters**, asked
    /// of the controller, one card.
    #[test]
    fn the_selection_offers_my_small_monsters() {
        let (mut f, san, mine, theirs) = field(&[1000, 1600, 1500], &[1000]);
        let e = effect_of(&f, san);
        let run = resolve(&mut f, e, 0);
        let mut offered = run.offered[0].clone();
        offered.sort_unstable();
        let mut want = vec![mine[0], mine[2]];
        want.sort_unstable();
        assert_eq!(offered, want, "the 1000 and the 1500");
        assert!(!run.offered[0].contains(&mine[1]), "not the 1600");
        assert!(!run.offered[0].contains(&theirs[0]), "not the opponent's");
        assert_eq!(run.asked[0], (0, 1, 1));
    }

    /// **It searches, reveals, and locks the fetched card's name** — the
    /// whole body, end to end through three suspensions.
    #[test]
    fn it_searches_reveals_and_locks_the_name() {
        let (mut f, san, _, _) = field(&[1000, 1200], &[]);
        let e = effect_of(&f, san);
        let run = resolve(&mut f, e, 0);
        let taken = run.offered[0][0];
        let code_of = f.cards[taken].data.code;
        assert_eq!(f.cards[taken].current.location, location::HAND);
        assert!(f.cards[taken].reason & reason::EFFECT != 0);
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::ConfirmCards { player: 1, codes } if codes == &vec![code_of]
            )),
            "shown to the opponent"
        );
        let lock = lock_of(&f).expect("a CANNOT_ACTIVATE lock was registered");
        let le = f.effects.get(lock).unwrap();
        assert!(le.is_type(effect_type::FIELD));
        assert!(le.is_flag(flag::PLAYER_TARGET), "it names a player");
        assert_eq!((le.s_range, le.o_range), (1, 0), "its controller only");
        assert_eq!(
            api::get_label(le),
            i64::from(code_of),
            "and it remembers the fetched card"
        );
        assert!(
            le.reset_flag & reset::PHASE != 0,
            "it expires with the phase"
        );
    }

    /// **The lock refuses that name and nothing else.** The value
    /// function is what does the refusing, and it reads the label it was
    /// given against the card being activated.
    #[test]
    fn the_lock_refuses_only_the_name_it_remembers() {
        let (mut f, san, _, _) = field(&[1000], &[]);
        let e = effect_of(&f, san);
        let run = resolve(&mut f, e, 0);
        let taken = run.offered[0][0];
        let code_of = f.cards[taken].data.code;
        let lock = lock_of(&f).expect("a lock");

        // Something that *is* the fetched card.
        let mut same = Card::with_data(
            CardData {
                code: code_of,
                type_: card_type::MONSTER | card_type::NORMAL,
                ..Default::default()
            },
            0,
        );
        same.current.controller = 0;
        let same = f.new_card(same);
        let mut other = Card::with_data(
            CardData {
                code: code_of + 1,
                type_: card_type::MONSTER | card_type::NORMAL,
                ..Default::default()
            },
            0,
        );
        other.current.controller = 0;
        let other = f.new_card(other);

        let ask = |f: &mut Field, card: CardId| {
            let mut probe = crate::effect::Effect::new(effect_type::ACTIVATE, code::FREE_CHAIN);
            probe.owner = Some(card);
            probe.handler = Some(card);
            let probe = f.new_effect(probe);
            let ev = Event::new(0);
            let ctx = ctx_for(probe, &ev, 0);
            let le = f.effects.get(lock).unwrap().clone();
            le.get_value(f, &ctx)
        };
        assert_ne!(ask(&mut f, same), 0, "the remembered name is refused");
        assert_eq!(ask(&mut f, other), 0, "and nothing else is");
    }

    /// **A card that never reached the hand leaves no lock.** The
    /// script's `if tc:IsLocation(LOCATION_HAND)` — a third result read
    /// back, after the selection and the send.
    #[test]
    fn a_search_that_never_arrived_locks_nothing() {
        let (mut f, san, mine, _) = field(&[1000], &[]);
        let e = effect_of(&f, san);
        // Cannot leave the deck, so the selection finds nothing at all.
        f.cards[mine[0]].set_status(status::LEAVE_CONFIRMED, true);
        let run = resolve(&mut f, e, 0);
        assert!(run.offered.iter().all(Vec::is_empty), "nothing to offer");
        assert_eq!(f.cards[mine[0]].current.location, location::DECK);
        assert!(lock_of(&f).is_none(), "and no lock");
        assert!(
            !f.messages
                .iter()
                .any(|m| matches!(m, Message::ConfirmCards { .. })),
            "nothing revealed"
        );
    }

    /// **A card that is chosen but never arrives leaves no lock.**
    ///
    /// This is the case the script's `if tc:IsLocation(LOCATION_HAND)`
    /// exists for, and it is not the same as finding nothing: the card
    /// passes the filter, is offered, is chosen, and the *send* is what
    /// refuses it. An immunity to the searching effect does that —
    /// `cannot_make_the_trip` asks `is_affect_by_effect` — so the card
    /// stays in the deck and there is no name to lock.
    #[test]
    fn a_card_that_never_arrived_leaves_no_lock() {
        fn immune_to_everything(_: &Effect, _: &Field, _: &Ctx) -> i64 {
            1
        }
        let (mut f, san, mine, _) = field(&[1000], &[]);
        let mut immunity = Effect::new(effect_type::SINGLE, code::IMMUNE_EFFECT);
        immunity.owner = Some(mine[0]);
        immunity.handler = Some(mine[0]);
        immunity.flag[0] |= flag::FUNC_VALUE;
        immunity.value_fn = Some(immune_to_everything);
        let immunity = f.new_effect(immunity);
        f.cards[mine[0]].immune_effect.push(immunity);

        let e = effect_of(&f, san);
        let run = resolve(&mut f, e, 0);
        assert_eq!(
            run.offered[0],
            vec![mine[0]],
            "it passes the filter and is offered"
        );
        assert_eq!(
            f.cards[mine[0]].current.location,
            location::DECK,
            "but the send is refused"
        );
        assert!(
            lock_of(&f).is_none(),
            "so there is no name to lock: the guard is what stops it"
        );
    }

    /// **The lock is registered for the searcher**, whichever player
    /// that is — `1` and `0` in the target range read the same way round
    /// only when you know whose effect it is.
    #[test]
    fn the_lock_belongs_to_the_searcher() {
        for tp in [0u8, 1] {
            let (mut f, san, _, _) = field_as(tp, &[1000], &[]);
            let e = effect_of(&f, san);
            resolve_as(&mut f, tp, e, 0);
            let lock = lock_of(&f).unwrap_or_else(|| panic!("a lock for tp={tp}"));
            let le = f.effects.get(lock).unwrap();
            assert_eq!(le.effect_owner, tp, "registered for the searcher");
            assert_eq!((le.s_range, le.o_range), (1, 0));
        }
    }
}
