//! Gatling Dragon (`87751584`) — `cardscripts/c87751584.lua`.
//!
//! ```text
//! "Barrel Dragon" + "Blowback Dragon"
//! Toss a coin 3 times. Destroy a number of monsters on the field equal
//! to the number of Heads.
//! ```
//!
//! The pool's first **Fusion Monster**, and the first card of any kind to
//! draw from the duel's random generator.
//!
//! ## The first coin toss in the pool
//!
//! Every duel this project has compared so far has been deterministic in
//! a particular way: under `DUEL_PSEUDO_SHUFFLE` nothing draws from the
//! generator, so the two engines agreeing said nothing about their seeds.
//! `examples/trace.rs` names the seed explicitly on both sides for
//! exactly this moment — the first card that tosses a coin is the first
//! card that can tell the two generators apart.
//!
//! ## Its fusion procedure is registered and cannot be reached
//!
//! `Fusion.AddProcMix` names Barrel Dragon and Blowback Dragon, neither
//! of which is in this pool — and more to the point, **nothing in this
//! pool can perform a Fusion Summon at all**. See
//! [`super::proc_fusion`]: the procedure is registered, and its condition
//! and operation panic. Metamorphosis is the only way this card reaches
//! the field here.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, CardId};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 87751584;

/// `Fusion.AddProcMix(c,true,true,81480460,25551951)` — Barrel Dragon and
/// Blowback Dragon.
const MATERIALS: [u32; 2] = [81_480_460, 25_551_951];

const MZONE: u32 = location::MZONE as u32;

pub fn initial_effect(f: &mut Field, c: CardId) {
    api::enable_revive_limit(f, c);
    super::proc_fusion::add_proc_mix(f, c, true, true, &MATERIALS);
    // Toss 3 coins and destroy monsters on the field
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::DESTROY | category::COIN);
    api::set_type(f, e1, effect_type::IGNITION);
    api::set_range(f, e1, u16::from(location::MZONE));
    api::set_count_limit(f, e1, 1, 0, 0);
    api::set_target(f, e1, destg);
    api::set_operation(f, e1, desop);
    api::register_effect(f, c, e1, false);
}

/// `s.destg` — **the count slot carries the number of coins**, not a
/// number of cards. `SetOperationInfo(0, CATEGORY_COIN, nil, 0, tp, 3)`
/// puts the three in the *parameter*, and zero in the count.
fn destg(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        return api::yes(api::is_existing_matching_card(
            f,
            None,
            tp,
            MZONE,
            MZONE,
            1,
            api::Except::None,
        ));
    }
    api::set_operation_info(f, 0, category::COIN, None, 0, tp, 3);
    api::yes(true)
}

/// `s.desop` — toss, count, then destroy that many.
fn desop(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let g = api::get_matching_group(f, None, tp, MZONE, MZONE, api::Except::None);
    if g.is_empty() {
        return api::done();
    }
    api::toss_coin(f, tp, 3);
    api::suspend(move |f, _ctx| {
        let ct = api::count_heads(&api::tossed_coins(f));
        if ct == 0 {
            return api::done();
        }
        // **Capped at what is on the board**, so three heads against two
        // monsters destroys two rather than asking an impossible question.
        let ct = (ct as usize).min(g.len()) as u8;
        api::hint(f, hint::SELECTMSG, tp, hintmsg::DESTROY);
        api::group_select(f, &g, tp, ct, ct, api::Except::None);
        api::suspend(move |f, _ctx| {
            let dg = api::group_selected(f);
            // `Duel.HintSelection(dg)`: the second argument defaults to **true**
            // (`libduel.cpp` HintSelection), so this is `MSG_CARD_SELECTED`.
            api::hint_selection(f, &dg, true);
            api::destroy(f, dg, reason::EFFECT);
            api::done()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::effect::flag;
    use crate::event::{code, EffectId, Event};
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    fn monster(f: &mut Field, player: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 3300 + seq + u32::from(player) * 50,
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

    /// Gatling Dragon on `tp`'s field, with monsters on both sides.
    fn board(tp: u8, mine: u32, theirs: u32) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let gd = f.new_card(d);
        f.add_card(tp, gd, location::MZONE, 0, false);
        f.cards[gd].current.position = position::FACEUP_ATTACK;
        f.initialize_card(gd);
        let ours = (0..mine).map(|i| monster(&mut f, tp, i + 1)).collect();
        let theirs = (0..theirs).map(|i| monster(&mut f, 1 - tp, i)).collect();
        (f, gd, ours, theirs)
    }

    fn effect_of(f: &Field, gd: CardId) -> EffectId {
        f.cards[gd].field_effect.equal_range(0)[0]
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

    #[derive(Default)]
    struct Run {
        /// Each `SelectCard`: who, offered, and the bounds.
        questions: Vec<(u8, Vec<CardId>, u8, u8)>,
        hints: Vec<(u8, u8, u64)>,
        /// Each toss as the host saw it: who, and how many coins.
        tosses: Vec<(u8, usize)>,
        /// Every `CardSelected` announcement.
        announced: usize,
    }

    /// Drive the operation with the coin results forced, so a test is
    /// about the card rather than about the generator.
    fn resolve_with(f: &mut Field, e: EffectId, tp: u8, faces: &[bool]) -> Run {
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.core.sub_solving_event.push_back(Event::new(0));
        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player: tp,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        let mut run = Run::default();
        let mut seen = 0usize;
        for _ in 0..8192 {
            while seen < f.messages.len() {
                match &f.messages[seen] {
                    Message::Hint {
                        kind,
                        player,
                        value,
                    } => run.hints.push((*kind, *player, *value)),
                    Message::TossCoin { player, results } => {
                        run.tosses.push((*player, results.len()))
                    }
                    Message::CardSelected { .. } => run.announced += 1,
                    _ => {}
                }
                seen += 1;
            }
            // The toss is the engine's; forcing the faces here is what
            // makes "three heads" a repeatable board rather than a seed.
            // **Only ever the faces, never the count** — overwriting the
            // length would hide how many coins the card asked for.
            if f.core.coin_results.len() == faces.len() && f.core.coin_results != faces {
                f.core.coin_results = faces.to_vec();
            }
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        break;
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectCard {
                        player,
                        cards,
                        min,
                        max,
                        ..
                    }) => {
                        run.questions.push((*player, cards.clone(), *min, *max));
                        let min = usize::from(*min);
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, i as i32);
                        }
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                Status::End => break,
            }
        }
        run
    }

    fn alive(f: &Field, c: CardId) -> bool {
        f.cards[c].current.location == location::MZONE
    }

    /// The reason a card left the field with.
    fn gone_because(f: &Field, c: CardId) -> u32 {
        f.cards[c].reason
    }

    mod registration {
        use super::*;

        /// **The fusion procedure is registered and its materials
        /// recorded**, even though nothing in this pool can ask it
        /// anything.
        #[test]
        fn the_fusion_procedure_is_registered_with_its_materials() {
            let (f, gd, _, _) = board(0, 0, 0);
            let ids = f.cards[gd].single_effect.equal_range(code::FUSION_MATERIAL);
            assert_eq!(ids.len(), 1, "one material procedure");
            let x = f.effects.get(ids[0]).expect("the procedure");
            assert!(x.is_flag(flag::CANNOT_DISABLE));
            assert!(x.is_flag(flag::UNCOPYABLE));
            assert_eq!(
                api::fusion_materials(&f, gd),
                &MATERIALS,
                "Barrel Dragon and Blowback Dragon"
            );
            for wanted in [code::UNSUMMONABLE_CARD, code::REVIVE_LIMIT] {
                assert_eq!(f.cards[gd].single_effect.equal_range(wanted).len(), 1);
            }
        }

        /// **A Fusion Monster's printed line puts it in the Extra Deck**,
        /// which is where the harness deals it on both engines.
        #[test]
        fn its_printed_line_is_a_fusion_monster() {
            let d = crate::cards::card_data(CODE).expect("printed data");
            assert!(d.is_type(card_type::FUSION));
            assert!(d.is_type(card_type::MONSTER));
            assert_eq!((d.level, d.attack, d.defense), (8, 2600, 1200));
        }

        #[test]
        fn one_ignition_in_two_categories() {
            let (f, gd, _, _) = board(0, 0, 0);
            let e = f.effects.get(effect_of(&f, gd)).expect("the effect");
            assert!(e.is_type(effect_type::IGNITION));
            assert_eq!(e.category, category::DESTROY | category::COIN);
            assert_eq!(e.count_limit, 1);
            assert_eq!(e.range, u16::from(location::MZONE));
        }
    }

    mod the_toss {
        use super::*;

        /// **The scan is two-sided** — and saying so needs our own row
        /// empty, which means taking Gatling Dragon off the field, since
        /// it is itself a monster the scan would find.
        #[test]
        fn the_scan_reaches_both_sides() {
            let (mut f, gd, _, _) = board(0, 0, 1);
            let e = effect_of(&f, gd);
            f.remove_card(gd);
            let ev = Event::new(0);
            assert_eq!(
                destg(&mut f, &ctx_for(e, &ev, 0), false, None).finished(),
                Some(1),
                "only the opponent's monster is left, and it counts"
            );
            let (mut f, gd, _, _) = board(0, 1, 0);
            let e = effect_of(&f, gd);
            assert_eq!(
                destg(&mut f, &ctx_for(e, &ev, 0), false, None).finished(),
                Some(1),
                "and so does one of ours"
            );
        }

        /// An empty field offers nothing.
        #[test]
        fn an_empty_field_is_not_offered() {
            let (mut f, gd, _, _) = board(0, 0, 0);
            let e = effect_of(&f, gd);
            f.remove_card(gd);
            let ev = Event::new(0);
            assert_eq!(
                destg(&mut f, &ctx_for(e, &ev, 0), false, None).finished(),
                Some(0)
            );
        }

        /// **The count slot carries the number of coins**, not a number
        /// of cards: the three goes in the *parameter*.
        #[test]
        fn the_target_records_three_coins_in_the_parameter() {
            let (mut f, gd, _, _) = board(1, 0, 1);
            let e = effect_of(&f, gd);
            let mut ch = Chain::new(e, Event::new(0));
            ch.triggering_player = 1;
            ch.chain_count = 1;
            f.core.current_chain.push(ch);
            let ev = Event::new(0);
            destg(&mut f, &ctx_for(e, &ev, 1), true, None);
            let info = f.core.current_chain[0]
                .opinfos
                .get(&category::COIN)
                .expect("the coin category");
            assert_eq!(info.count, 0, "no cards are named");
            assert_eq!(info.param, 3, "three coins");
            assert_eq!(info.player, 1, "the activating player");
        }

        /// **Three heads destroys three**, chosen by the controller with
        /// the destroy prompt.
        #[test]
        fn three_heads_destroys_three() {
            let (mut f, gd, ours, theirs) = board(0, 2, 2);
            let e = effect_of(&f, gd);
            let run = resolve_with(&mut f, e, 0, &[true, true, true]);
            assert_eq!(
                run.tosses,
                vec![(0, 3)],
                "one toss, of three coins, by the controller"
            );
            assert_eq!(run.questions.len(), 1, "one choice");
            let (player, offered, min, max) = run.questions[0].clone();
            assert_eq!(player, 0);
            assert_eq!((min, max), (3, 3), "exactly three");
            assert_eq!(offered.len(), 5, "every monster on the field, both sides");
            assert!(offered.contains(&gd), "including itself");
            assert!(offered.contains(&theirs[0]), "and the opponent's");
            let gone = [gd, ours[0], ours[1], theirs[0], theirs[1]]
                .iter()
                .filter(|&&c| !alive(&f, c))
                .count();
            assert_eq!(gone, 3);
            assert!(run.hints.contains(&(hint::SELECTMSG, 0, hintmsg::DESTROY)));
            assert_eq!(run.announced, 1, "and the choice is shown to the table as MSG_CARD_SELECTED (HintSelection's default)");
            for &c in &[gd, ours[0], ours[1], theirs[0], theirs[1]] {
                if !alive(&f, c) {
                    assert!(
                        gone_because(&f, c) & reason::EFFECT != 0,
                        "destroyed by an effect"
                    );
                }
            }
        }

        /// **No heads destroys nothing**, and asks nothing either.
        #[test]
        fn no_heads_asks_nothing() {
            let (mut f, gd, ours, _) = board(0, 2, 0);
            let e = effect_of(&f, gd);
            let run = resolve_with(&mut f, e, 0, &[false, false, false]);
            assert_eq!(run.tosses, vec![(0, 3)], "it still tosses");
            assert!(run.questions.is_empty(), "but asks nothing");
            assert!(
                !run.hints.contains(&(hint::SELECTMSG, 0, hintmsg::DESTROY)),
                "and does not prompt for a destruction it is not making"
            );
            assert!(alive(&f, gd) && alive(&f, ours[0]) && alive(&f, ours[1]));
        }

        /// **Capped at what is on the board.** Three heads against two
        /// monsters destroys two, rather than asking for three.
        #[test]
        fn more_heads_than_monsters_destroys_what_there_is() {
            let (mut f, gd, ours, _) = board(0, 1, 0);
            let e = effect_of(&f, gd);
            let run = resolve_with(&mut f, e, 0, &[true, true, true]);
            let (_, offered, min, max) = run.questions[0].clone();
            assert_eq!(offered.len(), 2, "only two monsters exist");
            assert_eq!((min, max), (2, 2), "so only two are asked for");
            assert!(!alive(&f, gd) && !alive(&f, ours[0]), "and both go");
        }

        /// One head destroys one.
        #[test]
        fn one_head_destroys_one() {
            let (mut f, gd, ours, _) = board(0, 2, 0);
            let e = effect_of(&f, gd);
            let run = resolve_with(&mut f, e, 0, &[true, false, false]);
            assert_eq!((run.questions[0].2, run.questions[0].3), (1, 1));
            let gone = [gd, ours[0], ours[1]]
                .iter()
                .filter(|&&c| !alive(&f, c))
                .count();
            assert_eq!(gone, 1);
        }

        /// An empty board resolves to nothing without tossing at all.
        #[test]
        fn an_empty_board_tosses_nothing() {
            let mut f = Field::new(8000);
            f.infos.turn_id = 3;
            let mut d = Card::with_data(crate::cards::card_data(CODE).expect("data"), 0);
            d.current.controller = 0;
            d.set_status(status::EFFECT_ENABLED, true);
            let gd = f.new_card(d);
            f.add_card(0, gd, location::GRAVE, 0, false);
            f.initialize_card(gd);
            let e = effect_of(&f, gd);
            let run = resolve_with(&mut f, e, 0, &[true, true, true]);
            assert!(run.tosses.is_empty(), "no monsters, so no toss");
            assert!(run.questions.is_empty());
        }
    }
}
