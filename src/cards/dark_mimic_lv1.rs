//! Dark Mimic LV1 — `c74713516.lua`.
//!
//! The thirty-eighth card, and the pool's first **LV monster**: a flip
//! effect that draws, and a Standby Phase trigger that trades itself for
//! the next rung of the ladder.
//!
//! ## The draw travels on the chain link
//!
//! ```lua
//! Duel.SetTargetPlayer(tp)  Duel.SetTargetParam(1)
//! ...
//! local p,d=Duel.GetChainInfo(0,CHAININFO_TARGET_PLAYER,CHAININFO_TARGET_PARAM)
//! ```
//!
//! The same indirection Delinquent Duo and Snatch Steal use, and for the
//! same reason: `EFFECT_FLAG_PLAYER_TARGET` means the announcement names
//! a player, and the resolution draws for whoever was named rather than
//! for `tp`. On this card they are the same player; the indirection is
//! still the script's.
//!
//! ## The second effect trades the card in
//!
//! `Cost.SelfToGrave` (`utility.lua:1434`) is the cost — the card sends
//! **itself** to the graveyard to pay — and the operation then special
//! summons Dark Mimic LV3 from the hand or deck.
//!
//! Two details in that summon:
//!
//! - **`nocheck` and `nolimit` are both true**, so LV3 arrives ignoring
//!   its own summoning conditions. That is what an LV ladder is: the
//!   higher rung cannot normally be summoned at all.
//! - **`CompleteProcedure` afterwards**, which sets
//!   `STATUS_PROC_COMPLETE`. A card that arrived by having its conditions
//!   waived has not "properly" been summoned, and without this it could
//!   not later be revived from the graveyard. The call is the card saying
//!   that this counted.
//!
//! ## `GetMZoneCount` excludes the card itself
//!
//! ```lua
//! Duel.GetMZoneCount(tp,e:GetHandler())>0
//! ```
//!
//! Not `GetLocationCount`. The Mimic is *about to leave* — it is the cost
//! — so the seat it occupies is one the summon may use, and counting
//! without excluding itself would refuse the trade on a full field. The
//! operation then asks the ordinary `GetLocationCount`, by which time the
//! cost has been paid and the seat really is free.
//!
//! ## A branch this pool cannot reach
//!
//! **Dark Mimic LV3 (`1102515`) is not in the card pool**, so
//! `Card.IsCode(1102515)` is false for every card and `s.sptg` always
//! refuses. The second effect is therefore never offered — not for want
//! of a policy that would take it, but because the card it names does not
//! exist here. Transcribed in full and covered by unit tests.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::{location, position};
use crate::card::{reason, summon_type};
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId, EffectId};
use crate::field::Field;
use crate::host_question::{hint, hintmsg};
use crate::script_api as api;

pub const CODE: u32 = 74_713_516;

/// `s.listed_names` — the card this one goes looking for.
pub const LISTED_NAMES: [u32; 1] = [1_102_515];

/// `s.LVnum` and `s.LVset` — which rung of which ladder this is.
/// `SET_DARK_MIMIC` is `archetype_setcode_constants.lua:152`.
pub const LV_NUM: u32 = 1;
pub const LV_SET: u32 = 0x5e;

/// How many cards the flip draws.
const DRAW_COUNT: i32 = 1;

const HAND_OR_DECK: u32 = (location::HAND | location::DECK) as u32;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // FLIP: Draw 1 card
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::DRAW);
    api::set_type(f, e1, effect_type::SINGLE | effect_type::FLIP);
    api::set_property(f, e1, flag::PLAYER_TARGET, 0);
    api::set_target(f, e1, drtg);
    api::set_operation(f, e1, drop_);
    api::register_effect(f, c, e1, false);
    // Special Summon 1 "Dark Mimic LV3" from your hand or Deck
    let e2 = api::create_effect(f, c);
    api::set_description(f, e2, api::stringid(CODE, 1));
    api::set_category(f, e2, category::SPECIAL_SUMMON);
    api::set_type(f, e2, effect_type::FIELD | effect_type::TRIGGER_O);
    api::set_code(f, e2, code::PHASE | u32::from(crate::duel::phases::STANDBY));
    api::set_range(f, e2, u16::from(location::MZONE));
    api::set_condition(f, e2, on_its_own_turn);
    api::set_cost(f, e2, self_to_grave);
    api::set_target(f, e2, sptg);
    api::set_operation(f, e2, spop);
    api::register_effect(f, c, e2, false);
}

fn drtg(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    if !chk {
        return api::yes(true);
    }
    api::set_target_player(f, tp);
    api::set_target_param(f, DRAW_COUNT);
    api::set_operation_info(f, 0, category::DRAW, None, 0, tp, DRAW_COUNT);
    api::yes(true)
}

fn drop_(f: &mut Field, _ctx: &Ctx) -> Yield {
    // The player and the count come off the chain link, not from `tp`.
    let Some((p, d)) = api::get_chain(f, 0).map(|ch| (ch.target_player, ch.target_param)) else {
        return api::done();
    };
    api::draw(f, p, d as u32, reason::EFFECT);
    api::suspend(|_f, _ctx| api::done())
}

/// `function(e,tp) return Duel.IsTurnPlayer(tp) end` — only on its own
/// Standby Phase.
fn on_its_own_turn(f: &mut Field, ctx: &Ctx) -> bool {
    api::is_turn_player(f, ctx.player)
}

/// `Cost.SelfToGrave` (`utility.lua:1434`) — the card pays with itself.
fn self_to_grave(f: &mut Field, ctx: &Ctx, chk: bool) -> bool {
    let Some(c) = api::get_handler(f, ctx.reason_effect) else {
        return false;
    };
    if !chk {
        return api::is_able_to_grave_as_cost(f, c);
    }
    api::send_to_grave(f, vec![c], reason::COST);
    true
}

/// `s.spfilter` — the next rung, summonable by this effect with its own
/// conditions waived.
fn spfilter(e: EffectId, tp: u8) -> impl Fn(&mut Field, CardId) -> bool {
    move |f: &mut Field, c: CardId| {
        api::is_code(f, c, LISTED_NAMES[0])
            && api::is_can_be_special_summoned(f, c, e, summon_type::SPECIAL, tp, true, true)
    }
}

fn sptg(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let tp = ctx.player;
    let e = ctx.reason_effect;
    if !chk {
        // **Counting without itself**: the Mimic is the cost, so its own
        // seat is one the summon may use.
        let handler = api::get_handler(f, e);
        let without = handler.map_or(api::Except::None, api::Except::Card);
        let sp = spfilter(e, tp);
        return api::yes(
            api::get_mzone_count(f, tp, without, None, None) > 0
                && api::is_existing_matching_card(
                    f,
                    Some(&sp),
                    tp,
                    HAND_OR_DECK,
                    0,
                    1,
                    api::Except::None,
                ),
        );
    }
    // The announcement names where it will come from, in the parameter
    // slot — a location rather than a count.
    api::set_operation_info(
        f,
        0,
        category::SPECIAL_SUMMON,
        None,
        1,
        tp,
        HAND_OR_DECK as i32,
    );
    api::yes(true)
}

fn spop(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let e = ctx.reason_effect;
    // The ordinary count now: the cost has been paid, so the seat is
    // genuinely free and there is nothing to exclude.
    if api::get_location_count(f, tp, location::MZONE) <= 0 {
        return api::done();
    }
    api::hint(f, hint::SELECTMSG, tp, hintmsg::SPSUMMON);
    let sp = spfilter(e, tp);
    api::select_matching_card(
        f,
        tp,
        Some(&sp),
        tp,
        HAND_OR_DECK,
        0,
        1,
        1,
        api::Except::None,
    );
    api::suspend(move |f, _ctx| {
        let chosen = api::group_selected(f);
        let Some(&sc) = chosen.first() else {
            return api::done();
        };
        api::special_summon(
            f,
            vec![sc],
            summon_type::SPECIAL,
            tp,
            tp,
            true,
            true,
            position::FACEUP,
        );
        api::suspend(move |f, _ctx| {
            if api::resumed_value(f) > 0 {
                // It arrived with its conditions waived; this is the card
                // saying that counted as a proper summon.
                api::complete_procedure(f, sc);
            }
            api::done()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::chain::Chain;
    use crate::event::Event;
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    /// Dark Mimic LV1 face-up in `tp`'s first Monster Zone, on `tp`'s turn.
    fn field_as(tp: u8) -> (Field, CardId) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = tp;
        f.infos.phase = crate::duel::phases::STANDBY;
        let mut d = Card::with_data(crate::cards::card_data(CODE).expect("printed data"), tp);
        d.current.controller = tp;
        d.set_status(status::EFFECT_ENABLED, true);
        let m = f.new_card(d);
        f.add_card(tp, m, location::MZONE, 0, false);
        f.cards[m].current.position = position::FACEUP_ATTACK;
        f.initialize_card(m);
        f.apply_field_effect(m);
        (f, m)
    }

    fn field() -> (Field, CardId) {
        field_as(0)
    }

    fn flip_effect(f: &Field, m: CardId) -> EffectId {
        f.cards[m].single_effect.equal_range(code::FLIP)[0]
    }
    fn ladder_effect(f: &Field, m: CardId) -> EffectId {
        f.cards[m]
            .field_effect
            .equal_range(code::PHASE | u32::from(crate::duel::phases::STANDBY))[0]
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

    /// Put a card with `code_` in `player`'s hand.
    fn in_hand(f: &mut Field, player: u8, code_: u32, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER | card_type::EFFECT,
                level: 3,
                attack: 300,
                defense: 1300,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(player, id, location::HAND, seq, false);
        id
    }

    fn plain_monster(f: &mut Field, player: u8, code_: u32, seq: u32) -> CardId {
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
        f.cards[id].current.position = position::FACEUP_ATTACK;
        id
    }

    /// **Two printed effects**: a flip that draws for a named player, and
    /// an optional Standby Phase trigger that costs the card itself.
    #[test]
    fn the_script_registers_a_flip_draw_and_a_ladder_trigger() {
        let (f, m) = field();
        let e1 = f.effects.get(flip_effect(&f, m)).unwrap();
        assert!(e1.is_type(effect_type::FLIP), "a flip effect");
        assert_eq!(e1.code, code::FLIP, "SetType fixes the code");
        assert_eq!(e1.category, category::DRAW);
        assert!(e1.is_flag(flag::PLAYER_TARGET), "it names a player");
        assert!(!e1.is_flag(flag::CARD_TARGET));

        let e2 = f.effects.get(ladder_effect(&f, m)).unwrap();
        assert!(e2.is_type(effect_type::FIELD));
        assert!(e2.is_type(effect_type::TRIGGER_O), "optional");
        assert_eq!(e2.category, category::SPECIAL_SUMMON);
        assert_eq!(e2.range, u16::from(location::MZONE));
        assert!(e2.condition.is_some());
        assert!(e2.cost.is_some());
    }

    /// **The declarations the script carries**, pinned as literals so a
    /// later reader finds them stated rather than inferred.
    #[test]
    fn the_listed_name_and_the_ladder_rung_are_what_the_script_says() {
        assert_eq!(LISTED_NAMES, [1_102_515], "Dark Mimic LV3");
        assert_eq!(LV_NUM, 1);
        assert_eq!(LV_SET, 0x5e, "SET_DARK_MIMIC");
    }

    /// **The draw is announced for a named player and a count of one, and
    /// the resolution reads both back off the chain link.**
    ///
    /// Driven as player 1 as well, because a suite that only activates as
    /// player 0 cannot tell `tp` from a literal zero.
    #[test]
    fn the_draw_travels_on_the_chain_link() {
        for tp in [0u8, 1u8] {
            let (mut f, m) = field_as(tp);
            let e = flip_effect(&f, m);
            let mut ch = Chain::new(e, Event::new(code::FLIP));
            ch.triggering_player = tp;
            ch.chain_count = 1;
            f.core.current_chain.push(ch);
            let ev = Event::new(code::FLIP);
            drtg(&mut f, &ctx_for(e, &ev, tp), true, None);
            assert_eq!(f.core.current_chain[0].target_player, tp);
            assert_eq!(f.core.current_chain[0].target_param, 1);
            let info = f.core.current_chain[0]
                .opinfos
                .get(&category::DRAW)
                .expect("the draw category");
            assert_eq!(info.player, tp);
            assert_eq!(info.param, 1);

            // The resolution draws for whoever the link names — which is
            // set here to the *other* player, so reading `tp` instead
            // would draw for the wrong one.
            f.core.current_chain[0].target_player = 1 - tp;
            drop_(&mut f, &ctx_for(e, &ev, tp));
            match &f.core.subunits[0].kind {
                Kind::Draw {
                    playerid, count, ..
                } => {
                    assert_eq!(*playerid, 1 - tp, "the player the link named");
                    assert_eq!(*count, 1);
                }
                other => panic!("expected a Draw: {other:?}"),
            }
        }
    }

    /// **The ladder trigger fires only on its controller's own Standby
    /// Phase.**
    #[test]
    fn the_ladder_trigger_wants_its_own_turn() {
        let (mut f, m) = field();
        let e = ladder_effect(&f, m);
        let ev = Event::new(code::PHASE | u32::from(crate::duel::phases::STANDBY));
        f.infos.turn_player = 0;
        assert!(on_its_own_turn(&mut f, &ctx_for(e, &ev, 0)));
        f.infos.turn_player = 1;
        assert!(!on_its_own_turn(&mut f, &ctx_for(e, &ev, 0)));
    }

    /// **The cost is the card itself, checked before it is paid.**
    #[test]
    fn the_cost_sends_the_card_to_the_graveyard() {
        let (mut f, m) = field();
        let e = ladder_effect(&f, m);
        let ev = Event::new(0);
        let ctx = ctx_for(e, &ev, 0);
        assert!(self_to_grave(&mut f, &ctx, false), "it can pay");

        assert!(self_to_grave(&mut f, &ctx, true));
        match &f.core.subunits[0].kind {
            Kind::SendTo {
                targets,
                reason: why,
                ..
            } => {
                assert_eq!(
                    f.group(*targets).iter().copied().collect::<Vec<_>>(),
                    vec![m],
                    "itself"
                );
                assert_ne!(why & reason::COST, 0, "as a cost");
            }
            other => panic!("expected a SendTo: {other:?}"),
        }

        // A card that cannot be sent to a graveyard as a cost cannot pay.
        let (mut f, m) = field();
        let mut stop =
            crate::effect::Effect::new(effect_type::SINGLE, code::CANNOT_TO_GRAVE_AS_COST);
        stop.owner = Some(m);
        stop.handler = Some(m);
        let id = f.new_effect(stop);
        f.cards[m]
            .single_effect
            .insert(code::CANNOT_TO_GRAVE_AS_COST, id);
        f.cards[m].indexer.insert(id);
        let e = ladder_effect(&f, m);
        assert!(!self_to_grave(&mut f, &ctx_for(e, &ev, 0), false));
    }

    /// **The search wants Dark Mimic LV3 and nothing else** — and that
    /// card is not in this pool, which is why the whole second effect is
    /// never offered in a game.
    #[test]
    fn the_search_wants_the_next_rung_and_this_pool_has_none() {
        let (mut f, m) = field();
        let e = ladder_effect(&f, m);
        let sp = spfilter(e, 0);
        // Something else in the hand is not it.
        let other = in_hand(&mut f, 0, 9_001, 0);
        assert!(!sp(&mut f, other));
        // The named card is.
        let lv3 = in_hand(&mut f, 0, LISTED_NAMES[0], 1);
        assert!(sp(&mut f, lv3));
        // And the pool does not contain it, so nothing in a real duel can
        // match — the branch is structurally unreachable here.
        assert!(
            crate::cards::card_data(LISTED_NAMES[0]).is_none(),
            "Dark Mimic LV3 is not in the pool"
        );

        // The right name is not enough: the card must also be
        // summonable. One already on the field is not.
        let onfield = plain_monster(&mut f, 0, LISTED_NAMES[0], 1);
        assert!(api::is_code(&mut f, onfield, LISTED_NAMES[0]));
        assert!(!sp(&mut f, onfield), "already on the field");

        // And the conditions are **waived**: a card under a revive limit
        // that has not completed its procedure is refused from a hand
        // with the checks on, and accepted with them off.
        let limited = in_hand(&mut f, 0, LISTED_NAMES[0], 2);
        let mut lim = crate::effect::Effect::new(effect_type::SINGLE, code::REVIVE_LIMIT);
        lim.owner = Some(limited);
        lim.handler = Some(limited);
        let id = f.new_effect(lim);
        f.cards[limited]
            .single_effect
            .insert(code::REVIVE_LIMIT, id);
        f.cards[limited].indexer.insert(id);
        assert!(sp(&mut f, limited), "the waiver lets it through");
        assert!(
            !api::is_can_be_special_summoned(
                &mut f,
                limited,
                e,
                summon_type::SPECIAL,
                0,
                false,
                false
            ),
            "and with the checks on it would not be"
        );
    }

    /// **The seat count excludes the Mimic itself**, because it is the
    /// cost and is about to leave.
    ///
    /// With a full row and the Mimic in it, counting *with* itself
    /// answers zero and refuses the trade; counting without it answers
    /// one and allows it. That difference is the whole reason the script
    /// calls `GetMZoneCount` rather than `GetLocationCount`.
    #[test]
    fn the_seat_count_leaves_out_the_card_that_is_paying() {
        let (mut f, m) = field();
        in_hand(&mut f, 0, LISTED_NAMES[0], 0);
        for seat in 1..5 {
            plain_monster(&mut f, 0, 9_100 + seat, seat);
        }
        assert_eq!(
            api::get_location_count(&mut f, 0, location::MZONE),
            0,
            "the row is full"
        );
        assert_eq!(
            api::get_mzone_count(&mut f, 0, api::Except::Card(m), None, None),
            1,
            "but not once the Mimic is taken out of it"
        );
        let e = ladder_effect(&f, m);
        let ev = Event::new(0);
        f.core.reason_effect = Some(e);
        assert!(
            sptg(&mut f, &ctx_for(e, &ev, 0), false, None)
                .finished()
                .unwrap_or(0)
                != 0,
            "so the trade is offered"
        );
    }

    /// **And it still wants something to fetch.** The same full board
    /// with an empty hand refuses.
    #[test]
    fn it_wants_a_card_to_fetch() {
        let (mut f, m) = field();
        let e = ladder_effect(&f, m);
        let ev = Event::new(0);
        f.core.reason_effect = Some(e);
        assert!(
            sptg(&mut f, &ctx_for(e, &ev, 0), false, None)
                .finished()
                .unwrap_or(0)
                == 0,
            "nothing to fetch"
        );
        in_hand(&mut f, 0, LISTED_NAMES[0], 0);
        assert!(
            sptg(&mut f, &ctx_for(e, &ev, 0), false, None)
                .finished()
                .unwrap_or(0)
                != 0
        );
    }

    /// **The announcement names the hand and deck**, in the parameter
    /// slot — a location rather than a count.
    #[test]
    fn the_announcement_names_where_it_comes_from() {
        let (mut f, m) = field();
        let e = ladder_effect(&f, m);
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        f.core.current_chain.push(ch);
        let ev = Event::new(0);
        sptg(&mut f, &ctx_for(e, &ev, 0), true, None);
        let info = f.core.current_chain[0]
            .opinfos
            .get(&category::SPECIAL_SUMMON)
            .expect("the summon category");
        assert_eq!(info.count, 1);
        assert_eq!(info.player, 0);
        assert_eq!(
            info.param,
            (location::HAND | location::DECK) as i32,
            "a hand and a deck, not a count"
        );
    }

    /// **It summons the fetched card with its conditions waived, and then
    /// says that counted** — `STATUS_PROC_COMPLETE`, without which the
    /// card could not later be revived.
    #[test]
    fn it_summons_the_next_rung_and_completes_its_procedure() {
        let (mut f, m) = field();
        let lv3 = in_hand(&mut f, 0, LISTED_NAMES[0], 0);
        // The Mimic has paid: take it off the field as the cost would.
        f.move_card(0, m, location::GRAVE, 0, false);
        let e = ladder_effect(&f, m);
        assert!(!f.cards[lv3].is_status(status::PROC_COMPLETE));
        // Through `ExecuteOperation`, not by calling `spop`: the
        // operation **suspends** to read the selection back, and a direct
        // call discards the continuation along with the summon.
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.core.sub_solving_event.push_back(Event::new(0));
        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player: 0,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        drive(&mut f);
        assert_eq!(f.cards[lv3].current.location, location::MZONE);
        assert_eq!(f.cards[lv3].current.controller, 0, "its own side");
        assert!(
            f.cards[lv3].is_status(status::PROC_COMPLETE),
            "the procedure was completed"
        );
        assert!(
            f.messages
                .iter()
                .any(|m| matches!(m, Message::Hint { kind, player, value }
                if *kind == hint::SELECTMSG && *player == 0 && *value == hintmsg::SPSUMMON)),
            "prefaced by the special-summon prompt"
        );
    }

    /// **The summon itself waives the conditions too.** A fetched card
    /// under a revive limit arrives only because `nocheck` and `nolimit`
    /// are both true — with them off the summon is refused and the card
    /// stays in the hand.
    #[test]
    fn the_summon_waives_the_fetched_cards_conditions() {
        let (mut f, m) = field();
        let lv3 = in_hand(&mut f, 0, LISTED_NAMES[0], 0);
        let mut lim = crate::effect::Effect::new(effect_type::SINGLE, code::REVIVE_LIMIT);
        lim.owner = Some(lv3);
        lim.handler = Some(lv3);
        let id = f.new_effect(lim);
        f.cards[lv3].single_effect.insert(code::REVIVE_LIMIT, id);
        f.cards[lv3].indexer.insert(id);
        f.move_card(0, m, location::GRAVE, 0, false);

        let e = ladder_effect(&f, m);
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        ch.chain_id = 11;
        f.core.current_chain.push(ch);
        f.core.chain_solving = true;
        f.core.sub_solving_event.push_back(Event::new(0));
        f.emplace(Kind::ExecuteOperation {
            resume: None,
            effect: e,
            player: 0,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        drive(&mut f);
        assert_eq!(
            f.cards[lv3].current.location,
            location::MZONE,
            "the waiver carried it through the summon as well"
        );
    }

    /// **A full row at resolution time stops it**, and the count asked
    /// then is the ordinary one — the cost has been paid, so there is
    /// nothing left to exclude.
    #[test]
    fn a_full_row_at_resolution_stops_the_summon() {
        let (mut f, m) = field();
        let lv3 = in_hand(&mut f, 0, LISTED_NAMES[0], 0);
        for seat in 1..5 {
            plain_monster(&mut f, 0, 9_100 + seat, seat);
        }
        // The Mimic is still on the field, so the row is genuinely full.
        let e = ladder_effect(&f, m);
        f.core.reason_effect = Some(e);
        let ev = Event::new(0);
        spop(&mut f, &ctx_for(e, &ev, 0));
        assert!(f.core.subunits.is_empty(), "nothing queued");
        assert_eq!(f.cards[lv3].current.location, location::HAND);
    }

    fn drive(f: &mut Field) {
        for _ in 0..4096 {
            match f.process() {
                Status::Continue => {
                    if f.core.units.is_empty() && f.core.subunits.is_empty() {
                        return;
                    }
                }
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectCard { min, .. }) => {
                        let min = usize::from(*min);
                        f.core.return_cards.clear();
                        f.core.returns.set_i32(0, 0);
                        f.core.returns.set_i32(1, min as i32);
                        for i in 0..min {
                            f.core.returns.set_i32(i + 2, i as i32);
                        }
                    }
                    Some(Message::SelectPlace { player, flag, .. }) => {
                        let (pl, flag) = (*player, *flag);
                        let seq = (0..5u32).find(|s| flag & (1 << s) == 0).unwrap_or(0);
                        f.core.returns.set_i8(0, pl as i8);
                        f.core.returns.set_i8(1, location::MZONE as i8);
                        f.core.returns.set_i8(2, seq as i8);
                    }
                    Some(Message::SelectPosition { .. }) => {
                        f.core.returns.set(i32::from(position::FACEUP_ATTACK))
                    }
                    other => panic!("unexpected question: {other:?}"),
                },
                Status::End => return,
            }
        }
        panic!("did not settle");
    }
}
