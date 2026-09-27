//! Mystic Tomato — `c83011278.lua`.
//!
//! The twenty-second card, and the first to **special summon**. It also
//! brings the first filter that needs *context*: what counts as a legal
//! target depends on the effect doing the summoning, so the filter is a
//! closure over the effect rather than a bare function.
//!
//! ## A filter with arguments
//!
//! ```lua
//! function s.filter(c,e,tp)
//!     return c:IsAttackBelow(1500) and c:IsAttribute(ATTRIBUTE_DARK)
//!        and c:IsCanBeSpecialSummoned(e,0,tp,false,false)
//! end
//! ...
//! Duel.IsExistingMatchingCard(s.filter,tp,LOCATION_DECK,0,1,nil,e,tp)
//! ```
//!
//! The trailing `e, tp` are the reference's **extra arguments**: the scan
//! passes them to every call of the filter. The port's `Filter` is a
//! `&dyn Fn`, so a closure over `e` and `tp` says the same thing and
//! needs no seam change — the extra-argument list exists in Lua because
//! Lua has no closures at that call site.
//!
//! ## The room check is asked twice, and that is not redundant
//!
//! `target` refuses the activation when there is no free monster zone,
//! and `operation` returns early on the same question. A zone free when
//! the trigger was put on the chain can be taken by the time it resolves
//! — by something else chained on top of it — so the second check is the
//! one that matters. A port that kept only the first would summon into a
//! full field.
//!
//! ## The condition is where it *is*, not where it was
//!
//! `e:GetHandler():IsLocation(LOCATION_GRAVE)` — the opposite of
//! Sangan's, which reads `previous`. Mystic Tomato triggers on being
//! destroyed by battle and asks whether it actually reached the
//! graveyard: a monster banished instead of buried does not summon.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::{location, position};
use crate::card::attribute;
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 83_011_278;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // special summon
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::SPECIAL_SUMMON);
    api::set_type(f, e1, effect_type::SINGLE | effect_type::TRIGGER_O);
    api::set_code(f, e1, code::BATTLE_DESTROYED);
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
    api::is_location(f, handler, u16::from(location::GRAVE))
}

/// `s.filter(c, e, tp)` — as a closure over the two extra arguments the
/// reference passes along the scan.
fn filter_for(e: crate::event::EffectId, tp: u8) -> impl Fn(&mut Field, CardId) -> bool {
    move |f: &mut Field, c: CardId| {
        api::is_attack_below(f, c, 1500)
            && api::is_attribute(f, c, attribute::DARK)
            && api::is_can_be_special_summoned(f, c, e, 0, tp, false, false)
    }
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        let filter = filter_for(ctx.reason_effect, tp);
        return api::yes(
            api::get_location_count(f, tp, location::MZONE) > 0
                && api::is_existing_matching_card(
                    f,
                    Some(&filter),
                    tp,
                    DECK,
                    0,
                    1,
                    api::Except::None,
                ),
        );
    }
    api::set_operation_info(f, 0, category::SPECIAL_SUMMON, None, 1, tp, DECK as i32);
    api::yes(true)
}

fn operation(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    // Asked again: the zone that was free when this went on the chain
    // can have been taken by something chained on top of it.
    if api::get_location_count(f, tp, location::MZONE) <= 0 {
        return api::done();
    }
    let filter = filter_for(ctx.reason_effect, tp);
    api::hint(f, hint::SELECTMSG, tp, hintmsg::SPSUMMON);
    api::select_matching_card(f, tp, Some(&filter), tp, DECK, 0, 1, 1, api::Except::None);
    api::suspend(move |f, ctx| {
        let g = api::group_selected(f);
        if g.is_empty() {
            return api::done();
        }
        let tp = ctx.player;
        api::special_summon(f, g, 0, tp, tp, false, false, position::FACEUP_ATTACK);
        // `Duel.SpecialSummon` yields: the operation ends only once the
        // summon has happened, and it is that end — `check_level` back at
        // zero — that shuffles the deck the search disturbed. Returning here
        // ended the operation first, and the shuffle never came (fuzz seed
        // 1020, seen only once `MSG_SHUFFLE_DECK` was in the trace).
        api::suspend(|_, _| api::done())
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

    fn deck_monster(f: &mut Field, owner: u8, code_: u32, attack: i32, attr: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack,
                defense: 1000,
                attribute: attr,
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

    /// The tomato in `tp`'s graveyard, having just been destroyed in
    /// battle, with a deck of `(attack, attribute)` behind it and
    /// `occupied` of `tp`'s monster zones already taken.
    fn field_as(tp: u8, mine: &[(i32, u32)], occupied: u32) -> (Field, CardId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        for seat in 0..occupied {
            let mut blocker = Card::with_data(
                CardData {
                    code: 4_000 + seat,
                    type_: card_type::MONSTER | card_type::NORMAL,
                    level: 4,
                    attack: 1000,
                    defense: 1000,
                    ..Default::default()
                },
                tp,
            );
            blocker.current.controller = tp;
            let b = f.new_card(blocker);
            f.add_card(tp, b, location::MZONE, seat, false);
            f.cards[b].current.position = position::FACEUP_ATTACK;
        }
        let mut t = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        t.current.controller = tp;
        t.set_status(status::EFFECT_ENABLED, true);
        let tom = f.new_card(t);
        f.add_card(tp, tom, location::GRAVE, 0, false);
        f.cards[tom].current.position = position::FACEUP;
        f.cards[tom].previous.location = location::MZONE;
        f.initialize_card(tom);
        let deck = mine
            .iter()
            .enumerate()
            .map(|(i, &(atk, attr))| deck_monster(&mut f, tp, 9_100 + i as u32, atk, attr))
            .collect();
        (f, tom, deck)
    }

    fn field(mine: &[(i32, u32)], occupied: u32) -> (Field, CardId, Vec<CardId>) {
        field_as(0, mine, occupied)
    }

    fn effect_of(f: &Field, tom: CardId) -> crate::event::EffectId {
        f.cards[tom]
            .single_effect
            .equal_range(code::BATTLE_DESTROYED)[0]
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

    fn asks(f: &mut Field, e: crate::event::EffectId, tp: u8, chk: bool) -> bool {
        let ev = Event::new(code::BATTLE_DESTROYED);
        let ctx = ctx_for(e, &ev, tp);
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
        let mut ch = Chain::new(e, Event::new(code::BATTLE_DESTROYED));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.core
            .sub_solving_event
            .push_back(Event::new(code::BATTLE_DESTROYED));
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
                            .push_back(Event::new(code::BATTLE_DESTROYED));
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
                other => panic!("stopped with {other:?}"),
            }
        }
        run
    }

    fn resolve(f: &mut Field, e: crate::event::EffectId, choice: usize) -> Run {
        resolve_as(f, 0, e, choice)
    }

    const DARK: u32 = attribute::DARK;
    const LIGHT: u32 = attribute::LIGHT;

    /// **One printed optional trigger on being destroyed by battle**,
    /// declaring a special summon.
    #[test]
    fn the_script_registers_an_optional_trigger() {
        let (f, tom, _) = field(&[], 0);
        let ids = f.cards[tom]
            .single_effect
            .equal_range(code::BATTLE_DESTROYED);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::SINGLE));
        assert!(
            e.is_type(effect_type::TRIGGER_O),
            "optional: the controller may decline it"
        );
        assert!(!e.is_type(effect_type::TRIGGER_F));
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert_eq!(e.category, category::SPECIAL_SUMMON);
        assert_eq!(e.description, api::stringid(CODE, 0));
        assert!(e.condition.is_some());
    }

    /// **The condition is where it *is*, which is the opposite of
    /// Sangan's.** A tomato that was destroyed but banished instead of
    /// buried summons nothing.
    #[test]
    fn the_condition_wants_it_in_the_graveyard_now() {
        let (mut f, tom, _) = field(&[], 0);
        let e = effect_of(&f, tom);
        let ev = Event::new(code::BATTLE_DESTROYED);
        let ctx = ctx_for(e, &ev, 0);
        assert!(condition(&mut f, &ctx), "it is in the graveyard");
        // Banished on the way instead.
        f.cards[tom].current.location = location::REMOVED;
        assert!(
            !condition(&mut f, &ctx),
            "not buried, so nothing to trigger"
        );
        // And it is `current`, not `previous` — which still says MZONE.
        assert_eq!(f.cards[tom].previous.location, location::MZONE);
        let mut orphan = crate::effect::Effect::new(
            effect_type::SINGLE | effect_type::TRIGGER_O,
            code::BATTLE_DESTROYED,
        );
        orphan.owner = None;
        orphan.handler = None;
        let orphan = f.new_effect(orphan);
        assert!(api::get_handler(&f, orphan).is_none());
        let ctx = ctx_for(orphan, &ev, 0);
        assert!(!condition(&mut f, &ctx), "no handler, no trigger");
    }

    /// **The filter is three clauses**, each refused on its own.
    #[test]
    fn the_filter_wants_a_small_dark_monster_that_can_be_summoned() {
        let (mut f, tom, deck) = field(&[(1500, DARK), (1600, DARK), (1000, LIGHT)], 0);
        let e = effect_of(&f, tom);
        f.core.reason_effect = Some(e);
        f.core.reason_player = 0;
        let filter = filter_for(e, 0);
        assert!(filter(&mut f, deck[0]), "1500 DARK");
        assert!(!filter(&mut f, deck[1]), "1600 is above the bound");
        assert!(!filter(&mut f, deck[2]), "LIGHT is the wrong attribute");
        // And one that cannot be summoned at all.
        let stuck = deck_monster(&mut f, 0, 9_900, 1000, DARK);
        f.cards[stuck].set_status(status::FORBIDDEN, true);
        assert!(
            !filter(&mut f, stuck),
            "a forbidden card is not specially summonable"
        );

        // `nocheck` is what the script passes `false` for, and it is what
        // keeps a revive-limited monster in the deck. Passing `true`
        // would let a Nomi monster be summoned straight out of it.
        let nomi = deck_monster(&mut f, 0, 9_901, 1000, DARK);
        let mut limit = crate::effect::Effect::new(effect_type::SINGLE, code::REVIVE_LIMIT);
        limit.owner = Some(nomi);
        limit.handler = Some(nomi);
        let limit = f.new_effect(limit);
        f.cards[nomi]
            .single_effect
            .insert(code::REVIVE_LIMIT, limit);
        assert!(
            !filter(&mut f, nomi),
            "revive-limited, straight from a deck"
        );
        assert!(
            api::is_can_be_special_summoned(&mut f, nomi, e, 0, 0, true, false),
            "and `nocheck` is precisely what would let it through"
        );
    }

    /// **The filter asks about the summoning player**, not the other
    /// one. A unique-on-field copy sitting on *my* side blocks a second
    /// copy coming to my side and not to theirs, so a filter built for
    /// the wrong player answers the opposite way round.
    #[test]
    fn the_filter_asks_about_the_summoning_player() {
        let (mut f, tom, deck) = field(&[(1000, DARK)], 0);
        let e = effect_of(&f, tom);
        f.core.reason_effect = Some(e);
        f.core.reason_player = 0;

        // A copy of the deck monster already face-up on player 0's side,
        // with a one-of-these-at-a-time restriction.
        let code_of = f.cards[deck[0]].data.code;
        let mut blocker = Card::with_data(
            CardData {
                code: code_of,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                attribute: DARK,
                ..Default::default()
            },
            0,
        );
        blocker.current.controller = 0;
        blocker.current.location = location::MZONE;
        blocker.current.sequence = 0;
        blocker.current.position = position::FACEUP_ATTACK;
        blocker.set_status(status::EFFECT_ENABLED, true);
        blocker.unique_code = code_of;
        blocker.unique_location = u16::from(location::MZONE);
        blocker.unique_pos = [1, 0];
        blocker.unique_fieldid = 1;
        let blocker = f.new_card(blocker);
        f.players[0].mzone[0] = Some(blocker);
        f.core.unique_cards[0].push(blocker);
        let mut ue = crate::effect::Effect::new(effect_type::SINGLE, 0);
        ue.owner = Some(blocker);
        ue.handler = Some(blocker);
        let ue = f.new_effect(ue);
        f.cards[blocker].unique_effect = Some(ue);

        let mine = filter_for(e, 0);
        let theirs = filter_for(e, 1);
        assert!(
            !mine(&mut f, deck[0]),
            "a copy is already face-up on my side"
        );
        assert!(
            theirs(&mut f, deck[0]),
            "but not on theirs — so a filter built for the wrong player \
             would let it through"
        );
    }

    /// **Both halves of the activation check.** No room refuses it even
    /// with a legal monster; no legal monster refuses it even with room.
    #[test]
    fn it_needs_both_a_free_zone_and_a_legal_monster() {
        let (mut f, tom, _) = field(&[(1000, DARK)], 0);
        let e = effect_of(&f, tom);
        assert!(asks(&mut f, e, 0, false), "room and a target");

        let (mut f, tom, _) = field(&[(1000, DARK)], 5);
        let e = effect_of(&f, tom);
        assert!(!asks(&mut f, e, 0, false), "no free monster zone");

        let (mut f, tom, _) = field(&[(1600, DARK), (1000, LIGHT)], 0);
        let e = effect_of(&f, tom);
        assert!(!asks(&mut f, e, 0, false), "room, but nothing legal");
    }

    /// **Both scans are one-sided.** A legal monster in the *opponent's*
    /// deck neither permits the activation nor appears in the selection —
    /// the `o` mask is zero in both places.
    #[test]
    fn neither_scan_reaches_the_opponents_deck() {
        let (mut f, tom, _) = field(&[], 0);
        let theirs = deck_monster(&mut f, 1, 9_800, 1000, DARK);
        let e = effect_of(&f, tom);
        assert!(
            !asks(&mut f, e, 0, false),
            "their deck is full of legal monsters, and none of them count"
        );
        // With one of mine as well, the selection offers only mine.
        let mine = deck_monster(&mut f, 0, 9_801, 1000, DARK);
        assert!(asks(&mut f, e, 0, false));
        let run = resolve(&mut f, e, 0);
        assert_eq!(run.offered[0], vec![mine], "mine only");
        assert!(!run.offered[0].contains(&theirs));
        assert_eq!(
            f.cards[theirs].current.location,
            location::DECK,
            "and theirs stays put"
        );
    }

    /// **The recorded operation names the summoner's deck**, with no
    /// card — nobody has looked in it yet.
    #[test]
    fn the_recorded_operation_names_the_deck() {
        for tp in [0u8, 1] {
            let (mut f, tom, _) = field_as(tp, &[(1000, DARK), (1200, DARK)], 0);
            let e = effect_of(&f, tom);
            resolve_as(&mut f, tp, e, 0);
            let op = f.core.current_chain[0]
                .opinfos
                .get(&category::SPECIAL_SUMMON)
                .cloned()
                .unwrap_or_else(|| panic!("a SPECIAL_SUMMON operation for tp={tp}"));
            assert_eq!(op.cards, None, "no card can be named yet");
            assert_eq!(op.count, 1);
            assert_eq!(op.player, tp);
            assert_eq!(op.param, DECK as i32);
        }
    }

    /// **It summons the chosen monster face-up in attack**, on its own
    /// side, and leaves the rest of the deck alone.
    #[test]
    fn it_summons_the_chosen_monster_face_up_in_attack() {
        let (mut f, tom, deck) = field(&[(1000, DARK), (1200, DARK)], 0);
        let e = effect_of(&f, tom);
        let run = resolve(&mut f, e, 0);
        assert_eq!(run.asked[0], (0, 1, 1), "the controller picks one");
        let summoned = run.offered[0][0];
        let left = *deck.iter().find(|&&c| c != summoned).expect("the other");
        assert_eq!(f.cards[summoned].current.location, location::MZONE);
        assert_eq!(f.cards[summoned].current.controller, 0, "its own side");
        assert_eq!(
            f.cards[summoned].current.position,
            position::FACEUP_ATTACK,
            "face-up attack, as the script asks"
        );
        assert_eq!(f.cards[left].current.location, location::DECK, "only one");
    }

    /// **The prompt is the special-summon message, to the controller.**
    #[test]
    fn the_prompt_is_the_special_summon_message() {
        let (mut f, tom, _) = field(&[(1000, DARK)], 0);
        let e = effect_of(&f, tom);
        resolve(&mut f, e, 0);
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::Hint { kind, player: 0, value }
                    if *kind == hint::SELECTMSG && *value == hintmsg::SPSUMMON
            )),
            "HINTMSG_SPSUMMON to the summoning player"
        );
    }

    /// **The room check at resolution is the one that matters.**
    ///
    /// A zone free when the trigger went on the chain can be taken by the
    /// time it resolves. Here the field fills up in between, and the
    /// operation must summon nothing rather than force a monster into a
    /// full row.
    #[test]
    fn a_field_that_filled_up_in_between_summons_nothing() {
        let (mut f, tom, deck) = field(&[(1000, DARK)], 0);
        let e = effect_of(&f, tom);
        assert!(asks(&mut f, e, 0, false), "there was room at activation");
        // Five monsters arrive while the trigger sits on the chain.
        for seat in 0..5u32 {
            let mut blocker = Card::with_data(
                CardData {
                    code: 5_000 + seat,
                    type_: card_type::MONSTER | card_type::NORMAL,
                    level: 4,
                    attack: 1000,
                    defense: 1000,
                    ..Default::default()
                },
                0,
            );
            blocker.current.controller = 0;
            let b = f.new_card(blocker);
            f.add_card(0, b, location::MZONE, seat, false);
            f.cards[b].current.position = position::FACEUP_ATTACK;
        }
        let run = resolve(&mut f, e, 0);
        assert!(run.offered.is_empty(), "nothing is even offered");
        assert_eq!(
            f.cards[deck[0]].current.location,
            location::DECK,
            "and nothing is summoned into a full row"
        );
    }
}
