//! Mirror Force — `c44095762.lua`.
//!
//! The first card with a **condition**, and the first to fire on an
//! attack. One `EFFECT_TYPE_ACTIVATE` effect on `EVENT_ATTACK_ANNOUNCE`
//! whose condition is `Duel.IsTurnPlayer(1-tp)` — only on the opponent's
//! turn, which is the whole of "when an opponent's monster declares an
//! attack" as the script expresses it: the event itself carries the
//! declaration, the condition carries whose it was.
//!
//! The scan is one-sided. `Duel.GetMatchingGroup(s.filter, tp, 0,
//! LOCATION_MZONE, nil)` passes **zero** as the controller's mask, so
//! nothing of the activating player's is ever in the group — Mirror Force
//! clears the attacker's board and leaves its own alone, and that
//! asymmetry is in the masks, not in a filter.
//!
//! The filter is `Card.IsAttackPos`, a test of orientation: a monster in
//! defence survives, and so does a Set one (face-down *defence*).
//!
//! Every call here is to `script_api`, by the rule in `cards/mod.rs`.

use crate::board::location;
use crate::card::reason;
use crate::effect::{effect_type, Ctx, Yield};
use crate::event::{category, code, CardId};
use crate::field::Field;
use crate::script_api as api;

pub const CODE: u32 = 44_095_762;

pub fn initial_effect(f: &mut Field, c: CardId) {
    // Activate
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
    api::is_turn_player(f, 1 - ctx.player)
}

fn filter(f: &mut Field, c: CardId) -> bool {
    api::is_attack_pos(f, c)
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
    let n = g.len() as u8;
    api::set_operation_info(f, 0, category::DESTROY, Some(g), n, 0, 0);
    api::yes(true)
}

fn activate(f: &mut Field, ctx: &Ctx) -> Yield {
    let tp = ctx.player;
    let g = api::get_matching_group(f, Some(&filter), tp, 0, MZONE, api::Except::None);
    if !g.is_empty() {
        api::destroy(f, g, reason::EFFECT);
    }
    Yield::Done(0)
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

    fn card(f: &mut Field, owner: u8, code: u32, type_: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code,
                type_,
                level: 4,
                attack: 1700,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        let fid = f.next_field_id_raw();
        f.cards[id].fieldid = fid;
        f.cards[id].fieldid_r = fid;
        id
    }

    fn monster(f: &mut Field, owner: u8, seat: u32, pos: u8) -> CardId {
        let id = card(
            f,
            owner,
            1000 + seat + u32::from(owner) * 100,
            card_type::MONSTER,
        );
        f.add_card(owner, id, location::MZONE, seat, false);
        f.cards[id].current.position = pos;
        id
    }

    /// A Battle Step with Mirror Force Set in player 0's row since an
    /// earlier turn. `theirs` is player 1's board and `mine` player 0's,
    /// as `(seat, position)` pairs; `turn_player` is whose Battle Phase it
    /// is, which is what the condition reads.
    fn field(
        turn_player: u8,
        theirs: &[(u32, u8)],
        mine: &[(u32, u8)],
    ) -> (Field, CardId, Vec<CardId>, Vec<CardId>) {
        let mut f = Field::new(8000);
        f.infos.turn_id = 4;
        f.infos.turn_player = turn_player;
        f.infos.phase = phases::BATTLE_STEP;
        let theirs = theirs
            .iter()
            .map(|&(seat, pos)| monster(&mut f, 1, seat, pos))
            .collect();
        let mine = mine
            .iter()
            .map(|&(seat, pos)| monster(&mut f, 0, seat, pos))
            .collect();
        let mf = card(&mut f, 0, CODE, card_type::TRAP);
        f.add_card(0, mf, location::SZONE, 0, false);
        f.cards[mf].current.position = position::FACEDOWN;
        f.initialize_card(mf);
        (f, mf, theirs, mine)
    }

    /// Declare the turn player's attack with its first attacker and
    /// answer every response window by activating the first offer,
    /// stopping when the Battle Phase menu comes back. Returns how many
    /// chains player 0 was offered at each of its windows.
    fn attack_and_respond(
        f: &mut Field,
    ) -> (
        Vec<usize>,
        Option<std::collections::BTreeMap<u64, crate::chain::OpTarget>>,
    ) {
        f.emplace(Kind::BattleCommand {
            state: Box::default(),
        });
        let mut attacked = false;
        let mut offered = Vec::new();
        let mut opinfo = None;
        for _ in 0..8192 {
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => match f.messages.last() {
                    Some(Message::SelectBattleCmd { attackable, .. }) if !attacked => {
                        assert!(!attackable.is_empty(), "player 1 can attack");
                        // 1 = attack, index 0
                        f.core.returns.set(1);
                        attacked = true;
                    }
                    // The Battle Phase is asked again once the attack is
                    // over. That is the boundary the card's work ends at,
                    // so stop here rather than playing on into the End
                    // Phase — where the rule cleanup would move cards for
                    // reasons that have nothing to do with Mirror Force.
                    Some(Message::SelectBattleCmd { .. }) => break,
                    Some(Message::SelectChain { player, chains, .. }) => {
                        if *player == 0 {
                            offered.push(chains.len());
                        }
                        if opinfo.is_none() {
                            if let Some(link) = f.core.current_chain.last() {
                                opinfo = Some(link.opinfos.clone());
                            }
                        }
                        f.core.returns.set(if chains.is_empty() { -1 } else { 0 });
                    }
                    // Which monster to attack. The tagged encoding:
                    // slot 0 is the index width (0 = u32), slot 1 the
                    // count, then indices into the offered list.
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
        (offered, opinfo)
    }

    /// The `DESTROY` operation the chain link recorded, if any.
    fn recorded_destroy(
        opinfo: &Option<std::collections::BTreeMap<u64, crate::chain::OpTarget>>,
    ) -> Option<crate::chain::OpTarget> {
        opinfo.as_ref()?.get(&category::DESTROY).cloned()
    }

    /// **`initial_effect` registers one printed activate effect** on
    /// `EVENT_ATTACK_ANNOUNCE`, with a condition and the destroy category.
    #[test]
    fn the_script_registers_a_printed_activate_effect() {
        let (f, mf, _, _) = field(1, &[], &[]);
        let ids: Vec<_> = f.cards[mf]
            .field_effect
            .equal_range(code::ATTACK_ANNOUNCE)
            .to_vec();
        assert_eq!(ids.len(), 1, "one effect on EVENT_ATTACK_ANNOUNCE");
        let e = f.effects.get(ids[0]).unwrap();
        assert!(e.is_type(effect_type::ACTIVATE));
        assert!(e.is_flag(flag::INITIAL), "stamped while initializing");
        assert!(!e.is_flag(flag::PLAYER_TARGET));
        assert!(e.condition.is_some(), "the script sets one");
        assert_eq!(e.category, category::DESTROY);
        assert_eq!(e.handler, Some(mf));
        assert!(
            f.cards[mf]
                .field_effect
                .equal_range(code::FREE_CHAIN)
                .is_empty(),
            "not a free-chain card"
        );
    }

    /// **The condition is the opponent's turn**, read as the *player id*:
    /// `IsTurnPlayer(1 - tp)`.
    #[test]
    fn the_condition_holds_only_on_the_opponents_turn() {
        let (mut f, mf, _, _) = field(1, &[], &[]);
        let e = f.cards[mf].field_effect.equal_range(code::ATTACK_ANNOUNCE)[0];
        let ev = crate::event::Event::new(0);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(
            condition(&mut f, &ctx),
            "player 1's turn, asked for player 0"
        );
        f.infos.turn_player = 0;
        assert!(!condition(&mut f, &ctx), "its own turn: no");
    }

    /// **The filter is orientation, not face-up-ness.** Attack position
    /// passes whether face-up or face-down; either defence fails.
    #[test]
    fn the_filter_is_attack_position() {
        let mut f = Field::new(8000);
        let up = monster(&mut f, 1, 0, position::FACEUP_ATTACK);
        let down = monster(&mut f, 1, 1, position::FACEDOWN_ATTACK);
        let def = monster(&mut f, 1, 2, position::FACEUP_DEFENSE);
        let set = monster(&mut f, 1, 3, position::FACEDOWN_DEFENSE);
        assert!(filter(&mut f, up));
        assert!(
            filter(&mut f, down),
            "face-down attack position is attack position"
        );
        assert!(!filter(&mut f, def));
        assert!(!filter(&mut f, set), "a Set monster is face-down defence");
    }

    /// **An attack opens a window that offers it**, and activating it
    /// destroys every attack-position monster of the turn player's — and
    /// only those: a defender in defence survives, and so does everything
    /// on the activating player's own side, whatever its position.
    #[test]
    fn an_attack_triggers_it_and_it_clears_the_attackers_board() {
        let (mut f, mf, theirs, mine) = field(
            1,
            &[
                (0, position::FACEUP_ATTACK),
                (1, position::FACEUP_ATTACK),
                (2, position::FACEUP_DEFENSE),
            ],
            &[(0, position::FACEUP_ATTACK)],
        );
        let (offered, opinfo) = attack_and_respond(&mut f);
        assert!(offered.contains(&1), "player 0 was offered it: {offered:?}");
        for &t in &theirs[..2] {
            assert_eq!(
                f.cards[t].current.location,
                location::GRAVE,
                "attack position"
            );
            assert!(f.cards[t].reason & reason::DESTROY != 0);
            assert!(f.cards[t].reason & reason::EFFECT != 0, "by effect");
        }
        assert_eq!(
            f.cards[theirs[2]].current.location,
            location::MZONE,
            "the defender in defence survives"
        );
        assert_eq!(
            f.cards[mine[0]].current.location,
            location::MZONE,
            "my own attack-position monster survives: the s mask is zero"
        );
        assert_eq!(
            f.cards[mf].current.location,
            location::GRAVE,
            "a resolved Normal Trap"
        );
        // What the link recorded: the two attack-position monsters of the
        // turn player's, and a count that matches.
        let recorded = recorded_destroy(&opinfo).expect("a DESTROY operation");
        let mut want = theirs[..2].to_vec();
        want.sort_unstable();
        assert_eq!(recorded.cards, Some(want), "only the attacker's side");
        assert_eq!(recorded.count, 2);
    }

    /// **A direct attack against an empty board still triggers it.** This
    /// is the shape that separates the two masks: the scan passes **zero**
    /// for the controller's own side, so with nothing of player 0's on the
    /// field the only thing it can find is the attacker.
    #[test]
    fn a_direct_attack_against_an_empty_board_still_triggers_it() {
        let (mut f, mf, theirs, _) = field(1, &[(0, position::FACEUP_ATTACK)], &[]);
        let (offered, opinfo) = attack_and_respond(&mut f);
        assert!(
            offered.contains(&1),
            "offered with nothing of my own: {offered:?}"
        );
        assert_eq!(
            f.cards[theirs[0]].current.location,
            location::GRAVE,
            "the direct attacker is destroyed"
        );
        assert!(f.cards[theirs[0]].reason & reason::EFFECT != 0, "by effect");
        assert_eq!(f.cards[mf].current.location, location::GRAVE);
        let recorded = recorded_destroy(&opinfo).expect("a DESTROY operation");
        assert_eq!(recorded.cards, Some(vec![theirs[0]]));
        assert_eq!(recorded.count, 1);
    }

    /// **An empty board at resolution queues nothing** — the script's
    /// `#g > 0` guard. It cannot be reached through an attack, because the
    /// attacker is always in attack position, so the operation is called
    /// directly.
    #[test]
    fn an_empty_board_at_resolution_queues_nothing() {
        let (mut f, mf, _, _) = field(1, &[], &[]);
        let e = f.cards[mf].field_effect.equal_range(code::ATTACK_ANNOUNCE)[0];
        let ev = crate::event::Event::new(0);
        let ctx = Ctx {
            reason_effect: e,
            player: 0,
            event: &ev,
            card: None,
            args: &[],
        };
        assert!(f.core.subunits.is_empty());
        activate(&mut f, &ctx);
        assert!(
            f.core.subunits.is_empty(),
            "nothing to destroy, nothing queued"
        );
        monster(&mut f, 1, 0, position::FACEUP_ATTACK);
        activate(&mut f, &ctx);
        assert_eq!(f.core.subunits.len(), 1, "one destroy queued");
        assert!(matches!(f.core.subunits[0].kind, Kind::Destroy { .. }));
    }

    /// **On its controller's own turn it is not offered.** The attacker
    /// is always in attack position, so the target scan never comes up
    /// empty during an attack — the condition is what refuses, and this
    /// drives it through the seam rather than calling it directly.
    #[test]
    fn on_its_own_controllers_turn_it_is_not_offered() {
        let (mut f, mf, theirs, mine) = field(
            0,
            &[(0, position::FACEUP_ATTACK)],
            &[(0, position::FACEUP_ATTACK)],
        );
        let (offered, _opinfo) = attack_and_respond(&mut f);
        assert!(
            offered.iter().all(|&n| n == 0),
            "nothing offered: {offered:?}"
        );
        assert_eq!(f.cards[mf].current.location, location::SZONE, "still Set");
        // Its own attacker still trades with the defender — equal attack,
        // mutual destruction. The point is *what killed it*: battle, not
        // an effect, so nothing here came from Mirror Force.
        for c in [mine[0], theirs[0]] {
            assert_eq!(f.cards[c].current.location, location::GRAVE);
            assert!(
                f.cards[c].reason & reason::BATTLE != 0,
                "destroyed in battle"
            );
            assert!(
                f.cards[c].reason & reason::EFFECT == 0,
                "and not by an effect"
            );
        }
    }
}
