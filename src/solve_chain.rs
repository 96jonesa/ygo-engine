//! `SolveChain`: resolving the chain, link by link, innermost first.
//!
//! A translation of `field::process(Processors::SolveChain&)`. Ten numbered
//! steps of which four — 6 through 9 — are never case labels, and the unit
//! loops rather than running once: step 10 resolves a link, pops it, and
//! assigns `RESTART` to begin the next from step 0.
//!
//! ```text
//!          ┌──────────────── more links ────────────────┐
//!          │                                            │
//! 0 ─> 1 ─> 2 ─> 3 ─> 4 ─> 5 ──(step = 9)──> 10 ──(RESTART)
//!      │    │                                 │
//!      │    └──(disabled / gone: step = 3)──> 4
//!      └──(negated: step = 9)─────────────────┘
//!                                             │
//!                               (stack empty) └─> 11 ─> 12 ─> 13 (done)
//! ```
//!
//! Two jumps are worth naming because they skip work rather than reordering
//! it. A negated *activation* jumps from step 1 straight to 10: no operation
//! runs and no `EVENT_CHAIN_SOLVING` is raised. A negated *effect* jumps from
//! step 2 to 4, skipping only step 3 — so the operation does not run, but
//! everything after it still does.
//!
//! The tail, 11 to 13, runs **once per chain** rather than per link: cards
//! confirmed to be leaving are sent away, the chain is reset, and
//! `EVENT_CHAIN_END` is raised. That is why the loop at step 10 has to fall
//! out of itself rather than the unit simply retiring.
//!
//! ## What is not ported
//!
//! Same rule as `AddChain`: where a subsystem does not exist, the call is a
//! named function that **panics**. Nothing can reach them — no duel starts.
//! `docs/processor-loop.md` carries the list.

use crate::card::{card_type, status};
use crate::chain::chain_flag;
use crate::duel::flags;
use crate::effect::{effect_type, flag};
use crate::event::code;
use crate::field::{timing, Field, Message};
use crate::processor::{Kind, RESTART};

impl Field {
    /// One step of `SolveChain`. Returns true when the unit is finished.
    pub(crate) fn solve_chain_step(&mut self, step: u16, skip: SolveChainSkip) -> bool {
        // The reference takes a reverse iterator once at the top and every
        // case uses it; an empty stack at step 0 is the only case where it
        // would be invalid, and that returns immediately.
        if self.core.current_chain.is_empty() && step == 0 {
            return true;
        }

        match step {
            0 => {
                self.restore_spsummon_counters();
                let chain = self.core.current_chain.last().unwrap().clone();
                self.messages.push(Message::ChainSolving {
                    chain_count: chain.chain_count,
                });
                if let Some(handler) = self
                    .effects
                    .get(chain.triggering_effect)
                    .and_then(|e| e.get_handler(&self.cards))
                {
                    self.add_to_disable_check_list(handler);
                }
                self.adjust_instant();
                self.raise_event(
                    None,
                    code::CHAIN_ACTIVATING,
                    Some(chain.triggering_effect),
                    0,
                    chain.triggering_player,
                    chain.triggering_player,
                    u32::from(chain.chain_count),
                );
                self.process_instant_event();
                false
            }

            1 => {
                let last = self.core.current_chain.len() - 1;
                let chain = self.core.current_chain[last].clone();
                let effect = chain.triggering_effect;

                // A negated *activation*: nothing of the effect happens, and
                // the chain link is unwound rather than resolved.
                if chain.flag & chain_flag::DISABLE_ACTIVATE != 0
                    && self.is_chain_negatable(chain.chain_count)
                {
                    self.remove_oath_effect(effect);
                    let refundable = self
                        .effects
                        .get(effect)
                        .is_some_and(|e| e.is_flag(flag::COUNT_LIMIT) && e.count_flag & 0x1 != 0);
                    if refundable {
                        let (c, f, h) = self
                            .effects
                            .get(effect)
                            .map(|e| (e.count_code, e.count_flag, e.count_hopt_index))
                            .unwrap();
                        self.dec_effect_code(c, f, h, chain.triggering_player);
                    }
                    self.restore_chain_counter(chain.triggering_player, last);

                    // Anything that was going to trigger *off this
                    // activation* is dropped: it never happened.
                    let count = u32::from(chain.chain_count);
                    let dropped = |c: &crate::chain::Chain| {
                        c.evt.event_code == code::CHAINING && c.evt.event_value == count
                    };
                    self.core.new_fchain.retain(|c| !dropped(c));
                    self.core.new_ochain.retain(|c| !dropped(c));

                    self.raise_event(
                        None,
                        code::CHAIN_NEGATED,
                        Some(effect),
                        0,
                        chain.triggering_player,
                        chain.triggering_player,
                        count,
                    );
                    self.process_instant_event();
                    self.set_step(9);
                    return false;
                }

                self.core.current_chain[last].applied_chain_counters = None;
                self.release_oath_relation(effect);
                self.break_effect(true);
                self.core.chain_solving = true;
                self.raise_event(
                    None,
                    code::CHAIN_SOLVING,
                    Some(effect),
                    0,
                    chain.triggering_player,
                    chain.triggering_player,
                    u32::from(chain.chain_count),
                );
                self.process_instant_event();
                false
            }

            2 => {
                self.core.spsummon_state_count_tmp = self.core.spsummon_state_count;
                let last = self.core.current_chain.len() - 1;
                let chain = self.core.current_chain[last].clone();
                let effect = chain.triggering_effect;
                let Some(handler) = self
                    .effects
                    .get(effect)
                    .and_then(|e| e.get_handler(&self.cards))
                else {
                    return false;
                };

                // A continuous spell or trap that has left the field takes
                // its effect with it.
                if chain.flag & chain_flag::CONTINUOUS_CARD != 0
                    && !self.cards[handler].has_chain_relation(effect, chain.chain_id)
                {
                    self.set_step(3);
                    return false;
                }

                // The card physically turns face-up now, which is when its
                // continuous effects begin to apply.
                let is_activate = self
                    .effects
                    .get(effect)
                    .is_some_and(|e| e.is_type(effect_type::ACTIVATE));
                if is_activate
                    && self.cards[handler].has_chain_relation(effect, chain.chain_id)
                    && chain.replace_op.is_none()
                {
                    self.enable_field_effect(handler, true);
                    self.adjust_instant();
                }

                // A negated *effect*. Note the exemption: an effect that
                // creates a continuous target runs its operation even when
                // disabled, because the target has to exist for anything
                // later to refer to.
                let creates_continuous_target = self
                    .effects
                    .get(effect)
                    .is_some_and(|e| e.is_flag(flag::CONTINUOUS_TARGET));
                if self.is_chain_disablable(chain.chain_count)
                    && (!creates_continuous_target || chain.replace_op.is_some())
                {
                    let disabled = self.is_chain_disabled(chain.chain_count)
                        || (self.cards[handler].get_status(status::DISABLED | status::FORBIDDEN)
                            && self.cards[handler].has_chain_relation(effect, chain.chain_id));
                    if disabled {
                        if chain.flag & chain_flag::DISABLE_EFFECT == 0 {
                            self.messages.push(Message::ChainDisabled {
                                chain_count: chain.chain_count,
                            });
                        }
                        self.raise_event(
                            None,
                            code::CHAIN_DISABLED,
                            Some(effect),
                            0,
                            chain.triggering_player,
                            chain.triggering_player,
                            u32::from(chain.chain_count),
                        );
                        self.process_instant_event();
                        self.set_step(3);
                        return false;
                    }
                }
                false
            }

            3 => {
                let last = self.core.current_chain.len() - 1;
                let chain = self.core.current_chain[last].clone();
                let effect = chain.triggering_effect;
                let Some(handler) = self
                    .effects
                    .get(effect)
                    .and_then(|e| e.get_handler(&self.cards))
                else {
                    return false;
                };
                if chain.flag & chain_flag::CONTINUOUS_CARD != 0
                    && !self.cards[handler].has_chain_relation(effect, chain.chain_id)
                {
                    return false;
                }

                // A replacement operation is swapped in for the duration and
                // swapped back at step 4 — the effect itself is not changed.
                if let Some(replacement) = chain.replace_op {
                    if let Some(e) = self.effects.get_mut(effect) {
                        self.core.backed_up_operation = e.operation;
                        e.operation = Some(replacement);
                    }
                } else {
                    self.core.backed_up_operation = None;
                }
                if self
                    .effects
                    .get(effect)
                    .is_some_and(|e| e.operation.is_some())
                {
                    self.core.sub_solving_event.push_back(chain.evt);
                    self.emplace(Kind::ExecuteOperation {
                        resume: None,
                        effect,
                        player: chain.triggering_player,
                        subject: None,
                        args: Vec::new(),
                        was_disabled: false,
                    });
                }
                false
            }

            4 => {
                let effect = self.core.current_chain.last().unwrap().triggering_effect;
                if let Some(original) = self.core.backed_up_operation {
                    if let Some(e) = self.effects.get_mut(effect) {
                        e.operation = Some(original);
                    }
                }
                self.core.special_summoning.clear();
                self.core.equiping_cards.clear();
                false
            }

            5 => {
                // The reference's `std::exchange(backed_up_operation, 0) == 0`
                // both reads and clears: the special-summon bookkeeping runs
                // only when no replacement operation was in force, because a
                // replaced operation's summons are the replacer's business.
                let had_replacement = self.core.backed_up_operation.take().is_some();
                if !had_replacement {
                    self.count_special_summons();
                }
                self.core.spsummon_state_count_tmp = [0, 0];
                self.core.chain_solving = false;

                // A delayed continuous effect now gets its turn, turn
                // player first.
                if !self.core.delayed_continuous_tp.is_empty() {
                    self.core.conti_player = self.infos.turn_player;
                    if let Some(c) = self.core.delayed_continuous_tp.pop_front() {
                        self.core.sub_solving_continuous.push_back(c);
                    }
                    self.emplace(Kind::SolveContinuous {
                        state: Default::default(),
                    });
                } else if !self.core.delayed_continuous_ntp.is_empty() {
                    self.core.conti_player = 1 - self.infos.turn_player;
                    if let Some(c) = self.core.delayed_continuous_ntp.pop_front() {
                        self.core.sub_solving_continuous.push_back(c);
                    }
                    self.emplace(Kind::SolveContinuous {
                        state: Default::default(),
                    });
                } else {
                    self.core.conti_player = crate::event::PLAYER_NONE;
                }

                let chain = self.core.current_chain.last().unwrap().clone();
                self.raise_event(
                    None,
                    code::CHAIN_SOLVED,
                    Some(chain.triggering_effect),
                    0,
                    chain.triggering_player,
                    chain.triggering_player,
                    u32::from(chain.chain_count),
                );
                self.adjust_disable_check_list();
                self.process_instant_event();
                self.set_step(9);
                false
            }

            10 => {
                let chain = self.core.current_chain.last().unwrap().clone();
                self.messages.push(Message::ChainSolved {
                    chain_count: chain.chain_count,
                });
                let effect = chain.triggering_effect;
                let Some(handler) = self
                    .effects
                    .get(effect)
                    .and_then(|e| e.get_handler(&self.cards))
                else {
                    return true;
                };

                // The ACTIVATE type granted at `AddChain` step 5 is given
                // back, so the effect is a plain trigger again.
                if chain.flag & chain_flag::ACTIVATING != 0 {
                    if let Some(e) = self.effects.get_mut(effect) {
                        if e.is_type(effect_type::ACTIVATE) {
                            e.effect_type &= !effect_type::ACTIVATE;
                        }
                    }
                }
                // A hand effect that stayed hidden is shuffled back, so the
                // opponent learns nothing from its position.
                if chain.flag & chain_flag::HAND_EFFECT != 0
                    && !self.cards[handler].current.is_faceup()
                    && self.cards[handler].current.location == crate::board::location::HAND
                {
                    self.shuffle_hand(self.cards[handler].current.controller);
                }
                for &target in &chain.target_cards {
                    self.cards[target].release_chain_relation(effect, chain.chain_id);
                }
                // An equip that never found something to equip leaves.
                if self.cards[handler].data.is_type(card_type::EQUIP)
                    && self
                        .effects
                        .get(effect)
                        .is_some_and(|e| e.is_type(effect_type::ACTIVATE))
                    && self.cards[handler].equiping_target.is_none()
                    && self.cards[handler].has_chain_relation(effect, chain.chain_id)
                {
                    self.cards[handler].set_status(status::LEAVE_CONFIRMED, true);
                }

                if let Some(e) = self.effects.get_mut(effect) {
                    e.active_type = 0;
                    e.active_handler = None;
                }
                self.cards[handler].release_chain_relation(effect, chain.chain_id);

                // Effects held back while the chain resolved come on now,
                // but only for cards still on the field.
                let delayed = std::mem::take(&mut self.core.delayed_enable_set);
                for card in delayed {
                    if self.cards[card].current.location == crate::board::location::MZONE {
                        self.enable_field_effect(card, true);
                    }
                }
                self.adjust_all();

                self.core.current_chain.pop();
                self.core.real_chain_count -= 1;
                if self.core.real_chain_count < 0 {
                    self.core.real_chain_count = 0;
                }
                if self.core.current_chain.is_empty() {
                    self.core.chain_limit.clear();
                    // Fall through to the once-per-chain tail.
                    return false;
                }
                // The reference restores `core.reserved` here too, gated on
                // `core.summoning_card || core.summoning_proc_group_type`.
                // Neither is ported — no summon-in-progress marker exists —
                // so the only live restore point is case 12 below, which is
                // gated on `effect_damage_step` as well. Noted rather than
                // silently dropped: when the summon marker lands, this is
                // the second place it has to be read.

                // More links: begin the next from step 0.
                self.set_step(RESTART);
                false
            }

            11 => {
                // A card may have been marked to leave and then saved, so
                // the set is re-checked rather than trusted.
                let confirmed: Vec<usize> = std::mem::take(&mut self.core.leave_confirmed)
                    .into_iter()
                    .filter(|&c| self.cards[c].is_status(status::LEAVE_CONFIRMED))
                    .collect();
                if !confirmed.is_empty() {
                    self.send_to_grave(confirmed);
                }
                false
            }

            12 => {
                let point = std::mem::take(&mut self.core.point_event);
                self.core.used_event.extend(point);
                self.messages.push(Message::ChainEnd);
                self.reset_chain();
                // **The chain is over, so a unit that stepped aside for it
                // comes back.** `DamageStep`'s case 1 parks itself in
                // `core.reserved` and reports finished; this is where it is
                // put back on the queue.
                //
                // The reference also gates this on the summon-in-progress
                // markers, which are not ported. `effect_damage_step` is
                // the condition that matters here.
                if self.core.effect_damage_step == 1 {
                    self.restore_reserved_unit();
                }
                false
            }

            13 => {
                self.core.just_sent_cards.clear();
                self.raise_event(None, code::CHAIN_END, None, 0, 0, 0, 0);
                self.process_instant_event();
                self.adjust_all();
                // The window after a chain ends. Opened unless the caller
                // asked for neither triggers nor new activations — note the
                // `||`: one of the two being wanted is enough.
                if !skip.trigger || !skip.new {
                    self.core.hint_timing[0] |= timing::CHAIN_END;
                    self.core.hint_timing[1] |= timing::CHAIN_END;
                    self.emplace(Kind::PointEvent {
                        skip: crate::point_event::PointEventSkip {
                            trigger: skip.trigger,
                            // Not `skip.freechain`: the reference widens it
                            // by `skip_new` at this call site.
                            freechain: skip.freechain || skip.new,
                            new: skip.new,
                        },
                    });
                }
                self.core.returns.set(1);
                true
            }

            _ => true,
        }
    }

    /// Give back what a chain's Special Summons cost, at the top of the
    /// chain that is about to resolve.
    ///
    /// **Both branches are switched off by this project's duel options**, and
    /// so is the flag that reaches them — `spsummon_rst` is only ever set
    /// under `DUEL_CANNOT_SUMMON_OATH_OLD`. It is ported rather than left as
    /// a panic because the branch becomes reachable the moment the option
    /// changes, and because the shape of the give-back is the only place the
    /// `_rst` halves of the two counters are read.
    ///
    /// Note that the flag is cleared **inside** the first branch, not
    /// alongside it: with the old-oath option off, `spsummon_rst` would stay
    /// set — which is consistent, because nothing sets it in that
    /// configuration either.
    fn restore_spsummon_counters(&mut self) {
        if !self.core.spsummon_rst {
            return;
        }
        if self.is_flag(flags::CANNOT_SUMMON_OATH_OLD) {
            self.set_spsummon_counter(0, false, true);
            self.set_spsummon_counter(1, false, true);
            self.core.spsummon_rst = false;
        }
        if self.is_flag(flags::SPSUMMON_ONCE_OLD_NEGATE) {
            for plr in 0..2 {
                let codes: Vec<u32> = self.core.spsummon_once_map[plr].keys().copied().collect();
                for spcode in codes {
                    let back = self.core.spsummon_once_map_rst[plr]
                        .get(&spcode)
                        .copied()
                        .unwrap_or(0);
                    if let Some(n) = self.core.spsummon_once_map[plr].get_mut(&spcode) {
                        *n -= back;
                    }
                    self.core.spsummon_once_map_rst[plr].insert(spcode, 0);
                }
            }
        }
    }

    /// Case 5's special-summon bookkeeping, from the link's declared
    /// `CATEGORY_SPECIAL_SUMMON` operation info.
    ///
    /// Everything inside is gated on `DUEL_CANNOT_SUMMON_OATH_OLD` and,
    /// within that, `DUEL_SPSUMMON_ONCE_OLD_NEGATE` — neither of which this
    /// configuration sets, so under MR5 the function reads the info and
    /// does nothing. Ported in full rather than stubbed: the primitives it
    /// needs exist, and a configuration that turns the flags on should
    /// find the behaviour rather than a panic.
    ///
    /// Two shapes of declaration. `PLAYER_ALL` means "both players summon,
    /// one card each, `op_param` says which player the first belongs to";
    /// otherwise every declared card is summoned by the activating player
    /// — except that a card-targeting effect may say `op_player == 0x10`
    /// to mean the *opponent* summons the targets.
    fn count_special_summons(&mut self) {
        let Some(link) = self.core.current_chain.last() else {
            return;
        };
        let Some(info) = link
            .opinfos
            .get(&crate::event::category::SPECIAL_SUMMON)
            .cloned()
        else {
            return;
        };
        if info.count == 0 {
            return;
        }
        let tp = link.triggering_player;
        let effect = link.triggering_effect;
        let old_oath = self.is_flag(crate::duel::flags::CANNOT_SUMMON_OATH_OLD);
        let once_negate = self.is_flag(crate::duel::flags::SPSUMMON_ONCE_OLD_NEGATE)
            && self.core.global_flag & crate::field::global_flag::SPSUMMON_ONCE != 0;

        if old_oath {
            if self.core.spsummon_state_count_tmp[tp as usize]
                == self.core.spsummon_state_count[tp as usize]
            {
                self.set_spsummon_counter(tp, true, false);
            }
            let other = (1 - tp) as usize;
            if info.player == crate::event::PLAYER_ALL
                && self.core.spsummon_state_count_tmp[other]
                    == self.core.spsummon_state_count[other]
            {
                self.set_spsummon_counter(1 - tp, true, false);
            }
        }
        if old_oath {
            let Some(cards) = info.cards else {
                return;
            };
            if info.player == crate::event::PLAYER_ALL {
                // Two cards, one per player; `op_param` is the first's player.
                let sumplayer = info.param as u8;
                let (a, b) = (cards[0], cards[1]);
                if once_negate {
                    for (c, p) in [(a, sumplayer), (b, 1 - sumplayer)] {
                        let spcode = self.cards[c].spsummon_code;
                        if spcode != 0 {
                            *self.core.spsummon_once_map[p as usize]
                                .entry(spcode)
                                .or_insert(0) += 1;
                        }
                    }
                }
                self.check_card_counter(a, crate::summon_support::activity::SPSUMMON, sumplayer);
                self.check_card_counter(
                    b,
                    crate::summon_support::activity::SPSUMMON,
                    1 - sumplayer,
                );
            } else {
                let mut sumplayer = tp;
                let card_target = self
                    .effects
                    .get(effect)
                    .is_some_and(|e| e.is_flag(crate::effect::flag::CARD_TARGET));
                if card_target && info.player == 0x10 {
                    sumplayer = 1 - sumplayer;
                }
                for c in cards {
                    let spcode = self.cards[c].spsummon_code;
                    if once_negate && spcode != 0 {
                        *self.core.spsummon_once_map[sumplayer as usize]
                            .entry(spcode)
                            .or_insert(0) += 1;
                    }
                    self.check_card_counter(
                        c,
                        crate::summon_support::activity::SPSUMMON,
                        sumplayer,
                    );
                }
            }
        }
    }
}

/// The three `skip_*` flags `SolveChain` carries and hands to the window it
/// opens at the end. Named rather than three bare bools, because the one at
/// the call site that matters — `skip_freechain || skip_new` — is easy to
/// mistranscribe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SolveChainSkip {
    pub trigger: bool,
    pub freechain: bool,
    pub new: bool,
}

#[cfg(test)]
mod tests {

    /// **Special-summon bookkeeping needs a declared count.** Under the
    /// old-oath flag a declared summon bumps the state counter; a
    /// declaration with a count of zero does not.
    #[test]
    fn special_summon_counting_needs_a_declared_count() {
        use crate::chain::{Chain, OpTarget};
        use crate::event::{category, Event};
        let mut f = Field::with_flags(
            8000,
            crate::duel::REFERENCE_CONFIGURATION | crate::duel::flags::CANNOT_SUMMON_OATH_OLD,
        );
        let e = f.new_effect(crate::effect::Effect::new(0, 0));
        let mut ch = Chain::new(e, Event::new(0));
        ch.triggering_player = 0;
        ch.opinfos.insert(
            category::SPECIAL_SUMMON,
            OpTarget {
                cards: None,
                count: 0,
                player: 0,
                param: 0,
            },
        );
        f.core.current_chain.push(ch);
        f.count_special_summons();
        assert_eq!(f.core.spsummon_state_count, [0, 0], "count 0: nothing");

        f.core.current_chain[0]
            .opinfos
            .get_mut(&category::SPECIAL_SUMMON)
            .unwrap()
            .count = 1;
        f.count_special_summons();
        assert_eq!(f.core.spsummon_state_count, [1, 0], "count 1: counted");
    }
    use super::*;
    use crate::board::{location, position};
    use crate::card::{Card, CardData};
    use crate::chain::Chain;
    use crate::effect::{flag, Effect};
    use crate::event::Event;
    use crate::field::Field;

    /// A face-up card with one effect, pushed onto the chain stack.
    fn chained(f: &mut Field, chain_count: u8) -> (usize, usize, u16) {
        let mut c = Card::with_data(
            CardData {
                code: 44095762,
                type_: card_type::TRAP,
                ..Default::default()
            },
            0,
        );
        c.current.location = location::SZONE;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let card = f.new_card(c);

        let mut e = Effect::new(
            effect_type::ACTIVATE | effect_type::ACTIONS,
            code::FREE_CHAIN,
        );
        e.owner = Some(card);
        e.handler = Some(card);
        let effect = f.new_effect(e);

        let mut chain = Chain::new(effect, Event::new(code::FREE_CHAIN));
        chain.chain_count = chain_count;
        chain.chain_id = f.next_field_id();
        let chain_id = chain.chain_id;
        f.core.current_chain.push(chain);
        (card, effect, chain_id)
    }

    /// `chaincount == 0` means the innermost link, not link zero. Every one
    /// of the three predicates takes it that way, and reading it as an index
    /// is off by one for every caller that passes a real link number.
    mod chain_count_is_one_based {
        use super::*;

        #[test]
        fn zero_means_the_innermost_link() {
            let mut f = Field::new(8000);
            let (_, first, _) = chained(&mut f, 1);
            let (_, second, _) = chained(&mut f, 2);
            f.effects.get_mut(first).unwrap().flag[0] |= flag::CANNOT_INACTIVATE;

            assert!(
                !f.is_chain_negatable(1),
                "link 1 is the first, and it cannot be negated"
            );
            assert!(f.is_chain_negatable(2), "link 2 can");
            assert!(
                f.is_chain_negatable(0),
                "and 0 means the innermost, which is link 2"
            );
            let _ = second;
        }

        #[test]
        fn a_count_past_the_stack_is_no() {
            let mut f = Field::new(8000);
            chained(&mut f, 1);
            assert!(!f.is_chain_negatable(2));
            assert!(!f.is_chain_disablable(2));
            assert!(!f.is_chain_disabled(2));
        }
    }

    /// A forbidden card loses its own "cannot be negated" protection: the
    /// reference skips both checks entirely when the handler is forbidden.
    #[test]
    fn a_forbidden_card_is_disablable_whatever_it_claims() {
        let mut f = Field::new(8000);
        let (card, effect, _) = chained(&mut f, 1);
        f.effects.get_mut(effect).unwrap().flag[0] |= flag::CANNOT_DISABLE;
        assert!(!f.is_chain_disablable(1));

        f.cards[card].set_status(status::FORBIDDEN, true);
        assert!(
            f.is_chain_disablable(1),
            "forbidden skips the protection entirely"
        );
    }

    /// `is_chain_disabled` finds the negating effect by its stored chain id,
    /// and stamps `RESET_CHAIN` on it — asking the question has a side
    /// effect, which is why the reference's version is non-const.
    #[test]
    fn finding_the_negation_stamps_it_for_reset() {
        use crate::field::reset;
        let mut f = Field::new(8000);
        let (card, _, chain_id) = chained(&mut f, 1);

        let mut negate = Effect::new(effect_type::SINGLE, code::DISABLE_CHAIN);
        negate.owner = Some(card);
        negate.handler = Some(card);
        negate.value = i64::from(chain_id);
        let negate = f.new_effect(negate);
        f.cards[card]
            .single_effect
            .insert(code::DISABLE_CHAIN, negate);

        assert_eq!(f.effects.get(negate).unwrap().reset_flag & reset::CHAIN, 0);
        assert!(f.is_chain_disabled(1));
        assert_ne!(
            f.effects.get(negate).unwrap().reset_flag & reset::CHAIN,
            0,
            "asking marked it for reset"
        );
    }

    /// A negating effect aimed at a *different* chain id does not match.
    #[test]
    fn a_negation_for_another_link_does_not_match() {
        let mut f = Field::new(8000);
        let (card, _, chain_id) = chained(&mut f, 1);
        let mut negate = Effect::new(effect_type::SINGLE, code::DISABLE_CHAIN);
        negate.owner = Some(card);
        negate.handler = Some(card);
        negate.value = i64::from(chain_id) + 1;
        let negate = f.new_effect(negate);
        f.cards[card]
            .single_effect
            .insert(code::DISABLE_CHAIN, negate);
        assert!(!f.is_chain_disabled(1));
    }

    /// The chain flag is enough on its own, with no effect to find.
    #[test]
    fn the_chain_flag_alone_means_disabled() {
        let mut f = Field::new(8000);
        chained(&mut f, 1);
        f.core.current_chain[0].flag |= chain_flag::DISABLE_EFFECT;
        assert!(f.is_chain_disabled(1));
    }

    /// An empty chain stack finishes the unit at once rather than indexing
    /// off the end.
    #[test]
    fn an_empty_stack_finishes_the_unit() {
        let mut f = Field::new(8000);
        assert!(f.solve_chain_step(0, SolveChainSkip::default()));
    }

    /// The window at the end opens unless *both* skips are asked for — the
    /// reference's condition is `!skip_trigger || !skip_new`, so either one
    /// being wanted is enough.
    #[test]
    fn the_closing_window_opens_unless_both_are_skipped() {
        use crate::processor::Kind;
        let cases = [
            (false, false, true),
            (true, false, true),
            (false, true, true),
            (true, true, false),
        ];
        for (trigger, new, expected) in cases {
            let mut f = Field::new(8000);
            let skip = SolveChainSkip {
                trigger,
                freechain: false,
                new,
            };
            chained(&mut f, 1);
            // Step 13 raises an event and calls adjust_all, both of which
            // are ported, so it can be driven directly.
            f.solve_chain_step(13, skip);
            let opened = f
                .core
                .subunits
                .iter()
                .any(|u| matches!(u.kind, Kind::PointEvent { .. }));
            assert_eq!(
                opened, expected,
                "trigger={trigger} new={new} should open={expected}"
            );
        }
    }

    /// `skip_freechain` is passed to the window as `freechain || new`, not
    /// as itself. Transcribing it as `skip.freechain` alone would leave
    /// free-chain activations offered where the reference suppresses them.
    #[test]
    fn the_window_widens_freechain_by_new() {
        use crate::processor::Kind;
        let mut f = Field::new(8000);
        chained(&mut f, 1);
        f.solve_chain_step(
            13,
            SolveChainSkip {
                trigger: false,
                freechain: false,
                new: true,
            },
        );
        let found = f.core.subunits.iter().find_map(|u| match u.kind {
            Kind::PointEvent { skip } => Some(skip.freechain),
            _ => None,
        });
        assert_eq!(found, Some(true), "new implies freechain");
    }
}
