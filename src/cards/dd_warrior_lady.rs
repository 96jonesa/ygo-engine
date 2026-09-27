//! D.D. Warrior Lady — `c7572887.lua`.
//!
//! The first **optional** trigger, and the first card to read the battle
//! it is in. `EFFECT_TYPE_SINGLE + EFFECT_TYPE_TRIGGER_O` on
//! `EVENT_BATTLED`: optional means it is offered in a response window
//! rather than chaining itself, which is the difference from Airknight
//! Parshath's forced one.
//!
//! Both combatants are candidates, and the card is deliberately written
//! to work from either seat — it banishes itself along with whatever it
//! fought, whether it attacked or was attacked. That is why `target`
//! asks the question twice, once each way round.
//!
//! ## `IsRelateToBattle` is an identity test
//!
//! Not "took part in this battle" but "**is still the same card** that
//! did": the check compares the card's reset id against the two the field
//! recorded when the battle began, and a card that left and came back has
//! a new one. `target` records only the combatants that still pass it,
//! and `operation` filters again at resolution — the same re-read
//! discipline Dark Hole has, for the same reason.
//!
//! ## A direct attack has no target
//!
//! `Duel.GetAttackTarget()` is `nil` then, and the script's `t~=nil`
//! guards say so. Here that is an `Option`, and dropping the check would
//! mean a direct attack banishes nothing rather than panicking — a
//! silent wrong answer, which is why the test drives that case.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::position;
use crate::card::reason;
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 7_572_887;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // remove
    let e1 = api::create_effect(f, c);
    api::set_description(f, e1, api::stringid(CODE, 0));
    api::set_category(f, e1, category::REMOVE);
    api::set_type(f, e1, effect_type::SINGLE | effect_type::TRIGGER_O);
    api::set_code(f, e1, code::BATTLED);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, operation);
    api::register_effect(f, c, e1, false);
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, _chkc: Option<CardId>) -> Yield {
    let handler = api::get_handler(f, ctx.reason_effect);
    let a = api::get_attacker(f);
    let t = api::get_attack_target(f);
    if !chk {
        // Either seat: it was attacked and the attacker can go, or it
        // attacked and there was something there that can.
        let was_attacked =
            t == handler && a.is_some_and(|a| api::is_able_to_remove(f, a, None, None, None));
        let attacked =
            a == handler && t.is_some_and(|t| api::is_able_to_remove(f, t, None, None, None));
        return api::yes(was_attacked || attacked);
    }
    let g = still_the_cards_that_fought(f, a, t);
    let n = g.len() as u8;
    api::set_operation_info(f, 0, category::REMOVE, Some(g), n, 0, 0);
    api::yes(true)
}

fn operation(f: &mut Field, _ctx: &Ctx) -> Yield {
    let a = api::get_attacker(f);
    let d = api::get_attack_target(f);
    let rg = still_the_cards_that_fought(f, a, d);
    api::remove(f, rg, position::FACEUP, reason::EFFECT);
    Yield::Done(0)
}

/// The combatants that are still the cards that fought, in attacker-then-
/// defender order.
///
/// The script spells this out twice — once building the group to record
/// and once building the group to banish — as `g:AddCard` guarded by
/// `IsRelateToBattle`, then `g:Filter(Card.IsRelateToBattle, nil)`. The
/// two are the same question asked at two different times, and the
/// re-read is the point: a combatant that left and came back between
/// activation and resolution is dropped from the second. Written once
/// here so the two cannot drift apart, and called at both moments so the
/// re-read still happens.
fn still_the_cards_that_fought(f: &Field, a: Option<CardId>, t: Option<CardId>) -> Vec<CardId> {
    [a, t]
        .into_iter()
        .flatten()
        .filter(|&c| api::is_relate_to_battle(f, c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, status, Card, CardData};
    use crate::duel::phases;
    use crate::effect::flag;
    use crate::field::Message;
    use crate::processor::{Kind, Status};

    /// `target`, asked as a plain yes/no. It answers through `returns`
    /// like any card function, so a test reads the finished value rather
    /// than the `Yield`.
    fn asks(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> bool {
        target(f, ctx, chk, chkc).finished().unwrap_or(0) != 0
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

    fn vanilla(code_: u32, atk: i32) -> CardData {
        CardData {
            code: code_,
            type_: card_type::MONSTER | card_type::NORMAL,
            level: 4,
            attack: atk,
            defense: atk,
            ..Default::default()
        }
    }

    /// Player 0's Battle Step with D.D. Warrior Lady ready to attack, and
    /// `theirs` a monster of player 1's to attack into, if any.
    fn field(theirs: Option<i32>) -> (Field, CardId, Option<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 4;
        f.infos.turn_player = 0;
        f.infos.phase = phases::BATTLE_STEP;
        let dd = put(
            &mut f,
            0,
            crate::cards::card_data(CODE).expect("printed data"),
            location::MZONE,
            0,
        );
        f.cards[dd].current.position = crate::position::FACEUP_ATTACK;
        f.initialize_card(dd);
        let t = theirs.map(|atk| {
            let t = put(&mut f, 1, vanilla(2000, atk), location::MZONE, 0);
            f.cards[t].current.position = crate::position::FACEUP_ATTACK;
            t
        });
        (f, dd, t)
    }

    /// **`initial_effect` registers one optional trigger** on
    /// `EVENT_BATTLED`, with the remove category and a description —
    /// optional because it is offered in a window rather than chaining
    /// itself.
    #[test]
    fn the_script_registers_an_optional_trigger() {
        let (f, dd, _) = field(None);
        let ids = f.cards[dd].single_effect.equal_range(code::BATTLED);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::SINGLE));
        assert!(e.is_type(effect_type::TRIGGER_O), "offered, not forced");
        assert!(
            !e.is_type(effect_type::TRIGGER_F),
            "a forced trigger would chain itself"
        );
        assert!(e.is_flag(flag::INITIAL));
        assert_eq!(e.category, category::REMOVE);
        assert_eq!(e.description, api::stringid(CODE, 0));
    }

    /// **`IsRelateToBattle` is an identity test**, not a membership one:
    /// a card whose reset id no longer matches what the field recorded
    /// when the battle began has left and come back, and fails it.
    #[test]
    fn relate_to_battle_compares_the_recorded_reset_ids() {
        let (mut f, dd, t) = field(Some(1500));
        let t = t.unwrap();
        f.core.pre_field = [f.cards[dd].fieldid_r, f.cards[t].fieldid_r];
        assert!(api::is_relate_to_battle(&f, dd));
        assert!(api::is_relate_to_battle(&f, t));
        // It left and came back: same card, new reset id.
        let fresh = f.next_field_id_raw();
        f.cards[t].fieldid_r = fresh;
        assert!(
            !api::is_relate_to_battle(&f, t),
            "no longer the card that fought"
        );
    }

    /// **Attacking a monster banishes both**, face-up and by effect, once
    /// the window is taken.
    #[test]
    fn attacking_a_monster_banishes_both() {
        let (mut f, dd, t) = field(Some(1500));
        let t = t.unwrap();
        let offered = run_battle(&mut f);
        assert!(offered > 0, "the optional trigger was offered");
        for c in [dd, t] {
            assert_eq!(
                f.cards[c].current.location,
                location::REMOVED,
                "banished, not destroyed"
            );
            assert!(f.cards[c].current.position & crate::position::FACEUP != 0);
            assert!(f.cards[c].reason & reason::EFFECT != 0);
        }
        let rec = recorded_remove().expect("a REMOVE operation was recorded");
        let mut want = vec![dd, t];
        want.sort_unstable();
        let mut got = rec.cards.clone().unwrap_or_default();
        got.sort_unstable();
        assert_eq!(got, want, "both combatants");
        assert_eq!(rec.count, 2);
    }

    /// **It works from the other seat too.** Attacked rather than
    /// attacking, the card still offers itself and still banishes both —
    /// the `chk == 0` question asks twice, once each way round.
    #[test]
    fn being_attacked_banishes_both_as_well() {
        let (mut f, dd, _) = field(None);
        // Player 1's turn, and its monster attacks into D.D.
        f.infos.turn_player = 1;
        let a = put(&mut f, 1, vanilla(2000, 1200), location::MZONE, 0);
        f.cards[a].current.position = crate::position::FACEUP_ATTACK;
        let offered = run_battle(&mut f);
        assert!(offered > 0, "offered from the attacked seat");
        for c in [dd, a] {
            assert_eq!(f.cards[c].current.location, location::REMOVED);
        }
    }

    /// **An attacker that cannot be banished does not offer the
    /// trigger.** The `chk == 0` question asks whether the *other*
    /// combatant can actually go, not merely whether there is one.
    #[test]
    fn an_unbanishable_attacker_offers_nothing() {
        let (mut f, dd, _) = field(None);
        f.infos.turn_player = 1;
        let a = put(&mut f, 1, vanilla(2000, 1200), location::MZONE, 0);
        f.cards[a].current.position = crate::position::FACEUP_ATTACK;
        let e = api::create_effect(&mut f, a);
        api::set_type(&mut f, e, effect_type::SINGLE);
        api::set_code(&mut f, e, code::CANNOT_REMOVE);
        api::register_effect(&mut f, a, e, false);
        let offered = run_battle(&mut f);
        assert_eq!(offered, 0, "nothing worth asking about");
        assert_eq!(
            f.cards[dd].current.location,
            location::MZONE,
            "the defender stands"
        );
        // The attacker still loses the battle — 1200 into 1500 — but it
        // is *destroyed*, not banished, which is the distinction.
        assert_eq!(f.cards[a].current.location, location::GRAVE);
        assert!(f.cards[a].reason & reason::BATTLE != 0, "by battle");
        assert!(f.cards[a].reason & reason::EFFECT == 0, "not by an effect");
    }

    /// **A combatant that is no longer the card that fought is dropped**,
    /// both from what `target` records and from what `operation`
    /// banishes. The reset id is what changes when a card leaves and
    /// comes back, so changing it is the whole of "this is a different
    /// card now".
    #[test]
    fn a_combatant_that_left_and_came_back_is_dropped() {
        let (mut f, dd, t) = field(Some(1500));
        let t = t.unwrap();
        f.core.attacker = Some(dd);
        f.core.attack_target = Some(t);
        f.core.pre_field = [f.cards[dd].fieldid_r, f.cards[t].fieldid_r];
        let e = f.cards[dd].single_effect.equal_range(code::BATTLED)[0];
        let ev = crate::event::Event::new(code::BATTLED);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: Some(dd),
            args: &[],
        };
        // Both still count, for the group to record and the group to
        // banish alike.
        assert!(asks(&mut f, &ctx, true, None));
        assert_eq!(
            still_the_cards_that_fought(&f, Some(dd), Some(t)),
            vec![dd, t],
            "attacker first, then defender"
        );
        // Now the defender is a different card.
        let fresh = f.next_field_id_raw();
        f.cards[t].fieldid_r = fresh;
        assert_eq!(
            still_the_cards_that_fought(&f, Some(dd), Some(t)),
            vec![dd],
            "and it drops out of what would be recorded"
        );
        f.core.subunits.clear();
        operation(&mut f, &ctx);
        assert_eq!(f.core.subunits.len(), 1, "one send-to queued");
        let queued = match &f.core.subunits[0].kind {
            Kind::SendTo { targets, .. } => f.group(*targets).iter().copied().collect::<Vec<_>>(),
            other => panic!("unexpected unit: {other:?}"),
        };
        assert_eq!(
            queued,
            vec![dd],
            "only the card that is still the one that fought"
        );
    }

    /// **A direct attack has no attack target**, and the card copes: the
    /// window is not offered at all, because the `chk == 0` question needs
    /// something on the other side of the battle.
    #[test]
    fn a_direct_attack_offers_nothing() {
        let (mut f, dd, _) = field(None);
        let offered = run_battle(&mut f);
        assert_eq!(offered, 0, "nothing to banish but itself");
        assert_eq!(
            f.cards[dd].current.location,
            location::MZONE,
            "and it stays on the field"
        );
        assert!(f.players[1].lp < 8000, "the direct attack still connected");
    }

    thread_local! {
        /// The `REMOVE` operation the chain link recorded, captured at the
        /// first window that had something on it.
        static OPINFO: std::cell::RefCell<Option<Option<crate::chain::OpTarget>>> =
            const { std::cell::RefCell::new(None) };
    }

    fn recorded_remove() -> Option<crate::chain::OpTarget> {
        OPINFO.with(|o| o.borrow().clone().flatten())
    }

    /// Declare the one attack, take every window offered, and stop when
    /// the battle menu comes back. Returns how many chains were offered
    /// to the attacking player across the battle.
    fn run_battle(f: &mut Field) -> usize {
        OPINFO.with(|o| *o.borrow_mut() = None);
        f.emplace(Kind::BattleCommand {
            state: Box::default(),
        });
        let mut attacked = false;
        let mut offered = 0;
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => {
                    // The link exists only while the chain is live, so the
                    // recorded operation is taken at whatever pause comes
                    // first after it forms.
                    if OPINFO.with(|o| o.borrow().is_none()) {
                        if let Some(link) = f.core.current_chain.last() {
                            let rec = link.opinfos.get(&category::REMOVE).cloned();
                            if rec.is_some() {
                                OPINFO.with(|o| *o.borrow_mut() = Some(rec));
                            }
                        }
                    }
                    match f.messages.last() {
                        Some(Message::SelectBattleCmd { attackable, .. }) if !attacked => {
                            assert!(!attackable.is_empty(), "the attack is available");
                            f.core.returns.set(1);
                            attacked = true;
                        }
                        Some(Message::SelectBattleCmd { .. }) => break,
                        Some(Message::SelectChain { player, chains, .. }) => {
                            if *player == 0 {
                                offered += chains.len();
                            }
                            f.core.returns.set(if chains.is_empty() { -1 } else { 0 });
                        }
                        // An optional trigger offers itself this way.
                        Some(Message::SelectEffectYesNo { .. }) => {
                            offered += 1;
                            f.core.returns.set(1);
                        }
                        Some(Message::SelectCard { min, .. }) => {
                            let min = usize::from(*min);
                            f.core.return_cards.clear();
                            f.core.returns.set_i32(0, 0);
                            f.core.returns.set_i32(1, min as i32);
                            for i in 0..min {
                                f.core.returns.set_i32(i + 2, i as i32);
                            }
                        }
                        other => panic!("unexpected question: {other:?}"),
                    }
                }
                _ => break,
            }
        }
        assert!(attacked, "the attack was declared");
        offered
    }
}
