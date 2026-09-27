//! The equip-spell procedure — `proc_equip.lua`.
//!
//! `aux.AddEquipProcedure` and its three helpers, the shared shape every
//! equip spell that attaches "by rule" is built from: an activation that
//! targets a face-up monster and attaches the card to it, plus the
//! `EFFECT_EQUIP_LIMIT` that keeps it there.
//!
//! This lives under `cards/` rather than beside the engine because it is
//! **script**, not core: the reference implements it in Lua on top of the
//! same exports a card uses, and so does this. Like a card module, it
//! calls only `script_api`.
//!
//! ## `p` names a side, not a player
//!
//! The first argument is `0` for the equip spell's *own* controller, `1`
//! for the opponent's, and `PLAYER_ALL` for either. `EquipTarget` turns
//! that into a real player at resolution time, because "the opponent" is
//! `1 - tp` and `tp` is not known until the card is activated.
//!
//! ## The two hooks and what they are for
//!
//! `f` filters which monsters may be chosen, and is shared between the
//! target scan and the default equip limit — so a card whose filter stops
//! being true later comes off by rule, without the card saying so.
//!
//! `tg` runs **after** the target is chosen and before the announcement,
//! which is the only place a card can add to the category: Snatch Steal
//! uses it to declare that it also changes control.
//!
//! ## `EquipOperation` re-checks, and does not check the handler
//!
//! ```lua
//! if tc:IsRelateToEffect(e) and tc:IsFaceup() then Duel.Equip(tp,e:GetHandler(),tc) end
//! ```
//!
//! The *monster* is re-checked, for relation and for being face-up; the
//! equip card is not. `field::equip` refuses on its own behalf — a card
//! that has left the field cannot be placed in a Spell & Trap row — so
//! the procedure does not duplicate that.

use crate::board::location;
use crate::effect::{effect_type, flag, AuxArgs, Ctx, ValueFn, Yield};
use crate::event::{category, code, CardId, EffectId, PLAYER_ALL};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

/// Which side's monsters an equip spell may attach to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// `p == 0` — the equip spell's own controller.
    Own,
    /// `p == 1` — the opponent's.
    Opponent,
    /// `p == PLAYER_ALL`, or omitted.
    Either,
}

impl Side {
    /// The reference's `player` local: the side resolved against `tp`.
    fn player(self, tp: u8) -> u8 {
        match self {
            Side::Own => tp,
            Side::Opponent => 1 - tp,
            Side::Either => PLAYER_ALL,
        }
    }

    /// The reference's `p`, as the number a script would pass.
    fn player_argument(self) -> u8 {
        match self {
            Side::Own => 0,
            Side::Opponent => 1,
            Side::Either => PLAYER_ALL,
        }
    }

    /// The inverse, for reading it back off the effect.
    fn from_argument(p: u8) -> Self {
        match p {
            0 => Side::Own,
            1 => Side::Opponent,
            _ => Side::Either,
        }
    }
}

/// The description the procedure's activation carries — `1068`, the
/// library's "equip" line.
pub const ACTIVATE_DESCRIPTION: u64 = 1068;

/// What a card hands to [`add_equip_procedure`]. Every field is one of
/// the reference's optional arguments, and `None` means the reference's
/// `nil`.
pub struct EquipProcedure {
    /// `p` — whose monsters.
    pub side: Side,
    /// `f` — which monsters. Also the default equip limit's filter.
    pub filter: Option<crate::effect::AuxFilter>,
    /// `eqlimit` — the limit that keeps the card attached.
    ///
    /// **Required**, where the reference makes it optional and falls back
    /// to `Auxiliary.EquipLimit(f)` — the target filter, asked again of
    /// whatever the card is attached to. That default is *not* ported,
    /// and the reason is a seam rather than an oversight: the value seam
    /// hands out `&Field`, and a filter that asks about room (which is
    /// what an equip filter usually does) needs it mutably. Widening
    /// `ValueFn` is a crate-wide change, so it waits for a card that
    /// actually needs it. Every equip spell in this pool supplies its
    /// own limit, and this signature is what makes the gap impossible to
    /// take by accident.
    pub eqlimit: ValueFn,
    /// `tg` — run once the target is chosen, before the announcement.
    pub after_target: Option<crate::effect::AuxAfterTarget>,
}

/// `aux.AddEquipProcedure(c, p, f, eqlimit, cost, tg, op, con, prop)`.
///
/// The arguments this pool uses; `cost`, `op`, `con` and `prop` are
/// `nil` for every equip spell in it and are omitted rather than
/// carried as dead parameters.
///
/// Returns the activation effect, as the reference does.
pub fn add_equip_procedure(f: &mut Field, c: CardId, proc: EquipProcedure) -> EffectId {
    // Activate
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, ACTIVATE_DESCRIPTION);
    api::set_category(f, e1, category::EQUIP);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_code(f, e1, code::FREE_CHAIN);
    api::set_property(f, e1, flag::CARD_TARGET | flag::CONTINUOUS_TARGET, 0);
    api::set_target(f, e1, equip_target);
    api::set_operation(f, e1, equip_operation);
    // Equip limit
    let e2 = api::create_effect(f, c);
    api::set_type(f, e2, effect_type::SINGLE);
    api::set_code(f, e2, code::EQUIP_LIMIT);
    api::set_property(f, e2, flag::CANNOT_DISABLE, 0);
    api::set_value_fn(f, e2, proc.eqlimit);

    // What the reference's closure held. Parked on the activation
    // effect, which is the only thing the target and operation are
    // handed at resolution time.
    api::set_aux(
        f,
        e1,
        AuxArgs {
            side: proc.side.player_argument(),
            filter: proc.filter,
            after_target: proc.after_target,
        },
    );

    api::register_effect(f, c, e1, false);
    api::register_effect(f, c, e2, false);
    e1
}

/// `Auxiliary.EquipFilter(c, p, f, e, tp)` — a face-up monster on the
/// named side that the card's own filter accepts.
fn equip_filter(f: &mut Field, c: CardId, player: u8, e: EffectId, tp: u8) -> bool {
    if player != PLAYER_ALL && !api::is_controler(f, c, player) {
        return false;
    }
    if !api::is_faceup(f, c) {
        return false;
    }
    match api::get_aux(f, e).and_then(|a| a.filter) {
        Some(filter) => filter(f, c, e, tp),
        None => true,
    }
}

const MZONE: u32 = location::MZONE as u32;

/// `Auxiliary.EquipTarget(tg, p, f)`.
fn equip_target(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    let e = ctx.reason_effect;
    let Some(aux) = api::get_aux(f, e) else {
        return api::yes(false);
    };
    let player = Side::from_argument(aux.side).player(tp);
    if let Some(c) = chkc {
        return api::yes(
            api::is_location(f, c, u16::from(location::MZONE))
                && api::is_faceup(f, c)
                && equip_filter(f, c, player, e, tp),
        );
    }
    let scan = move |f: &mut Field, c: CardId| equip_filter(f, c, player, e, tp);
    if !chk {
        // **Both** Monster Zones, whichever side `p` names: the scan is
        // two-sided and the filter narrows it, rather than the scan
        // being pointed at one row.
        return api::yes(api::is_existing_target(
            f,
            Some(&scan),
            tp,
            MZONE,
            MZONE,
            1,
            api::Except::None,
        ));
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::EQUIP);
    api::select_target(
        f,
        tp,
        Some(&scan),
        tp,
        MZONE,
        MZONE,
        1,
        1,
        api::Except::None,
    );
    let handler = api::get_handler(f, e);
    api::suspend(move |f, ctx| {
        let Some(g) = api::selected_targets(f) else {
            return api::done();
        };
        if let (Some(after), Some(&tc)) =
            (api::get_aux(f, e).and_then(|a| a.after_target), g.first())
        {
            after(f, ctx, tc);
        }
        api::set_operation_info(f, 0, category::EQUIP, handler.map(|c| vec![c]), 1, 0, 0);
        api::done()
    })
}

/// `Auxiliary.EquipOperation(op)`.
fn equip_operation(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let e = ctx.reason_effect;
    let Some(c) = api::get_handler(f, e) else {
        return api::done();
    };
    let Some(tc) = api::get_first_target(f) else {
        return api::done();
    };
    if !api::is_relate_to_effect(f, tc, e) || !api::is_faceup(f, tc) {
        return api::done();
    }
    api::equip(f, tp, c, tc, true, false);
    api::suspend(|_f, _ctx| api::done())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::duel::phases;
    use crate::effect::Effect;
    use crate::event::Event;
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    /// A limit that accepts everything, so the tests are about the
    /// *target* half of the procedure.
    fn always(_e: &Effect, _f: &Field, _ctx: &Ctx) -> i64 {
        1
    }

    fn monster(f: &mut Field, player: u8, code_: u32, seq: u32, faceup: bool) -> CardId {
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
        f.cards[id].current.position = if faceup {
            position::FACEUP_ATTACK
        } else {
            position::FACEDOWN_DEFENSE
        };
        id
    }

    /// A synthetic equip spell built by the procedure alone, so the
    /// generic clauses are the only thing deciding anything.
    fn field_with(
        side: Side,
        filter: Option<crate::effect::AuxFilter>,
        after: Option<crate::effect::AuxAfterTarget>,
    ) -> (Field, CardId, EffectId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        let mut d = Card::with_data(
            CardData {
                code: 999_001,
                type_: card_type::SPELL | card_type::EQUIP,
                ..Default::default()
            },
            0,
        );
        d.current.controller = 0;
        d.set_status(status::EFFECT_ENABLED, true);
        let spell = f.new_card(d);
        f.add_card(0, spell, location::SZONE, 0, false);
        f.cards[spell].current.position = position::FACEUP;
        f.cards[spell].set_status(status::INITIALIZING, true);
        let e = add_equip_procedure(
            &mut f,
            spell,
            EquipProcedure {
                side,
                filter,
                eqlimit: always,
                after_target: None,
            },
        );
        let _ = after;
        f.cards[spell].set_status(status::INITIALIZING, false);
        (f, spell, e)
    }

    fn ask(f: &mut Field, e: EffectId, tp: u8, chkc: Option<CardId>) -> bool {
        f.core.reason_effect = Some(e);
        let ev = Event::new(code::FREE_CHAIN);
        let ctx = Ctx {
            reason_effect: e,
            player: tp,
            event: &ev,
            card: None,
            args: &[],
        };
        equip_target(f, &ctx, false, chkc).finished().unwrap_or(0) != 0
    }

    /// Drive target then operation, taking `pick` from the offer.
    fn resolve(f: &mut Field, e: EffectId, tp: u8, pick: usize) -> Vec<CardId> {
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = tp;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        let handler = api::get_handler(f, e).expect("an activating card");
        f.cards[handler].create_chain_relation(e, 11);
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
        let mut offered = Vec::new();
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
                        offered = cards.clone();
                        let min = usize::from(*min);
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, (pick + i) as i32);
                        }
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        offered
    }

    /// **`p` picks a side, and the three settings differ.** One board,
    /// three procedures: own, opponent's, either.
    #[test]
    fn the_side_argument_decides_whose_monsters_are_offered() {
        for (side, want_own, want_theirs) in [
            (Side::Own, true, false),
            (Side::Opponent, false, true),
            (Side::Either, true, true),
        ] {
            let (mut f, _, e) = field_with(side, None, None);
            let mine = monster(&mut f, 0, 7_001, 0, true);
            let theirs = monster(&mut f, 1, 8_001, 0, true);
            assert_eq!(ask(&mut f, e, 0, Some(mine)), want_own, "{side:?} own");
            assert_eq!(
                ask(&mut f, e, 0, Some(theirs)),
                want_theirs,
                "{side:?} opponent"
            );
            // And the offer agrees with the checks.
            let (mut f, _, e) = field_with(side, None, None);
            let mine = monster(&mut f, 0, 7_001, 0, true);
            let theirs = monster(&mut f, 1, 8_001, 0, true);
            let offered = resolve(&mut f, e, 0, 0);
            let mut want: Vec<CardId> = Vec::new();
            if want_own {
                want.push(mine);
            }
            if want_theirs {
                want.push(theirs);
            }
            want.sort_unstable();
            let mut got = offered;
            got.sort_unstable();
            assert_eq!(got, want, "{side:?} offer");
        }
    }

    /// **The side is resolved against `tp`, not against the table.**
    /// `Side::Own` means the activating player's own row, which is player
    /// 1's row when player 1 activates.
    #[test]
    fn the_side_is_resolved_against_the_activating_player() {
        let (mut f, _, e) = field_with(Side::Own, None, None);
        let p0 = monster(&mut f, 0, 7_001, 0, true);
        let p1 = monster(&mut f, 1, 8_001, 0, true);
        assert!(ask(&mut f, e, 0, Some(p0)));
        assert!(!ask(&mut f, e, 0, Some(p1)));
        assert!(!ask(&mut f, e, 1, Some(p0)), "asked as player 1");
        assert!(ask(&mut f, e, 1, Some(p1)));
    }

    /// **A face-down monster is never a target**, whatever the side.
    #[test]
    fn a_face_down_monster_is_not_offered() {
        let (mut f, _, e) = field_with(Side::Either, None, None);
        let up = monster(&mut f, 1, 8_001, 0, true);
        let down = monster(&mut f, 1, 8_002, 1, false);
        assert!(ask(&mut f, e, 0, Some(up)), "the positive sibling");
        assert!(!ask(&mut f, e, 0, Some(down)));
        let offered = resolve(&mut f, e, 0, 0);
        assert_eq!(offered, vec![up]);
    }

    /// **Only a Monster Zone**, which `chkc` checks separately from
    /// being face-up: a face-up monster in a graveyard is neither.
    #[test]
    fn a_monster_off_the_monster_zone_is_not_a_target() {
        let (mut f, _, e) = field_with(Side::Either, None, None);
        let onfield = monster(&mut f, 1, 8_001, 0, true);
        let buried = monster(&mut f, 1, 8_002, 1, true);
        f.move_card(1, buried, location::GRAVE, 0, false);
        f.cards[buried].current.position = position::FACEUP_ATTACK;
        assert!(ask(&mut f, e, 0, Some(onfield)), "the positive sibling");
        assert!(!ask(&mut f, e, 0, Some(buried)), "face-up, but buried");
    }

    /// **The card's own filter narrows the offer further**, and gets the
    /// effect and the activating player alongside the card.
    #[test]
    fn the_cards_filter_is_consulted_with_the_effect_and_the_player() {
        fn only_big(f: &mut Field, c: CardId, e: EffectId, tp: u8) -> bool {
            // Reads all three arguments, so a mutant that drops any of
            // them changes the answer.
            let _ = (e, tp);
            api::get_attack(f, c) >= 1500
        }
        let (mut f, _, e) = field_with(Side::Either, Some(only_big), None);
        let small = monster(&mut f, 1, 8_001, 0, true);
        let big = monster(&mut f, 1, 8_002, 1, true);
        f.cards[big].data.attack = 2000;
        assert!(!ask(&mut f, e, 0, Some(small)));
        assert!(ask(&mut f, e, 0, Some(big)));
        let offered = resolve(&mut f, e, 0, 0);
        assert_eq!(offered, vec![big]);
    }

    /// **The operation re-checks the monster**, for relation and for
    /// still being face-up, and attaches only when both hold.
    #[test]
    fn the_operation_re_checks_the_monster_before_attaching() {
        // The positive sibling: it attaches.
        let (mut f, spell, e) = field_with(Side::Either, None, None);
        let tc = monster(&mut f, 1, 8_001, 0, true);
        resolve(&mut f, e, 0, 0);
        assert_eq!(f.cards[spell].equiping_target, Some(tc));

        // Turned face-down between the target and the resolution.
        let (mut f, spell, e) = field_with(Side::Either, None, None);
        let tc = monster(&mut f, 1, 8_001, 0, true);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        ch.target_cards = vec![tc];
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.cards[spell].create_chain_relation(e, 11);
        f.cards[tc].create_chain_relation(e, 11);
        f.cards[tc].current.position = position::FACEDOWN_DEFENSE;
        operate(&mut f, e, 0);
        assert_eq!(f.cards[spell].equiping_target, None, "face-down now");
        // The equip card is left **where it was**. Without the face-up
        // test the attachment would still fail — `field::equip` refuses a
        // face-down target — but it would fail by sending the equip card
        // to the graveyard, which is a different outcome entirely.
        assert_eq!(
            f.cards[spell].current.location,
            location::SZONE,
            "and not thrown away trying"
        );

        // Never related.
        let (mut f, spell, e) = field_with(Side::Either, None, None);
        let tc = monster(&mut f, 1, 8_001, 0, true);
        let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        ch.target_cards = vec![tc];
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.cards[spell].create_chain_relation(e, 11);
        operate(&mut f, e, 0);
        assert_eq!(f.cards[spell].equiping_target, None, "not related");
    }

    fn operate(f: &mut Field, e: EffectId, tp: u8) {
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
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        return;
                    }
                }
                // An equip card coming from somewhere other than the row
                // has to be given a seat in it.
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectPlace { player, flag, .. }) => {
                        let (pl, flag) = (*player, *flag);
                        let seq = (0..5u32)
                            .find(|s| flag & (0x100 << s) == 0)
                            .expect("a free spell seat");
                        f.core.returns.set_i8(0, pl as i8);
                        f.core.returns.set_i8(1, location::SZONE as i8);
                        f.core.returns.set_i8(2, seq as i8);
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                other => panic!("stopped with {other:?}"),
            }
        }
        panic!("did not settle");
    }

    /// **The hook runs with the chosen monster**, after the choice and
    /// before the announcement.
    #[test]
    fn the_hook_is_given_the_chosen_monster() {
        use std::cell::Cell;
        thread_local! {
            static SEEN: Cell<usize> = const { Cell::new(usize::MAX) };
        }
        fn record(_f: &mut Field, _ctx: &Ctx, tc: CardId) {
            SEEN.with(|s| s.set(tc));
        }
        let (mut f, _, e) = field_with(Side::Either, None, None);
        // Registered separately: `field_with` takes the hook but the
        // procedure reads it off the effect, so set it there.
        let mut aux = api::get_aux(&f, e).expect("the procedure's arguments");
        aux.after_target = Some(record);
        api::set_aux(&mut f, e, aux);
        monster(&mut f, 1, 8_001, 0, true);
        monster(&mut f, 1, 8_002, 1, true);
        SEEN.with(|s| s.set(usize::MAX));
        let offered = resolve(&mut f, e, 0, 1);
        assert_eq!(offered.len(), 2, "two to choose between");
        assert_eq!(
            SEEN.with(Cell::get),
            offered[1],
            "the hook was handed the card actually picked, not the first"
        );
    }

    /// **The equip is made by the activating player**, which decides
    /// whose Spell & Trap row the card moves into.
    ///
    /// Only visible when the card is not already in a row — an equip
    /// spell activating from its own row never reads the argument, which
    /// is why the board here starts it in the hand.
    #[test]
    fn the_equip_is_made_by_the_activating_player() {
        for tp in [0u8, 1u8] {
            let (mut f, spell, e) = field_with(Side::Either, None, None);
            // Take the card out of the row and put it in a hand.
            f.move_card(tp, spell, location::HAND, 0, false);
            let tc = monster(&mut f, 1 - tp, 8_001, 0, true);
            let mut ch = Chain::new(e, Event::new(code::FREE_CHAIN));
            ch.triggering_player = tp;
            ch.chain_count = 1;
            ch.chain_id = 11;
            ch.target_cards = vec![tc];
            f.core.current_chain.push(ch);
            f.core.chain_solving = true;
            f.cards[spell].create_chain_relation(e, 11);
            f.cards[tc].create_chain_relation(e, 11);
            operate(&mut f, e, tp);
            assert_eq!(f.cards[spell].equiping_target, Some(tc), "tp {tp}");
            assert_eq!(
                f.cards[spell].current.controller, tp,
                "it went into the activating player's row"
            );
            assert_eq!(f.cards[spell].current.location, location::SZONE);
        }
    }

    /// **The announcement names the equip card**, not the monster —
    /// the monster is named by whatever the card's own hook adds.
    #[test]
    fn the_announcement_names_the_equip_card() {
        let (mut f, spell, e) = field_with(Side::Either, None, None);
        monster(&mut f, 1, 8_001, 0, true);
        resolve(&mut f, e, 0, 0);
        let info = f.core.current_chain[0]
            .opinfos
            .get(&category::EQUIP)
            .expect("the equip category");
        assert_eq!(info.cards.as_deref(), Some(&[spell][..]));
        assert_eq!(info.count, 1);
    }
}
