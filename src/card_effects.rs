//! Registering an effect on a card: `card::add_effect`.
//!
//! The card-side half of registration. `Field::add_effect` (the field-side
//! half, `field::add_effect`) puts an effect into the field's indexes;
//! this puts it into the **card's**, allocates its id, and does the dozen
//! pieces of bookkeeping the reference does at the same moment. Until
//! now the port had only the field half, and tests registered card effects
//! by hand — which skips all of it.
//!
//! ## What registering does besides indexing
//!
//! In order, because each is easy to lose:
//!
//! 1. A `SET_ATTACK`/`SET_DEFENSE` family effect **evicts** its
//!    predecessors of the same family (the stat-setting rules of
//!    precedence), unless either carries `SINGLE_RANGE`.
//! 2. The container is chosen by type — single, equip, target, xmaterial,
//!    field — and anything else is refused with 0.
//! 3. Two codes raise global flags (`SELF_TOGRAVE`, `DETACH_MATERIAL`).
//! 4. The id is allocated from `infos.field_id`, the shared counter.
//! 5. `INITIAL` is stamped while the card is `STATUS_INITIALIZING`; copy
//!    bookkeeping applies while it is `STATUS_COPYING_EFFECT`; and
//!    `COPY_INHERIT` takes the reason effect's copy id and reset.
//! 6. A field-type effect is also registered with the field **if it is in
//!    range now**, or if it is a hand trigger that is not a phase trigger.
//! 7. Disable-related effects queue their targets for a disable check.
//! 8. Oath, phase-reset, chain-reset and count-limited effects go into
//!    the field's side sets.
//! 9. A `CLIENT_HINT` effect announces itself with `MSG_CARD_HINT`.
//! 10. A positive `UPDATE_LEVEL` raises `EVENT_LEVEL_UP` at once.
//!
//! ## The return value is the id, and 0 means "not registered"
//!
//! Three refusals return 0 before any of that happens: an uncopyable
//! effect during a copy (it is remembered in `uncopy`), an effect already
//! indexed on this card, and a type that belongs in no container. The
//! script-facing `Card.RegisterEffect` reads the id back and treats
//! `<= 0` as failure, which is why this is an `i32` rather than a bool.

use crate::board::location;
use crate::card::status;
use crate::effect::{effect_type, flag};
use crate::event::{code, CardId, EffectId, PLAYER_NONE};
use crate::field::{chint, global_flag, reset, Field, Message};

/// The stat codes whose predecessors a new one evicts. Transcribed from
/// the four `if` blocks in the reference, one row each.
const EVICTS: [(&[u32], &[u32]); 4] = [
    (
        &[code::SET_ATTACK, code::SET_BASE_ATTACK],
        &[
            code::SET_ATTACK,
            code::SET_ATTACK_FINAL,
            code::SET_BASE_ATTACK,
        ],
    ),
    (
        &[code::SET_ATTACK_FINAL],
        &[
            code::UPDATE_ATTACK,
            code::SET_ATTACK,
            code::SET_ATTACK_FINAL,
        ],
    ),
    (
        &[code::SET_DEFENSE, code::SET_BASE_DEFENSE],
        &[
            code::SET_DEFENSE,
            code::SET_DEFENSE_FINAL,
            code::SET_BASE_DEFENSE,
        ],
    ),
    (
        &[code::SET_DEFENSE_FINAL],
        &[
            code::UPDATE_DEFENSE,
            code::SET_DEFENSE,
            code::SET_DEFENSE_FINAL,
        ],
    ),
];

impl Field {
    /// `card::add_effect` — register `id` on `card`. Returns the effect's
    /// new id, or 0 if it was not registered.
    pub fn add_card_effect(&mut self, card: CardId, id: EffectId) -> i32 {
        let Some(e) = self.effects.get(id) else {
            return 0;
        };
        let (ty, code_, single_range, uncopyable) = (
            e.effect_type,
            e.code,
            e.is_flag(flag::SINGLE_RANGE),
            e.is_flag(flag::UNCOPYABLE),
        );
        if self.cards[card].get_status(status::COPYING_EFFECT) && uncopyable {
            self.uncopy.insert(id);
            return 0;
        }
        if self.cards[card].indexer.contains(&id) {
            return 0;
        }

        // 1. Stat-setting precedence: the newcomer evicts its family.
        let mut check_target: Vec<CardId> = vec![card];
        if ty & effect_type::SINGLE != 0 {
            for (triggers, victims) in EVICTS {
                if triggers.contains(&code_) && !single_range {
                    let doomed: Vec<EffectId> = victims
                        .iter()
                        .flat_map(|&v| self.cards[card].single_effect.equal_range(v).to_vec())
                        .filter(|&old| {
                            self.effects
                                .get(old)
                                .is_some_and(|o| !o.is_flag(flag::SINGLE_RANGE))
                        })
                        .collect();
                    for old in doomed {
                        self.remove_card_effect(card, old);
                    }
                }
            }
            self.cards[card].single_effect.insert(code_, id);
        } else if ty & effect_type::EQUIP != 0 {
            self.cards[card].equip_effect.insert(code_, id);
            check_target = self.cards[card].equiping_target.into_iter().collect();
        } else if ty & effect_type::TARGET != 0 {
            self.cards[card].target_effect.insert(code_, id);
            check_target = self.cards[card].effect_target_cards.clone();
        } else if ty & effect_type::XMATERIAL != 0 {
            self.cards[card].xmaterial_effect.insert(code_, id);
            check_target = self.cards[card].overlay_target.into_iter().collect();
        } else if ty & effect_type::FIELD != 0 {
            self.cards[card].field_effect.insert(code_, id);
        } else {
            return 0;
        }

        // 3. Global flags two codes raise on registration.
        if code_ == code::SELF_TOGRAVE {
            self.core.global_flag |= global_flag::SELF_TOGRAVE;
        } else if code_ == code::DETACH_MATERIAL {
            self.core.global_flag |= global_flag::DETACH_EVENT;
        }

        // 4-5. The id, and the marks a card's state stamps on the effect.
        let new_id = self.next_field_id_raw();
        let card_type = self.cards[card].data.type_;
        let initializing = self.cards[card].get_status(status::INITIALIZING);
        let copying = self.cards[card].get_status(status::COPYING_EFFECT);
        let (copy_id, copy_reset, copy_reset_count) = (
            self.infos.copy_id,
            self.core.copy_reset,
            self.core.copy_reset_count,
        );
        let reason = self
            .core
            .reason_effect
            .and_then(|r| self.effects.get(r))
            .map(|r| (r.copy_id, r.reset_flag, r.reset_count));
        if let Some(e) = self.effects.get_mut(id) {
            e.id.set(new_id);
            e.initial_id = new_id;
            e.card_type = card_type;
            if initializing {
                e.flag[0] |= flag::INITIAL;
            }
            if copying {
                e.copy_id = copy_id;
                e.reset_flag |= copy_reset;
                e.reset_count = copy_reset_count;
            }
            if e.is_flag(flag::COPY_INHERIT) {
                if let Some((r_copy, r_reset, r_count)) = reason {
                    if r_copy != 0 {
                        e.copy_id = r_copy;
                        e.reset_flag |= r_reset;
                        if e.reset_count > r_count {
                            e.reset_count = r_count;
                        }
                    }
                }
            }
            e.handler = Some(card);
        }
        self.cards[card].indexer.insert(id);

        // 6. Field registration, if it is already somewhere it applies.
        let Some(e) = self.effects.get(id) else {
            return 0;
        };
        let (range, e_type, reset_flag) = (e.range, e.effect_type, e.reset_flag);
        let (is_oath, count_limited, client_hint, description) = (
            e.is_flag(flag::OATH),
            e.is_flag(flag::COUNT_LIMIT),
            e.is_flag(flag::CLIENT_HINT),
            e.description,
        );
        let controller = self.cards[card].current.controller;
        if e_type & effect_type::FIELD != 0 {
            let in_range = e.in_range(&self.cards, &self.cards[card]);
            let hand_trigger = controller != PLAYER_NONE
                && range & u16::from(location::HAND) != 0
                && e_type & effect_type::TRIGGER_O != 0
                && code_ & code::PHASE == 0;
            if in_range || hand_trigger {
                self.add_effect(id, PLAYER_NONE);
            }
        }

        // 7. A disable-related effect re-checks whatever it can disable.
        if controller != PLAYER_NONE && !check_target.is_empty() && self.is_disable_related(id) {
            for t in check_target {
                self.add_to_disable_check_list(t);
            }
        }

        // 8. The side sets.
        if is_oath {
            self.field_effects.oath.insert(id, self.core.reason_effect);
        }
        if reset_flag & reset::PHASE != 0 {
            self.field_effects.pheff.insert(id);
            if let Some(e) = self.effects.get_mut(id) {
                if e.reset_count == 0 {
                    e.reset_count += 1;
                }
            }
        }
        if reset_flag & reset::CHAIN != 0 {
            self.field_effects.cheff.insert(id);
        }
        if count_limited {
            self.field_effects.rechargeable.insert(id);
        }

        // 9. A hint the client shows on the card.
        if client_hint {
            let info = self.get_info_location(card);
            self.messages.push(Message::CardHint {
                controller: info.controller,
                location: info.location,
                sequence: info.sequence,
                kind: chint::DESC_ADD,
                value: description,
            });
        }

        // 10. A level increase is an event of its own, raised at once.
        if e_type & effect_type::SINGLE != 0 && code_ == code::UPDATE_LEVEL && !single_range {
            let val = self.effect_plain_value(id);
            if val > 0 {
                self.raise_single_event(
                    card,
                    vec![],
                    code::LEVEL_UP,
                    Some(id),
                    0,
                    0,
                    0,
                    val as u32,
                );
                self.process_single_event();
            }
        }
        new_id as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
    use crate::effect::Effect;

    fn field() -> Field {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f
    }

    fn card_in(f: &mut Field, loc: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 30000,
                type_: card_type::MONSTER,
                level: 4,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(0, id, loc, 0, false);
        id
    }

    fn single(f: &mut Field, owner: CardId, code_: u32) -> EffectId {
        let mut e = Effect::new(effect_type::SINGLE, code_);
        e.owner = Some(owner);
        f.new_effect(e)
    }

    /// **Registering the same effect twice is refused**, and the index
    /// holds it once.
    #[test]
    fn a_second_registration_of_the_same_effect_is_refused() {
        let mut f = field();
        let c = card_in(&mut f, location::MZONE);
        let e = single(&mut f, c, code::UPDATE_ATTACK);
        assert!(f.add_card_effect(c, e) > 0, "the first is accepted");
        assert_eq!(f.add_card_effect(c, e), 0, "the second is refused");
        assert_eq!(
            f.cards[c]
                .single_effect
                .equal_range(code::UPDATE_ATTACK)
                .len(),
            1
        );
    }

    /// **The handler is the card**, and the id comes from the shared
    /// `infos.field_id` counter — poisoned first, and the counter advanced
    /// first, so neither can pass on a default.
    #[test]
    fn the_handler_and_the_id_are_set_from_the_field() {
        let mut f = field();
        let c = card_in(&mut f, location::MZONE);
        let e = single(&mut f, c, code::UPDATE_ATTACK);
        f.infos.field_id.set(700);
        f.effects.get_mut(e).unwrap().id.set(12345);
        let id = f.add_card_effect(c, e);
        assert_eq!(id, 700, "the id is the counter's value");
        assert_eq!(f.infos.field_id.get(), 701, "and the counter advanced");
        let x = f.effects.get(e).unwrap();
        assert_eq!(x.id.get(), 700);
        assert_eq!(x.initial_id, 700);
        assert_eq!(x.handler, Some(c), "the handler is the card");
    }

    /// **A stat setter evicts its family.** A second `SET_ATTACK` removes
    /// the first; a `SINGLE_RANGE` one on either side is spared.
    #[test]
    fn a_set_attack_evicts_the_previous_set_attack() {
        let mut f = field();
        let c = card_in(&mut f, location::MZONE);
        let first = single(&mut f, c, code::SET_ATTACK);
        let second = single(&mut f, c, code::SET_ATTACK);
        f.add_card_effect(c, first);
        f.add_card_effect(c, second);
        let live = f.cards[c]
            .single_effect
            .equal_range(code::SET_ATTACK)
            .to_vec();
        assert_eq!(live, vec![second], "only the newcomer remains");
        assert!(
            !f.cards[c].indexer.contains(&first),
            "the old one left the index too"
        );

        // A single-range setter is not evicted.
        let ranged = single(&mut f, c, code::SET_ATTACK);
        f.effects.get_mut(ranged).unwrap().flag[0] |= flag::SINGLE_RANGE;
        f.add_card_effect(c, ranged);
        let third = single(&mut f, c, code::SET_ATTACK);
        f.add_card_effect(c, third);
        assert!(
            f.cards[c].indexer.contains(&ranged),
            "SINGLE_RANGE is spared"
        );
        assert!(!f.cards[c].indexer.contains(&second));
    }

    /// **A client-hint effect announces itself** with the description.
    #[test]
    fn a_client_hint_is_announced_on_the_card() {
        let mut f = field();
        let c = card_in(&mut f, location::MZONE);
        let e = single(&mut f, c, 0);
        {
            let x = f.effects.get_mut(e).unwrap();
            x.flag[0] |= flag::CLIENT_HINT;
            x.description = 67;
        }
        f.add_card_effect(c, e);
        let hint = f.messages.iter().find_map(|m| match m {
            Message::CardHint { kind, value, .. } => Some((*kind, *value)),
            _ => None,
        });
        assert_eq!(hint, Some((chint::DESC_ADD, 67)));
    }

    /// **A field-type effect whose range covers the card's zone is put in
    /// the field's index at registration.** One that does not is not.
    #[test]
    fn a_field_effect_in_range_is_registered_with_the_field_at_once() {
        let mut f = field();
        let c = card_in(&mut f, location::MZONE);
        let mut e = Effect::new(effect_type::FIELD, code::UPDATE_ATTACK);
        e.owner = Some(c);
        e.range = u16::from(location::MZONE);
        let e = f.new_effect(e);
        f.add_card_effect(c, e);
        assert!(f.field_effects.indexer.contains(&e), "in range: indexed");

        let d = card_in(&mut f, location::GRAVE);
        let mut g = Effect::new(effect_type::FIELD, code::UPDATE_ATTACK);
        g.owner = Some(d);
        g.range = u16::from(location::MZONE);
        let g = f.new_effect(g);
        f.add_card_effect(d, g);
        assert!(
            !f.field_effects.indexer.contains(&g),
            "out of range: not indexed"
        );
    }
}
