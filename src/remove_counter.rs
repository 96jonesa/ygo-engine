//! Taking counters off cards.
//!
//! `field::process(Processors::RemoveCounter&)`,
//! `field::process(Processors::SelectCounter&)` and
//! `field::get_field_counter`.
//!
//! ## The counter codes are **bases**, not keys
//!
//! `EFFECT_RCOUNTER_REPLACE` is `0x30000` and the registered code is
//! `0x30000 + countertype`; the raised event is `EVENT_REMOVE_COUNTER +
//! countertype` from `0x20000`. Looking either up by the bare constant
//! finds the effects for counter type **zero** and nothing else.
//!
//! ## `RemoveCounter`'s jump table, and the case it skips
//!
//! | from | `arg.step` | runs next |
//! |---|---|---|
//! | 0 | — (returns false) | case 1 |
//! | 1, a replacing effect | `3` | **case 4** |
//! | 1, a named card | `3` | **case 4** |
//! | 1, no named card | — (returns false) | case 2 |
//! | 2 | — (returns false) | case 3 |
//! | 3 | — (returns false) | case 4 |
//!
//! Both `arg.step = 3` jumps land on case **4** — the increment rule — so
//! they **skip case 3**, which is the only place `EVENT_REMOVE_COUNTER` is
//! raised.
//!
//! So removing counters *from a named card* raises no event, and only the
//! "choose across the field" path does. That reads like a slip and is the
//! reference's behaviour; a port that wrote `set_step(2)` here to "reach
//! case 3" would raise an event ocgcore does not, and nothing in a unit
//! test would say which of the two was right.
//!
//! ## `SelectCounter` clamps a **local**, and the clamp does not survive
//!
//! Case 0 takes `auto count = arg.count`, clamps it to the total actually
//! on the field, and writes the clamped value into the message. The
//! validation branch re-reads `arg.count`, which was never touched.
//!
//! With one card offered that is harmless: case 0 answers for the player.
//! With two or more and `count` above the field's total, the player is
//! shown the clamped number, answers it, and is refused forever — the
//! check demands the unclamped one. It is reproduced here because it is
//! what the reference does, not because it is right.

use crate::board::location;
use crate::event::{code, CardId, EffectId};
use crate::field::{CounterOffer, Field, Message};
use crate::processor::Kind;

impl Field {
    /// `field::get_field_counter` — how many counters of a type are on the
    /// field, counting whichever sides `self`/`oppo` permit.
    ///
    /// The loop flips **both** the player and which flag is consulted on
    /// each pass, so `self` governs `playerid`'s side and `oppo` the other
    /// one. Flipping only the player reads `self` twice.
    pub fn get_field_counter(
        &self,
        playerid: u8,
        self_side: bool,
        oppo_side: bool,
        counter_type: u16,
    ) -> u32 {
        let mut p = playerid;
        let mut allowed = self_side;
        let mut total = 0;
        for _ in 0..2 {
            if allowed {
                for zone in [
                    &self.players[p as usize].mzone,
                    &self.players[p as usize].szone,
                ] {
                    for c in zone.iter().flatten() {
                        total += u32::from(self.get_counter(*c, counter_type));
                    }
                }
            }
            p = 1 - p;
            allowed = oppo_side;
        }
        total
    }

    /// Queue a counter removal.
    #[allow(clippy::too_many_arguments)]
    /// `field::is_player_can_remove_counter` — could `count` counters of
    /// this type be taken, one way or another?
    ///
    /// Two ways, and the second is the one a bare "are there enough?"
    /// misses: a **replacement** effect registered on
    /// `EFFECT_RCOUNTER_REPLACE + countertype` that is activatable right
    /// now makes the removal payable however few counters there are. That
    /// is the same shape as `check_lp_cost`'s, and for the same reason —
    /// a cost is payable if *something* can pay it.
    ///
    /// `pcard` names one card, or `None` to count the field.
    #[allow(clippy::too_many_arguments)]
    pub fn is_player_can_remove_counter(
        &mut self,
        playerid: u8,
        pcard: Option<CardId>,
        self_side: bool,
        oppo_side: bool,
        counter_type: u16,
        count: u16,
        reason: u32,
    ) -> bool {
        let have = match pcard {
            Some(c) => u32::from(self.get_counter(c, counter_type)),
            None => self.get_field_counter(playerid, self_side, oppo_side, counter_type),
        };
        if have >= u32::from(count) {
            return true;
        }
        let ev = {
            let mut e = crate::event::Event::new(0);
            e.event_cards.clear();
            e.event_player = playerid;
            e.event_value = u32::from(count);
            e.reason = reason;
            e.reason_effect = self.core.reason_effect;
            e.reason_player = playerid;
            e
        };
        let which = code::RCOUNTER_REPLACE + u32::from(counter_type);
        for e in self.field_effects.continuous.equal_range(which).to_vec() {
            let p = self.effects.get(e).map_or(crate::event::PLAYER_NONE, |x| {
                x.get_handler_player(&self.cards)
            });
            if self.is_activateable(e, p, &ev, false, false, false, false, false) {
                return true;
            }
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    pub fn remove_counter_unit(
        &mut self,
        reason: u32,
        pcard: Option<CardId>,
        rplayer: u8,
        self_side: bool,
        oppo_side: bool,
        counter_type: u16,
        count: u16,
    ) {
        self.emplace(Kind::RemoveCounter {
            reason,
            pcard,
            rplayer,
            self_side,
            oppo_side,
            counter_type,
            count,
        });
    }

    /// One step of `RemoveCounter`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn remove_counter_step(
        &mut self,
        step: u16,
        reason: u32,
        pcard: Option<CardId>,
        rplayer: u8,
        self_side: bool,
        oppo_side: bool,
        counter_type: u16,
        count: u16,
    ) -> bool {
        match step {
            0 => self.rc_offer(
                reason,
                pcard,
                rplayer,
                self_side,
                oppo_side,
                counter_type,
                count,
            ),
            1 => self.rc_apply(
                reason,
                pcard,
                rplayer,
                self_side,
                oppo_side,
                counter_type,
                count,
            ),
            2 => {
                // The chosen split, one `int16_t` per offered card.
                for i in 0..self.core.select_cards.len() {
                    let n = self.core.returns.at_i16(i);
                    if n > 0 {
                        let c = self.core.select_cards[i];
                        self.remove_counter(c, counter_type, n as u16);
                    }
                }
                false
            }
            3 => {
                // Reached ONLY from case 2 — the named-card and replacing
                // paths jump past it. See the module note.
                self.raise_event(
                    None,
                    code::REMOVE_COUNTER + u32::from(counter_type),
                    self.core.reason_effect,
                    reason,
                    rplayer,
                    rplayer,
                    u32::from(count),
                );
                self.process_instant_event();
                false
            }
            4 => {
                self.core.returns.set(1);
                true
            }
            _ => true,
        }
    }

    /// Case 0: what could happen — the plain removal, plus any effect that
    /// offers to replace it — and ask if there is a choice.
    #[allow(clippy::too_many_arguments)]
    fn rc_offer(
        &mut self,
        reason: u32,
        pcard: Option<CardId>,
        rplayer: u8,
        self_side: bool,
        oppo_side: bool,
        counter_type: u16,
        count: u16,
    ) -> bool {
        self.core.select_options.clear();
        self.core.select_effects.clear();

        // `None` is the plain removal. It is offered only if it could
        // actually happen — a card without enough counters, or a field
        // without any, leaves the player nothing but the replacements.
        let possible = match pcard {
            Some(c) => self.get_counter(c, counter_type) >= count,
            None => self.get_field_counter(rplayer, self_side, oppo_side, counter_type) != 0,
        };
        if possible {
            self.core.select_options.push(10);
            self.core.select_effects.push(None);
        }

        let ev = {
            let mut e = crate::event::Event::new(0);
            e.event_cards.clear();
            e.event_player = rplayer;
            e.event_value = u32::from(count);
            e.reason = reason;
            e.reason_effect = self.core.reason_effect;
            e.reason_player = rplayer;
            e
        };
        // The registered code is the base plus the counter type.
        let which = code::RCOUNTER_REPLACE + u32::from(counter_type);
        for e in self.field_effects.continuous.equal_range(which).to_vec() {
            let p = self.effects.get(e).map_or(crate::event::PLAYER_NONE, |x| {
                x.get_handler_player(&self.cards)
            });
            if self.is_activateable(e, p, &ev, false, false, false, false, false) {
                let d = self.effects.get(e).map_or(0, |x| x.description);
                self.core.select_options.push(d);
                self.core.select_effects.push(Some(e));
            }
        }

        self.core.returns.set(0);
        if self.core.select_options.is_empty() {
            return true;
        }
        if self.core.select_options.len() == 1 {
            self.core.returns.set(0);
        } else if self.core.select_effects[0].is_none() && self.core.select_effects.len() == 2 {
            // Exactly one alternative to the plain removal is a yes/no
            // about *that* effect, not a menu of two.
            let handler = self.core.select_effects[1]
                .and_then(|e| self.effects.get(e).and_then(|x| x.get_handler(&self.cards)));
            if let Some(card) = handler {
                self.emplace(Kind::SelectEffectYesNo {
                    player: rplayer,
                    description: 220,
                    card,
                });
            }
        } else {
            self.emplace(Kind::SelectOption { player: rplayer });
        }
        false
    }

    /// Case 1: carry out whichever of the offers was taken.
    #[allow(clippy::too_many_arguments)]
    fn rc_apply(
        &mut self,
        reason: u32,
        pcard: Option<CardId>,
        rplayer: u8,
        self_side: bool,
        oppo_side: bool,
        counter_type: u16,
        count: u16,
    ) -> bool {
        let picked = self.core.returns.get().max(0) as usize;
        let chosen: Option<EffectId> = self
            .core
            .select_effects
            .get(picked)
            .copied()
            .unwrap_or(None);

        if let Some(e) = chosen {
            let ev = {
                let mut x = crate::event::Event::new(0);
                x.event_cards.clear();
                x.event_player = rplayer;
                x.event_value = u32::from(count);
                x.reason = reason;
                x.reason_effect = self.core.reason_effect;
                x.reason_player = rplayer;
                x
            };
            let p = self.effects.get(e).map_or(crate::event::PLAYER_NONE, |x| {
                x.get_handler_player(&self.cards)
            });
            self.solve_continuous(p, e, ev);
            // 3, not 4: the increment lands on case 4, skipping case 3.
            self.set_step(3);
            return false;
        }
        if let Some(c) = pcard {
            let removed = self.remove_counter(c, counter_type, count);
            self.core.returns.set(i32::from(removed));
            self.set_step(3);
            return false;
        }
        self.emplace(Kind::SelectCounter {
            playerid: rplayer,
            counter_type,
            count,
            self_side,
            oppo_side,
        });
        false
    }

    /// One step of `SelectCounter` — split `count` counters across the
    /// cards that hold them.
    pub(crate) fn select_counter_step(
        &mut self,
        step: u16,
        playerid: u8,
        counter_type: u16,
        count: u16,
        self_side: bool,
        oppo_side: bool,
    ) -> bool {
        if step == 0 {
            if count == 0 {
                return true;
            }
            // **A local clamp.** `count` here is a copy; the validation
            // branch below re-reads the unit's own, unclamped value. See
            // the module note — this is reproduced, not repaired.
            let mut count = count;
            let mut p = playerid;
            let mut allowed = self_side;
            let mut total = 0u32;
            self.core.select_cards.clear();
            for _ in 0..2 {
                if allowed {
                    for loc in [location::MZONE, location::SZONE] {
                        let zone: Vec<Option<CardId>> = if loc == location::MZONE {
                            self.players[p as usize].mzone.clone()
                        } else {
                            self.players[p as usize].szone.clone()
                        };
                        for slot in zone.into_iter().flatten() {
                            let n = self.get_counter(slot, counter_type);
                            if n != 0 {
                                self.core.select_cards.push(slot);
                                total += u32::from(n);
                            }
                        }
                    }
                }
                p = 1 - p;
                allowed = oppo_side;
            }
            if u32::from(count) > total {
                count = total as u16;
            }
            if self.core.select_cards.len() == 1 {
                self.core.returns.set_i16(0, count as i16);
                return true;
            }

            let mut cards = self.core.select_cards.clone();
            cards.sort_by(|&a, &b| {
                if self.card_operation_sort(a, b) {
                    std::cmp::Ordering::Less
                } else if self.card_operation_sort(b, a) {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            });
            self.core.select_cards.clone_from(&cards);
            let offers = cards
                .iter()
                .map(|&c| CounterOffer {
                    code: self.cards[c].data.code,
                    controller: self.cards[c].current.controller,
                    location: self.cards[c].current.location,
                    sequence: self.cards[c].current.sequence as u8,
                    count: self.get_counter(c, counter_type),
                })
                .collect();
            self.messages.push(Message::SelectCounter {
                player: playerid,
                counter_type,
                count,
                cards: offers,
            });
            return false;
        }

        // Two ways to be refused, and both are retries: asking a card for
        // more counters than it holds, and a split that does not total.
        let mut given: i32 = 0;
        for i in 0..self.core.select_cards.len() {
            let asked = self.core.returns.at_i16(i);
            let c = self.core.select_cards[i];
            if (self.get_counter(c, counter_type) as i16) < asked {
                self.messages.push(Message::Retry);
                return false;
            }
            given += i32::from(asked);
        }
        if given != i32::from(count) {
            self.messages.push(Message::Retry);
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, status, Card, CardData};
    use crate::counters::counter;
    use crate::duel::phases;
    use crate::effect::{effect_type, Effect};
    use crate::processor::Status;

    const SPELL_COUNTER: u16 = 0x1;
    /// Counters go on `WITHOUT_PERMIT` so no permitting effect is needed;
    /// the reads use the same composed type, as the module warns.
    const KIND: u16 = SPELL_COUNTER | counter::WITHOUT_PERMIT;

    fn field() -> Field {
        let mut f = Field::new(8000);
        f.infos.turn_id = 3;
        f.infos.turn_player = 0;
        f.infos.phase = phases::MAIN1;
        f
    }

    /// A face-up monster of `player`'s, holding `n` counters.
    fn holder(f: &mut Field, player: u8, loc: u8, n: u16) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 50000,
                type_: card_type::MONSTER | card_type::EFFECT,
                ..Default::default()
            },
            player,
        );
        c.current.controller = player;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        let zone = if loc == location::MZONE {
            &f.players[player as usize].mzone
        } else {
            &f.players[player as usize].szone
        };
        let seat = zone.iter().position(Option::is_none).expect("a free seat") as u32;
        f.add_card(player, id, loc, seat, false);
        if n > 0 {
            assert!(f.add_counter(id, 0, KIND, n, false), "counters placed");
        }
        id
    }

    fn run(f: &mut Field) -> Status {
        for _ in 0..512 {
            match f.process() {
                Status::Continue => continue,
                other => return other,
            }
        }
        panic!("did not settle");
    }

    fn raised_remove_event(f: &Field) -> bool {
        f.core
            .point_event
            .iter()
            .chain(f.core.instant_event.iter())
            .any(|e| e.event_code == code::REMOVE_COUNTER + u32::from(KIND))
    }

    mod get_field_counter {
        use super::*;

        /// **`self` governs the asked player's side and `oppo` the
        /// other's.** The loop flips both together; flipping only the
        /// player would read `self` twice and count the wrong side.
        #[test]
        fn each_flag_governs_its_own_side() {
            let mut f = field();
            holder(&mut f, 0, location::MZONE, 3);
            holder(&mut f, 1, location::MZONE, 5);
            assert_eq!(f.get_field_counter(0, true, false, KIND), 3, "own only");
            assert_eq!(f.get_field_counter(0, false, true, KIND), 5, "theirs only");
            assert_eq!(f.get_field_counter(0, true, true, KIND), 8, "both");
            assert_eq!(f.get_field_counter(0, false, false, KIND), 0, "neither");
            // And from the other seat the two swap over.
            assert_eq!(f.get_field_counter(1, true, false, KIND), 5);
            assert_eq!(f.get_field_counter(1, false, true, KIND), 3);
        }

        /// Both rows are counted, not just the monster zone.
        #[test]
        fn the_spell_row_counts_too() {
            let mut f = field();
            holder(&mut f, 0, location::MZONE, 2);
            holder(&mut f, 0, location::SZONE, 4);
            assert_eq!(f.get_field_counter(0, true, false, KIND), 6);
        }
    }

    mod remove_counter {
        use super::*;

        /// The plain path: a named card loses its counters.
        #[test]
        fn a_named_card_loses_the_counters() {
            let mut f = field();
            let c = holder(&mut f, 0, location::MZONE, 4);
            f.remove_counter_unit(0, Some(c), 0, true, false, KIND, 3);
            run(&mut f);
            assert_eq!(f.get_counter(c, KIND), 1, "three of four gone");
            assert_eq!(f.core.returns.get(), 1, "and the unit reports success");
        }

        /// **The named-card path raises no `EVENT_REMOVE_COUNTER`.**
        ///
        /// `arg.step = 3` plus the increment lands on case 4, skipping
        /// case 3 — the only place the event is raised. This reads like a
        /// slip and is the reference's behaviour, so it is pinned: a port
        /// that "fixed" it would diverge silently.
        #[test]
        fn the_named_card_path_skips_the_event() {
            let mut f = field();
            let c = holder(&mut f, 0, location::MZONE, 4);
            f.remove_counter_unit(0, Some(c), 0, true, false, KIND, 2);
            // Step until the unit is gone, checking at every step — the
            // event would be drained by `process_instant_event` if it were
            // ever raised, so a check only at the end proves nothing.
            let mut seen = false;
            for _ in 0..64 {
                if raised_remove_event(&f) {
                    seen = true;
                }
                if f.process() != Status::Continue {
                    break;
                }
            }
            assert!(!seen, "no EVENT_REMOVE_COUNTER on the named-card path");
            assert_eq!(f.get_counter(c, KIND), 2, "the counters did go");
        }

        /// **A card without enough counters is not offered the plain
        /// removal**, and with nothing else on offer the unit ends at once.
        #[test]
        fn too_few_counters_offers_nothing() {
            let mut f = field();
            let c = holder(&mut f, 0, location::MZONE, 1);
            f.core.returns.set(-99);
            f.remove_counter_unit(0, Some(c), 0, true, false, KIND, 3);
            run(&mut f);
            assert_eq!(f.get_counter(c, KIND), 1, "nothing was removed");
            assert_eq!(f.core.returns.get(), 0, "and zero is written, not left");
        }

        /// An empty field offers nothing either, by the other branch of
        /// the same check.
        #[test]
        fn an_empty_field_offers_nothing() {
            let mut f = field();
            f.core.returns.set(-99);
            f.remove_counter_unit(0, None, 0, true, true, KIND, 1);
            assert_eq!(
                run(&mut f),
                Status::End,
                "the unit ended rather than asking"
            );
            assert_eq!(f.core.returns.get(), 0);
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::SelectCounter { .. })),
                "and nobody was asked to split nothing"
            );
        }

        /// **The option list is exactly the plain removal** when no
        /// replacing effect applies — one option, so nothing is asked.
        #[test]
        fn one_option_asks_nothing() {
            let mut f = field();
            let c = holder(&mut f, 0, location::MZONE, 4);
            f.remove_counter_unit(0, Some(c), 0, true, false, KIND, 2);
            assert_eq!(f.process(), Status::Continue);
            assert_eq!(f.core.select_options, vec![10]);
            assert_eq!(f.core.select_effects, vec![None]);
            assert!(
                !f.core
                    .units
                    .iter()
                    .chain(f.core.subunits.iter())
                    .any(|u| matches!(
                        u.kind,
                        Kind::SelectOption { .. } | Kind::SelectEffectYesNo { .. }
                    )),
                "nothing to choose between"
            );
        }

        /// **The replacement lookup is by base + counter type.** An effect
        /// registered for a *different* counter type is not offered, which
        /// is what a lookup on the bare `RCOUNTER_REPLACE` would get wrong.
        #[test]
        fn the_replacement_is_found_by_base_plus_counter_type() {
            for (registered, offered) in [(KIND, true), (KIND + 1, false)] {
                let mut f = field();
                let c = holder(&mut f, 0, location::MZONE, 4);
                let source = holder(&mut f, 0, location::MZONE, 0);
                let mut e = Effect::new(
                    effect_type::CONTINUOUS | effect_type::ACTIONS,
                    code::RCOUNTER_REPLACE + u32::from(registered),
                );
                e.owner = Some(source);
                e.handler = Some(source);
                e.effect_owner = 0;
                e.description = 77;
                e.range = u16::from(location::MZONE);
                let id = f.new_effect(e);
                f.field_effects
                    .continuous
                    .insert(code::RCOUNTER_REPLACE + u32::from(registered), id);
                f.field_effects.indexer.insert(id);

                f.remove_counter_unit(0, Some(c), 0, true, false, KIND, 2);
                assert_eq!(f.process(), Status::Continue);
                assert_eq!(
                    f.core.select_effects.contains(&Some(id)),
                    offered,
                    "registered for {registered:#x}"
                );
            }
        }
    }

    mod is_player_can_remove_counter {
        use super::*;

        /// Register a replacement effect for `registered`, and return
        /// its id.
        fn replacement(f: &mut Field, registered: u16) -> crate::event::EffectId {
            let source = holder(f, 0, location::MZONE, 0);
            let mut e = Effect::new(
                effect_type::CONTINUOUS | effect_type::ACTIONS,
                code::RCOUNTER_REPLACE + u32::from(registered),
            );
            e.owner = Some(source);
            e.handler = Some(source);
            e.effect_owner = 0;
            e.description = 77;
            e.range = u16::from(location::MZONE);
            let id = f.new_effect(e);
            f.field_effects
                .continuous
                .insert(code::RCOUNTER_REPLACE + u32::from(registered), id);
            f.field_effects.indexer.insert(id);
            id
        }

        /// **Having enough is one way to be able to remove them.**
        #[test]
        fn enough_counters_is_enough() {
            let mut f = field();
            let c = holder(&mut f, 0, location::MZONE, 2);
            assert!(f.is_player_can_remove_counter(0, Some(c), false, false, KIND, 2, 0));
            assert!(f.is_player_can_remove_counter(0, Some(c), false, false, KIND, 1, 0));
            assert!(!f.is_player_can_remove_counter(0, Some(c), false, false, KIND, 3, 0));
        }

        /// **And a replacement effect is the other** — a cost is payable
        /// if *something* can pay it, which is the same shape
        /// `check_lp_cost` has.
        ///
        /// Registered for the wrong counter type it is not consulted,
        /// which is what a lookup on the bare `RCOUNTER_REPLACE` would
        /// get wrong.
        #[test]
        fn a_replacement_effect_makes_it_payable() {
            for (registered, payable) in [(KIND, true), (KIND + 1, false)] {
                let mut f = field();
                let c = holder(&mut f, 0, location::MZONE, 0);
                assert!(
                    !f.is_player_can_remove_counter(0, Some(c), false, false, KIND, 2, 0),
                    "no counters and nothing to replace with"
                );
                replacement(&mut f, registered);
                assert_eq!(
                    f.is_player_can_remove_counter(0, Some(c), false, false, KIND, 2, 0),
                    payable,
                    "registered for {registered:#x}"
                );
            }
        }

        /// **Naming a card asks about that card; naming none asks about
        /// the field.** A card with nothing on it does not borrow from
        /// the row.
        #[test]
        fn one_card_is_not_the_field() {
            let mut f = field();
            let empty = holder(&mut f, 0, location::MZONE, 0);
            holder(&mut f, 0, location::MZONE, 3);
            assert!(
                !f.is_player_can_remove_counter(0, Some(empty), false, false, KIND, 2, 0),
                "this card has none"
            );
            assert!(
                f.is_player_can_remove_counter(0, None, true, false, KIND, 2, 0),
                "but the row does"
            );
        }
    }

    /// Drive a `RemoveCounter` that has to ask, answering the split.
    /// Returns whether `EVENT_REMOVE_COUNTER + KIND` was ever visible.
    fn run_field_removal(f: &mut Field, split: &[i16]) -> bool {
        let mut seen = false;
        for _ in 0..128 {
            if raised_remove_event(f) {
                seen = true;
            }
            match f.process() {
                Status::Continue => continue,
                Status::Awaiting => {
                    for (i, &n) in split.iter().enumerate() {
                        f.core.returns.set_i16(i, n);
                    }
                }
                Status::End => break,
            }
        }
        seen
    }

    mod case_two {
        use super::*;

        /// **Each card is asked for its own share.** Indexing the answer
        /// but not the card list takes every count off the first card,
        /// which a one-card or an even split could never show.
        #[test]
        fn the_split_is_applied_card_by_card() {
            let mut f = field();
            let a = holder(&mut f, 0, location::MZONE, 4);
            let b = holder(&mut f, 1, location::MZONE, 4);
            f.core.select_cards = vec![a, b];
            f.core.returns.set_i16(0, 1);
            f.core.returns.set_i16(1, 3);
            assert!(!f.remove_counter_step(2, 0, None, 0, true, true, KIND, 4));
            assert_eq!(f.get_counter(a, KIND), 3, "the first lost one");
            assert_eq!(f.get_counter(b, KIND), 1, "the second lost three");
        }

        /// **A zero share touches its card at all.** `remove_counter`
        /// announces even a removal of nothing, so letting a zero through
        /// emits a message for a card that lost nothing.
        #[test]
        fn a_zero_share_is_skipped_entirely() {
            let mut f = field();
            let a = holder(&mut f, 0, location::MZONE, 4);
            let b = holder(&mut f, 1, location::MZONE, 4);
            f.core.select_cards = vec![a, b];
            f.core.returns.set_i16(0, 2);
            f.core.returns.set_i16(1, 0);
            let before = f.messages.len();
            assert!(!f.remove_counter_step(2, 0, None, 0, true, true, KIND, 2));
            let announced = f.messages[before..]
                .iter()
                .filter(|m| matches!(m, Message::RemoveCounter { .. }))
                .count();
            assert_eq!(announced, 1, "one card was touched, not two");
            assert_eq!(f.get_counter(b, KIND), 4);
        }
    }

    mod the_replacing_path {
        use super::*;

        /// Register a replacement for `KIND` and return its id.
        fn replacement(f: &mut Field) -> EffectId {
            let source = holder(f, 0, location::MZONE, 0);
            let mut e = Effect::new(
                effect_type::CONTINUOUS | effect_type::ACTIONS,
                code::RCOUNTER_REPLACE + u32::from(KIND),
            );
            e.owner = Some(source);
            e.handler = Some(source);
            e.effect_owner = 0;
            e.description = 77;
            e.range = u16::from(location::MZONE);
            let id = f.new_effect(e);
            f.field_effects
                .continuous
                .insert(code::RCOUNTER_REPLACE + u32::from(KIND), id);
            f.field_effects.indexer.insert(id);
            id
        }

        /// **Taking a replacement leaves the counters alone** and, like
        /// the named-card path, raises no event: `arg.step = 3` lands on
        /// case 4 and skips case 3.
        #[test]
        fn a_taken_replacement_removes_nothing_and_raises_nothing() {
            let mut f = field();
            let c = holder(&mut f, 0, location::MZONE, 4);
            replacement(&mut f);
            f.remove_counter_unit(0, Some(c), 0, true, false, KIND, 2);
            // Two options, so a yes/no is asked; answer **yes** to take
            // the replacement, which is index 1 in the option list.
            let mut seen = false;
            for _ in 0..128 {
                if raised_remove_event(&f) {
                    seen = true;
                }
                match f.process() {
                    Status::Continue => continue,
                    Status::Awaiting => f.core.returns.set(1),
                    Status::End => break,
                }
            }
            assert!(!seen, "the replacing path raises no event either");
            assert_eq!(f.get_counter(c, KIND), 4, "and removes nothing itself");
        }

        /// **Three options are a menu, not a yes/no.** The yes/no shape is
        /// for exactly one alternative to the plain removal.
        #[test]
        fn two_replacements_make_it_a_menu() {
            let mut f = field();
            let c = holder(&mut f, 0, location::MZONE, 4);
            replacement(&mut f);
            replacement(&mut f);
            f.remove_counter_unit(0, Some(c), 0, true, false, KIND, 2);
            assert_eq!(f.process(), Status::Continue);
            assert_eq!(f.core.select_options.len(), 3);
            let queued = |f: &Field, pred: fn(&Kind) -> bool| {
                f.core
                    .units
                    .iter()
                    .chain(f.core.subunits.iter())
                    .any(|u| pred(&u.kind))
            };
            assert!(
                queued(&f, |k| matches!(k, Kind::SelectOption { .. })),
                "a menu"
            );
            assert!(
                !queued(&f, |k| matches!(k, Kind::SelectEffectYesNo { .. })),
                "not a yes/no"
            );
        }
    }

    mod the_field_path {
        use super::*;

        /// **The field path does reach case 3, and raises the event with
        /// the counter type added on.** The counterpart to
        /// `the_named_card_path_skips_the_event`: without this, dropping
        /// the counter type from the raised code changes nothing any test
        /// can see.
        #[test]
        fn it_raises_the_event_for_this_counter_type() {
            let mut f = field();
            holder(&mut f, 0, location::MZONE, 2);
            holder(&mut f, 1, location::MZONE, 2);
            f.remove_counter_unit(0, None, 0, true, true, KIND, 3);
            assert!(
                run_field_removal(&mut f, &[2, 1]),
                "EVENT_REMOVE_COUNTER + KIND was raised"
            );
        }

        /// And the counters actually go.
        #[test]
        fn the_chosen_counters_are_taken() {
            let mut f = field();
            let a = holder(&mut f, 0, location::MZONE, 2);
            let b = holder(&mut f, 1, location::MZONE, 2);
            f.remove_counter_unit(0, None, 0, true, true, KIND, 3);
            run_field_removal(&mut f, &[2, 1]);
            assert_eq!(
                f.get_counter(a, KIND) + f.get_counter(b, KIND),
                1,
                "three of the four are gone"
            );
        }
    }

    mod select_counter {
        use super::*;

        /// **A card holding none is not offered.** Offering it would put a
        /// slot in the answer that can only ever be zero, and shift every
        /// later card's index.
        #[test]
        fn a_card_with_no_counters_is_not_offered() {
            let mut f = field();
            holder(&mut f, 0, location::MZONE, 3);
            holder(&mut f, 0, location::MZONE, 0);
            holder(&mut f, 1, location::MZONE, 3);
            assert!(!f.select_counter_step(0, 0, KIND, 4, true, true));
            assert_eq!(f.core.select_cards.len(), 2, "only the two holders");
            match f.messages.last() {
                Some(Message::SelectCounter { cards, .. }) => assert_eq!(cards.len(), 2),
                other => panic!("expected a counter question, got {other:?}"),
            }
        }

        /// **`self` and `oppo` are read on their own passes.** With only
        /// the opponent's side allowed, this player's holders are not
        /// offered — which a both-sides test cannot show.
        #[test]
        fn the_two_side_flags_are_not_interchangeable() {
            let mut f = field();
            let mine = holder(&mut f, 0, location::MZONE, 3);
            let theirs = holder(&mut f, 1, location::MZONE, 3);
            let other = holder(&mut f, 1, location::MZONE, 2);

            f.select_counter_step(0, 0, KIND, 4, false, true);
            let offered = f.core.select_cards.clone();
            assert!(!offered.contains(&mine), "my own side was excluded");
            assert!(offered.contains(&theirs) && offered.contains(&other));
        }

        /// A single holder answers for itself — no question.
        #[test]
        fn one_holder_answers_for_itself() {
            let mut f = field();
            holder(&mut f, 0, location::MZONE, 5);
            assert!(f.select_counter_step(0, 0, KIND, 3, true, false));
            assert_eq!(f.core.returns.at_i16(0), 3);
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::SelectCounter { .. })),
                "nothing was asked"
            );
        }

        /// **The clamp is to what is actually there.** Asking for more
        /// than the field holds answers with the total, not the ask.
        #[test]
        fn a_single_holder_clamps_to_what_it_has() {
            let mut f = field();
            holder(&mut f, 0, location::MZONE, 2);
            assert!(f.select_counter_step(0, 0, KIND, 7, true, false));
            assert_eq!(f.core.returns.at_i16(0), 2, "clamped to the total");
        }

        /// A count of zero is not a question either.
        #[test]
        fn a_count_of_zero_asks_nothing() {
            let mut f = field();
            holder(&mut f, 0, location::MZONE, 3);
            holder(&mut f, 1, location::MZONE, 3);
            assert!(f.select_counter_step(0, 0, KIND, 0, true, true));
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::SelectCounter { .. })),
                "nothing was asked (placing the counters logs its own messages)"
            );
        }

        /// Two holders means a question, carrying each card's own total.
        #[test]
        fn two_holders_are_offered_with_their_counts() {
            let mut f = field();
            holder(&mut f, 0, location::MZONE, 2);
            holder(&mut f, 1, location::MZONE, 5);
            assert!(!f.select_counter_step(0, 0, KIND, 4, true, true));
            match f.messages.last() {
                Some(Message::SelectCounter {
                    player,
                    counter_type,
                    count,
                    cards,
                }) => {
                    assert_eq!(*player, 0);
                    assert_eq!(*counter_type, KIND);
                    assert_eq!(*count, 4);
                    let mut totals: Vec<u16> = cards.iter().map(|c| c.count).collect();
                    totals.sort_unstable();
                    assert_eq!(totals, vec![2, 5]);
                }
                other => panic!("expected a counter question, got {other:?}"),
            }
        }

        /// **A split that does not total is refused**, and so is asking a
        /// card for more than it holds — both as retries.
        #[test]
        fn an_invalid_split_is_refused() {
            let setup = || {
                let mut f = field();
                let a = holder(&mut f, 0, location::MZONE, 2);
                let b = holder(&mut f, 1, location::MZONE, 5);
                f.core.select_cards = vec![a, b];
                f
            };
            // Totals 3, not 4.
            let mut f = setup();
            f.core.returns.set_i16(0, 1);
            f.core.returns.set_i16(1, 2);
            assert!(!f.select_counter_step(1, 0, KIND, 4, true, true));
            assert_eq!(f.messages.last(), Some(&Message::Retry));

            // Totals 4, but asks the first card for more than its two.
            let mut f = setup();
            f.core.returns.set_i16(0, 3);
            f.core.returns.set_i16(1, 1);
            assert!(!f.select_counter_step(1, 0, KIND, 4, true, true));
            assert_eq!(f.messages.last(), Some(&Message::Retry));

            // And a legal split is accepted.
            let mut f = setup();
            f.core.returns.set_i16(0, 2);
            f.core.returns.set_i16(1, 2);
            assert!(f.select_counter_step(1, 0, KIND, 4, true, true));
            assert_ne!(f.messages.last(), Some(&Message::Retry));
        }

        /// **The clamp does not survive into the validation.**
        ///
        /// Case 0 clamps a *local* copy and shows the clamped number;
        /// the check re-reads the unit's untouched `count`. With two
        /// holders and an ask above the field's total, answering what was
        /// shown is refused. Pinned because it is the reference's, not
        /// because it is right — see the module note.
        #[test]
        fn the_clamp_is_lost_between_the_question_and_the_check() {
            let mut f = field();
            let a = holder(&mut f, 0, location::MZONE, 1);
            let b = holder(&mut f, 1, location::MZONE, 1);
            // Ask for 5; the field holds 2.
            assert!(!f.select_counter_step(0, 0, KIND, 5, true, true));
            let shown = match f.messages.last() {
                Some(Message::SelectCounter { count, .. }) => *count,
                other => panic!("expected a counter question, got {other:?}"),
            };
            assert_eq!(shown, 2, "the player is shown the clamped total");

            f.core.select_cards = vec![a, b];
            f.core.returns.set_i16(0, 1);
            f.core.returns.set_i16(1, 1);
            // Answering exactly what was shown is refused, because the
            // check compares against the unclamped 5.
            assert!(!f.select_counter_step(1, 0, KIND, 5, true, true));
            assert_eq!(
                f.messages.last(),
                Some(&Message::Retry),
                "the answer the question asked for is rejected"
            );
        }
    }
}
