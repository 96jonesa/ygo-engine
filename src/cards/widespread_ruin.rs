//! Widespread Ruin — `c77754944.lua`.
//!
//! Destroys the attacking player's monster with the **highest attack**,
//! and when several tie, asks the activating player which of them goes.
//!
//! ## The condition reads the attacker's controller, not the turn player
//!
//! `Duel.GetAttacker():IsControler(1-tp)`. Mirror Force asks
//! `IsTurnPlayer(1-tp)` for the same "an opponent's monster attacked"
//! idea, and the two are not the same question: `IsControler` reads the
//! attacking *card*, so a monster attacking under someone else's control
//! is judged by who controls it now. There is no attacker at all outside
//! a battle, so the condition also has to survive being asked with an
//! empty slot — the reference would error there, and the port answers no.
//!
//! ## The maximum is a group
//!
//! `g:GetMaxGroup(Card.GetAttack)` keeps **every** card at the maximum,
//! not the first one found. That is the whole reason the card has a
//! selection at all: with a unique maximum it destroys silently, and the
//! player is only asked when there is a genuine tie. A port that took
//! the first maximum would pass every test where attacks differ and be
//! wrong exactly where this card is interesting.
//!
//! ## The group is re-scanned at resolution
//!
//! `target` records a `DESTROY` operation over the max group, then
//! `activate` scans again and takes the maximum again. Nothing is
//! targeted — the card has no `CARD_TARGET` property — so a board that
//! changed between activation and resolution is read fresh, and the
//! recorded operation is information for the opponent, not a promise.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 77_754_944;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Destroy 1 attacking monster with the highest ATK
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::DESTROY);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_code(f, e1, code::ATTACK_ANNOUNCE);
    api::set_condition(f, e1, condition);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

fn condition(f: &mut Field, ctx: &Ctx) -> bool {
    match api::get_attacker(f) {
        Some(a) => api::is_controler(f, a, 1 - ctx.player),
        None => false,
    }
}

fn filter(f: &mut Field, c: CardId) -> bool {
    api::is_faceup(f, c) && api::is_attack_pos(f, c)
}

const MZONE: u32 = location::MZONE as u32;

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        return api::yes(api::is_existing_matching_card(
            f,
            Some(&filter),
            tp,
            0,
            MZONE,
            1,
            api::Except::None,
        ));
    }
    let g = api::get_matching_group(f, Some(&filter), tp, 0, MZONE, api::Except::None);
    let tg = api::get_max_group(f, &g, &api::attack_value);
    api::set_operation_info(f, 0, category::DESTROY, Some(tg), 1, 0, 0);
    api::yes(true)
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let g = api::get_matching_group(f, Some(&filter), tp, 0, MZONE, api::Except::None);
    if g.is_empty() {
        return api::done();
    }
    let tg = api::get_max_group(f, &g, &api::attack_value);
    if tg.len() > 1 {
        api::hint(f, hint::SELECTMSG, tp, hintmsg::DESTROY);
        api::group_select(f, &tg, tp, 1, 1, api::Except::None);
        return api::suspend(move |f, _ctx| {
            let sg = api::group_selected(f);
            api::destroy(f, sg, reason::EFFECT);
            api::done()
        });
    }
    api::destroy(f, tg, reason::EFFECT);
    api::done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::duel::phases;
    use crate::effect::flag;
    use crate::field::Message;
    use crate::position;
    use crate::processor::{Kind, Status};

    /// `target`, asked as a plain yes/no.
    fn asks(f: &mut Field, ctx: &Ctx, chk: bool) -> bool {
        target(f, ctx, chk, None).finished().unwrap_or(0) != 0
    }

    fn put(f: &mut Field, owner: u8, data: CardData, loc: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(data, owner);
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        let fid = f.next_field_id_raw();
        f.cards[id].fieldid = fid;
        f.cards[id].fieldid_r = fid;
        id
    }

    fn monster_data(code_: u32, atk: i32) -> CardData {
        CardData {
            code: code_,
            type_: card_type::MONSTER | card_type::NORMAL,
            level: 4,
            attack: atk,
            defense: 1000,
            ..Default::default()
        }
    }

    fn monster(f: &mut Field, owner: u8, seat: u32, atk: i32, pos: u8) -> CardId {
        let id = put(
            f,
            owner,
            monster_data(1000 + seat + u32::from(owner) * 100, atk),
            location::MZONE,
            seat,
        );
        f.cards[id].current.position = pos;
        id
    }

    /// Player 1's Battle Step with Widespread Ruin Set in player 0's row
    /// since an earlier turn, and `theirs` monsters of player 1's given as
    /// `(attack, position)`. Player 0 has an **empty board**, so the
    /// attack is direct and the only card question the duel can ask is
    /// the card's own tie-break.
    fn field(theirs: &[(i32, u8)]) -> (Field, CardId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 4;
        f.infos.turn_player = 1;
        f.infos.phase = phases::BATTLE_STEP;
        let theirs = theirs
            .iter()
            .enumerate()
            .map(|(i, &(atk, pos))| monster(&mut f, 1, i as u32, atk, pos))
            .collect();
        let wr = put(
            &mut f,
            0,
            crate::cards::card_data(CODE).expect("printed data"),
            location::SZONE,
            0,
        );
        f.cards[wr].current.position = position::FACEDOWN;
        f.initialize_card(wr);
        (f, wr, theirs)
    }

    /// What one run of the Battle Step saw: the chain counts player 0 was
    /// offered, every card question it was asked, and the `DESTROY`
    /// operation the chain link recorded.
    struct Run {
        offered: Vec<usize>,
        chosen_from: Vec<Vec<CardId>>,
        /// `(player, min, max)` of each card question, which is as much
        /// of the card's asking as the answer alone cannot show.
        asked: Vec<(u8, u8, u8)>,
        opinfo: Option<crate::chain::OpTarget>,
    }

    /// Declare player 1's attack with its first attacker, activating the
    /// first offer in every window and answering each card question with
    /// its `choice`-th card.
    fn attack_and_respond(f: &mut Field, choice: usize) -> Run {
        f.emplace(Kind::BattleCommand {
            state: Box::default(),
        });
        let mut attacked = false;
        let mut run = Run {
            offered: Vec::new(),
            chosen_from: Vec::new(),
            asked: Vec::new(),
            opinfo: None,
        };
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectBattleCmd { attackable, .. }) if !attacked => {
                        assert!(!attackable.is_empty(), "the attack is available");
                        f.core.returns.set(1);
                        attacked = true;
                    }
                    Some(Message::SelectBattleCmd { .. }) => break,
                    Some(Message::SelectChain { player, chains, .. }) => {
                        if *player == 0 {
                            run.offered.push(chains.len());
                        }
                        if run.opinfo.is_none() {
                            if let Some(link) = f.core.current_chain.last() {
                                run.opinfo = link.opinfos.get(&category::DESTROY).cloned();
                            }
                        }
                        f.core.returns.set(if chains.is_empty() { -1 } else { 0 });
                    }
                    Some(Message::SelectCard {
                        player,
                        min,
                        max,
                        cards,
                        ..
                    }) => {
                        run.asked.push((*player, *min, *max));
                        let min = usize::from(*min);
                        run.chosen_from.push(cards.clone());
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, (choice + i) as i32);
                        }
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                _ => break,
            }
        }
        assert!(attacked, "the attack was declared");
        run
    }

    const UP: u8 = position::FACEUP_ATTACK;

    /// **One printed activate effect on the attack announcement**, with a
    /// condition and the destroy category — and no `CARD_TARGET`: the
    /// card records an operation but fixes nothing.
    #[test]
    fn the_script_registers_a_printed_activate_effect() {
        let (f, wr, _) = field(&[]);
        let ids = f.cards[wr].field_effect.equal_range(code::ATTACK_ANNOUNCE);
        assert_eq!(ids.len(), 1, "one effect on EVENT_ATTACK_ANNOUNCE");
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(e.is_flag(flag::INITIAL), "stamped while initializing");
        assert!(!e.is_flag(flag::CARD_TARGET), "it does not target");
        assert!(e.condition.is_some(), "the script sets one");
        assert_eq!(e.category, category::DESTROY);
        assert_eq!(e.handler, Some(wr));
        assert!(
            f.cards[wr]
                .field_effect
                .equal_range(code::FREE_CHAIN)
                .is_empty(),
            "not a free-chain card"
        );
    }

    /// **The condition reads the attacker's controller**, which is not the
    /// turn player: an attacker of mine on the opponent's turn is still
    /// mine, and the card stays down.
    #[test]
    fn the_condition_reads_who_controls_the_attacker() {
        let (mut f, wr, theirs) = field(&[(1700, UP)]);
        let mine = monster(&mut f, 0, 0, 1700, UP);
        let e = f.cards[wr].field_effect.equal_range(code::ATTACK_ANNOUNCE)[0];
        let ev = crate::event::Event::new(0);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(!condition(&mut f, &ctx), "no attacker at all: no");
        f.core.attacker = Some(theirs[0]);
        assert!(condition(&mut f, &ctx), "the opponent's monster attacked");
        f.core.attacker = Some(mine);
        assert!(!condition(&mut f, &ctx), "my own monster attacked");
        // Whose turn it is does not enter into it — the same board with
        // the turn flipped gives the same two answers.
        f.infos.turn_player = 0;
        assert!(!condition(&mut f, &ctx));
        f.core.attacker = Some(theirs[0]);
        assert!(condition(&mut f, &ctx), "still the opponent's card");
    }

    /// **The filter wants face-up *and* attack position.** Mirror Force
    /// asks only the second, so a face-down attack-position monster is
    /// in its group and not in this one.
    #[test]
    fn the_filter_wants_face_up_attack_position() {
        let mut f = Field::new(8000);
        let up = monster(&mut f, 1, 0, 1700, position::FACEUP_ATTACK);
        let down = monster(&mut f, 1, 1, 1700, position::FACEDOWN_ATTACK);
        let def = monster(&mut f, 1, 2, 1700, position::FACEUP_DEFENSE);
        let set = monster(&mut f, 1, 3, 1700, position::FACEDOWN_DEFENSE);
        assert!(filter(&mut f, up));
        assert!(
            !filter(&mut f, down),
            "face-down attack position is attack position, but not face-up"
        );
        assert!(!filter(&mut f, def));
        assert!(!filter(&mut f, set));
    }

    /// **`target` records the maximum with a count of one** — however
    /// many tie, the card destroys exactly one. Read from a real chain
    /// link, since that is the only place an operation lands.
    #[test]
    fn the_target_records_the_max_group_and_a_count_of_one() {
        let (mut f, _, theirs) = field(&[(1700, UP), (1900, UP), (1900, UP)]);
        let run = attack_and_respond(&mut f, 0);
        let op = run.opinfo.expect("the link recorded a DESTROY operation");
        let mut want = vec![theirs[1], theirs[2]];
        want.sort_unstable();
        assert_eq!(op.cards, Some(want), "both of the tied maximum");
        assert_eq!(op.count, 1, "one card is destroyed, not the group");
    }

    /// **`target` answers the scan** as a plain yes when something is
    /// there to destroy.
    #[test]
    fn the_target_answers_the_scan() {
        let (mut f, wr, _) = field(&[(1700, UP)]);
        let e = f.cards[wr].field_effect.equal_range(code::ATTACK_ANNOUNCE)[0];
        let ev = crate::event::Event::new(0);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(asks(&mut f, &ctx, false), "there is something to destroy");
        assert!(asks(&mut f, &ctx, true), "and the recording half says yes");
    }

    /// **`target` says no on an empty board**, and no on one where every
    /// monster is face-down or in defence.
    #[test]
    fn the_target_refuses_when_the_scan_is_empty() {
        let e_of =
            |f: &Field, wr: CardId| f.cards[wr].field_effect.equal_range(code::ATTACK_ANNOUNCE)[0];
        let (mut f, wr, _) = field(&[]);
        let ev = crate::event::Event::new(0);
        let ctx = Ctx {
            reason_effect: e_of(&f, wr),
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(!asks(&mut f, &ctx, false), "nothing on the field");
        let (mut f, wr, _) = field(&[(1700, position::FACEUP_DEFENSE)]);
        let ctx = Ctx {
            reason_effect: e_of(&f, wr),
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(!asks(&mut f, &ctx, false), "defence is not attack position");
        // The positive sibling: the same board with the monster stood up.
        let (mut f, wr, _) = field(&[(1700, UP)]);
        let ctx = Ctx {
            reason_effect: e_of(&f, wr),
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(asks(&mut f, &ctx, false), "attack position is");
    }

    /// **A unique maximum is destroyed without asking anyone.** The
    /// 1900 goes; the 1700 and the 1200 stay, and no card question is
    /// raised at all.
    #[test]
    fn a_unique_maximum_is_destroyed_silently() {
        let (mut f, wr, theirs) = field(&[(1700, UP), (1900, UP), (1200, UP)]);
        let run = attack_and_respond(&mut f, 0);
        assert!(run.offered.contains(&1), "offered: {:?}", run.offered);
        assert!(
            run.chosen_from.is_empty(),
            "nothing to choose between: {:?}",
            run.chosen_from
        );
        assert_eq!(
            f.cards[theirs[1]].current.location,
            location::GRAVE,
            "the 1900"
        );
        assert!(f.cards[theirs[1]].reason & reason::EFFECT != 0, "by effect");
        assert_eq!(f.cards[theirs[0]].current.location, location::MZONE);
        assert_eq!(f.cards[theirs[2]].current.location, location::MZONE);
        assert_eq!(
            f.cards[wr].current.location,
            location::GRAVE,
            "a resolved Normal Trap"
        );
    }

    /// **A tie is put to the activating player**, who is offered exactly
    /// the tied cards — not the whole board — and whose answer is the
    /// one that is destroyed.
    #[test]
    fn a_tie_is_chosen_between_and_only_the_choice_is_destroyed() {
        for choice in [0usize, 1] {
            let (mut f, _, theirs) = field(&[(1900, UP), (1200, UP), (1900, UP)]);
            let run = attack_and_respond(&mut f, choice);
            assert_eq!(run.chosen_from.len(), 1, "asked once");
            assert_eq!(
                run.asked[0],
                (0, 1, 1),
                "the activating player chooses exactly one"
            );
            let mut want = vec![theirs[0], theirs[2]];
            want.sort_unstable();
            assert_eq!(run.chosen_from[0], want, "only the tied maximum is offered");
            let gone = want[choice];
            let kept = want[1 - choice];
            assert_eq!(f.cards[gone].current.location, location::GRAVE);
            assert!(f.cards[gone].reason & reason::EFFECT != 0, "by effect");
            assert_eq!(
                f.cards[kept].current.location,
                location::MZONE,
                "the other half of the tie survives"
            );
            assert_eq!(
                f.cards[theirs[1]].current.location,
                location::MZONE,
                "the 1200 was never in it"
            );
        }
    }

    /// **The scan is one-sided.** The activating player's own monsters
    /// are never in it, whatever they are — the `s` mask the script
    /// passes is zero, so a 2500 of mine outranks nothing. This is the
    /// asymmetry that makes the card a Trap rather than a board wipe.
    #[test]
    fn my_own_monsters_are_never_in_the_scan() {
        let (mut f, _, theirs) = field(&[(1900, UP)]);
        let mine = monster(&mut f, 0, 0, 2500, UP);
        let run = attack_and_respond(&mut f, 0);
        let op = run.opinfo.expect("a DESTROY operation");
        assert_eq!(
            op.cards,
            Some(vec![theirs[0]]),
            "the 2500 of mine is not the maximum because it is not in the group"
        );
        assert_eq!(
            f.cards[mine].current.location,
            location::MZONE,
            "and it is not destroyed"
        );
        assert_eq!(f.cards[theirs[0]].current.location, location::GRAVE);
    }

    /// **The resolution re-scans through the filter**, not over the
    /// whole monster row. A face-up *defender* with the larger attack is
    /// outside the group, so it neither wins the maximum nor suppresses
    /// the tie between the two attackers.
    #[test]
    fn the_resolution_filters_rather_than_taking_the_row() {
        let (mut f, _, theirs) = field(&[(1900, UP), (2500, position::FACEUP_DEFENSE), (1900, UP)]);
        let run = attack_and_respond(&mut f, 0);
        assert_eq!(run.chosen_from.len(), 1, "the two attackers tie");
        let mut want = vec![theirs[0], theirs[2]];
        want.sort_unstable();
        assert_eq!(
            run.chosen_from[0], want,
            "the 2500 in defence is not offered"
        );
        assert_eq!(
            f.cards[theirs[1]].current.location,
            location::MZONE,
            "and it survives"
        );
    }

    /// **The prompt is the destroy hint**, sent to the activating player
    /// before the choice — the script's `Duel.Hint(HINT_SELECTMSG, tp,
    /// HINTMSG_DESTROY)`.
    #[test]
    fn the_tie_prompt_is_the_destroy_hint() {
        let (mut f, _, _) = field(&[(1900, UP), (1900, UP)]);
        attack_and_respond(&mut f, 0);
        assert!(
            f.messages.iter().any(|m| matches!(
                m,
                Message::Hint {
                    kind,
                    player: 0,
                    value
                } if *kind == hint::SELECTMSG && *value == hintmsg::DESTROY
            )),
            "the destroy hint went to the activating player"
        );
    }
}
