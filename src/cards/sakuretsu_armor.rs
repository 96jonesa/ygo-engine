//! Sakuretsu Armor — `c56120475.lua`.
//!
//! Mirror Force's opposite number: the same trigger, the same condition,
//! and a target of exactly one — the monster that declared the attack.
//! Like Trap Hole it **targets without a choice**, so `Duel.GetAttacker`
//! stands in for the selection the seam cannot yet run.
//!
//! ## Three tests at resolution, not one
//!
//! `activate` asks whether the target is still the card that was
//! targeted (`IsRelateToEffect`), whether it can still attack
//! (`CanAttack`), and whether its attack was **cancelled**
//! (`STATUS_ATTACK_CANCELED`). The last is the interesting one: an
//! attack that something else already called off leaves the monster on
//! the field, and Sakuretsu Armor is not allowed to destroy it anyway.
//!
//! Note `Card.IsStatus` is the **any-bit** reader — the script-facing
//! name for `card::get_status`, not for the core's all-bit `is_status`
//! (`docs/script-library.md` §8). With a single-bit mask the two agree,
//! which is exactly why getting it wrong here would never show.
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::card::{reason, status};
use crate::effect::{effect_type, flag, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 56_120_475;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate
    let e1 = api::create_effect(f, c);
    api::set_category(f, e1, category::DESTROY);
    api::set_property(f, e1, flag::CARD_TARGET, 0);
    api::set_type(f, e1, effect_type::ACTIVATE);
    api::set_code(f, e1, code::ATTACK_ANNOUNCE);
    api::set_condition(f, e1, condition);
    api::set_target(f, e1, target);
    api::set_operation(f, e1, activate);
    api::register_effect(f, c, e1, false);
}

fn condition(f: &mut Field, ctx: &Ctx) -> bool {
    api::is_turn_player(f, 1 - ctx.player)
}

fn target(f: &mut Field, ctx: &Ctx, chk: bool, chkc: Option<CardId>) -> Yield {
    let tg = api::get_attacker(f);
    if let Some(chkc) = chkc {
        return api::yes(Some(chkc) == tg);
    }
    let Some(tg) = tg else {
        // The script dereferences the attacker; there is always one in
        // the window this fires in, and with none there is nothing to do.
        return api::yes(false);
    };
    if !chk {
        return api::yes(
            api::is_on_field(f, tg) && api::is_can_be_effect_target(f, tg, ctx.reason_effect),
        );
    }
    api::set_target_card(f, vec![tg]);
    api::set_operation_info(f, 0, category::DESTROY, Some(vec![tg]), 1, 0, 0);
    api::yes(true)
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let Some(tc) = api::get_first_target(f) else {
        return Yield::Done(0);
    };
    if api::is_relate_to_effect(f, tc, ctx.reason_effect)
        && api::can_attack(f, tc)
        && !api::is_status(f, tc, status::ATTACK_CANCELED)
    {
        api::destroy(f, vec![tc], reason::EFFECT);
    }
    Yield::Done(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::location;
    use crate::card::{card_type, Card, CardData};
    use crate::duel::phases;
    use crate::field::Message;
    use crate::position;
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

    fn monster(code_: u32, atk: i32) -> CardData {
        CardData {
            code: code_,
            type_: card_type::MONSTER | card_type::NORMAL,
            level: 4,
            attack: atk,
            defense: 1000,
            ..Default::default()
        }
    }

    /// Player 1's Battle Step, its monster ready to attack, with
    /// Sakuretsu Armor Set in player 0's row since an earlier turn and
    /// `mine` monsters of player 0's to be attacked.
    fn field(mine: usize) -> (Field, CardId, CardId, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 4;
        f.infos.turn_player = 1;
        f.infos.phase = phases::BATTLE_STEP;
        let attacker = put(&mut f, 1, monster(1000, 1800), location::MZONE, 0);
        f.cards[attacker].current.position = position::FACEUP_ATTACK;
        let defenders = (0..mine)
            .map(|i| {
                let d = put(
                    &mut f,
                    0,
                    monster(2000 + i as u32, 1200),
                    location::MZONE,
                    i as u32,
                );
                f.cards[d].current.position = position::FACEUP_ATTACK;
                d
            })
            .collect();
        let sa = put(
            &mut f,
            0,
            crate::cards::card_data(CODE).expect("printed data"),
            location::SZONE,
            0,
        );
        f.cards[sa].current.position = position::FACEDOWN;
        f.initialize_card(sa);
        (f, sa, attacker, defenders)
    }

    /// Declare player 1's attack and take every window. Returns how many
    /// chains player 0 was offered.
    fn attack_and_respond(f: &mut Field) -> Vec<usize> {
        f.emplace(Kind::BattleCommand {
            state: Box::default(),
        });
        let mut attacked = false;
        let mut offered = Vec::new();
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
                            offered.push(chains.len());
                        }
                        f.core.returns.set(if chains.is_empty() { -1 } else { 0 });
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
                },
                _ => break,
            }
        }
        assert!(attacked, "the attack was declared");
        offered
    }

    /// **One activate effect on the attack announcement**, targeting, with
    /// a condition and the destroy category.
    #[test]
    fn the_script_registers_a_targeting_activate_effect() {
        let (f, sa, _, _) = field(1);
        let ids = f.cards[sa].field_effect.equal_range(code::ATTACK_ANNOUNCE);
        assert_eq!(ids.len(), 1);
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(e.is_flag(flag::CARD_TARGET), "it targets");
        assert!(e.is_flag(crate::effect::flag::INITIAL));
        assert!(e.condition.is_some());
        assert_eq!(e.category, category::DESTROY);
    }

    /// **The attacker is destroyed, and the defender is untouched.**
    /// Sakuretsu Armor names one card, and it is the one that declared
    /// the attack.
    #[test]
    fn it_destroys_the_attacker_and_nothing_else() {
        let (mut f, sa, attacker, defenders) = field(1);
        let offered = attack_and_respond(&mut f);
        assert!(offered.contains(&1), "offered: {offered:?}");
        assert_eq!(f.cards[attacker].current.location, location::GRAVE);
        assert!(f.cards[attacker].reason & reason::EFFECT != 0, "by effect");
        assert_eq!(
            f.cards[defenders[0]].current.location,
            location::MZONE,
            "the defender is not in the battle it cares about"
        );
        assert_eq!(
            f.cards[sa].current.location,
            location::GRAVE,
            "a resolved Trap"
        );
        let announced: Vec<_> = f
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::BecomeTarget { cards } => Some(cards.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(announced, vec![vec![attacker]], "the attacker was named");
    }

    /// **On its own controller's turn it is not offered** — the same
    /// condition Mirror Force has, driven through the seam.
    #[test]
    fn on_its_own_controllers_turn_it_is_not_offered() {
        let (mut f, sa, _, _) = field(1);
        f.infos.turn_player = 0;
        // Give player 0 the attack instead.
        let mine = put(&mut f, 0, monster(3000, 1900), location::MZONE, 3);
        f.cards[mine].current.position = position::FACEUP_ATTACK;
        let offered = attack_and_respond(&mut f);
        assert!(offered.iter().all(|&n| n == 0), "offered: {offered:?}");
        assert_eq!(f.cards[sa].current.location, location::SZONE, "still Set");
    }

    /// **A cancelled attack is spared.** The resolution test that has
    /// nothing to do with the target still being there: an attack already
    /// called off leaves the monster alone.
    #[test]
    fn an_attack_already_cancelled_spares_the_monster() {
        let (mut f, sa, attacker, _) = field(1);
        let e = f.cards[sa].field_effect.equal_range(code::ATTACK_ANNOUNCE)[0];
        f.core.attacker = Some(attacker);
        f.core.reason_effect = Some(e);
        let ev = crate::event::Event::new(code::ATTACK_ANNOUNCE);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        // Name the target the way an activation would.
        let mut ch = crate::chain::Chain::new(e, ev.clone());
        ch.triggering_player = 0;
        ch.chain_count = 1;
        f.core.current_chain.push(ch);
        assert!(asks(&mut f, &ctx, true, None));
        assert_eq!(api::get_first_target(&f), Some(attacker));

        f.cards[attacker].status |= status::ATTACK_CANCELED;
        f.core.subunits.clear();
        activate(&mut f, &ctx);
        assert!(
            f.core.subunits.is_empty(),
            "the attack was called off, so nothing is destroyed"
        );
        f.cards[attacker].status &= !status::ATTACK_CANCELED;
        activate(&mut f, &ctx);
        assert_eq!(f.core.subunits.len(), 1, "and otherwise it is");
    }

    /// **Each of the three resolution tests can refuse on its own.** The
    /// relation, the ability to attack, and the cancelled attack are
    /// separate questions, and a card failing any one is spared.
    #[test]
    fn each_resolution_test_refuses_on_its_own() {
        let (mut f, sa, attacker, _) = field(1);
        let e = f.cards[sa].field_effect.equal_range(code::ATTACK_ANNOUNCE)[0];
        f.core.attacker = Some(attacker);
        let ev = crate::event::Event::new(code::ATTACK_ANNOUNCE);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        let mut ch = crate::chain::Chain::new(e, ev.clone());
        ch.triggering_player = 0;
        ch.chain_count = 1;
        f.core.current_chain.push(ch);
        assert!(asks(&mut f, &ctx, true, None));

        let queued = |f: &mut Field| {
            f.core.subunits.clear();
            activate(f, &ctx);
            f.core.subunits.len()
        };
        assert_eq!(queued(&mut f), 1, "all three pass: destroyed");

        // 1. No longer the card that was targeted.
        f.cards[attacker].clear_relate_effect();
        assert_eq!(queued(&mut f), 0, "the relation is gone");
        f.cards[attacker].relate_effect_insert_for_test(e);
        assert_eq!(queued(&mut f), 1);

        // 2. It can no longer attack — face-up defence cannot.
        f.cards[attacker].current.position = position::FACEUP_DEFENSE;
        assert_eq!(queued(&mut f), 0, "it could not attack now");
        f.cards[attacker].current.position = position::FACEUP_ATTACK;
        assert_eq!(queued(&mut f), 1);

        // 3. The attack was already called off.
        f.cards[attacker].status |= status::ATTACK_CANCELED;
        assert_eq!(queued(&mut f), 0, "the attack is off");
    }

    /// **An untargetable attacker is not offered.** `chk == 0` asks both
    /// whether it is on the field and whether it may be targeted, and a
    /// battle-destroyed monster is the case that separates the two.
    #[test]
    fn an_untargetable_attacker_is_not_offered() {
        let (mut f, sa, attacker, _) = field(1);
        let e = f.cards[sa].field_effect.equal_range(code::ATTACK_ANNOUNCE)[0];
        f.core.attacker = Some(attacker);
        let ev = crate::event::Event::new(code::ATTACK_ANNOUNCE);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(asks(&mut f, &ctx, false, None), "ordinarily offered");
        f.cards[attacker].status |= status::BATTLE_DESTROYED;
        assert!(
            api::is_on_field(&f, attacker),
            "still on the field, so that half still passes"
        );
        assert!(
            !asks(&mut f, &ctx, false, None),
            "but no longer a legal target"
        );
    }

    /// **The third question names the attacker**, and no play reaches it.
    #[test]
    fn the_third_question_names_the_attacker() {
        let (mut f, sa, attacker, defenders) = field(1);
        let e = f.cards[sa].field_effect.equal_range(code::ATTACK_ANNOUNCE)[0];
        f.core.attacker = Some(attacker);
        let ev = crate::event::Event::new(code::ATTACK_ANNOUNCE);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(asks(&mut f, &ctx, false, Some(attacker)));
        assert!(!asks(&mut f, &ctx, false, Some(defenders[0])));
        // With no attacker at all there is nothing to name.
        f.core.attacker = None;
        assert!(!asks(&mut f, &ctx, false, Some(attacker)));
        assert!(!asks(&mut f, &ctx, false, None));
    }
}
