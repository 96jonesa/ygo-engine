//! `SolveContinuous` — resolving a continuous effect, which is not a chain.
//!
//! A continuous effect that triggers does not go onto the chain and cannot be
//! responded to. It is taken off `sub_solving_continuous`, its `target` and
//! `operation` are run, and it is put back. Five short cases, and case 4 is a
//! machine of its own: the queue of *delayed* continuous effects, alternating
//! between the players.
//!
//! ## `conti_solving` marks the phase-driven ones
//!
//! A continuous effect that is delayed, or whose code is a phase event,
//! raises `core.conti_solving` while it resolves and calls `adjust_all` when
//! it is done. The test for "its code is a phase event" is
//! `!(code & 0xfffff000) && (code & (EVENT_PHASE | EVENT_PHASE_START))` —
//! the first half excludes the ordinary event codes, which are all above
//! 0x1000, so what is left is exactly a phase bit.
//!
//! ## The delayed queue alternates, starting with the turn player
//!
//! Case 4 takes **one** delayed effect at a time and re-emplaces the unit,
//! switching sides when the current player's queue empties. `conti_player`
//! carries whose turn it is to resolve one, and `PLAYER_NONE` means the
//! queues are empty and the alternation is over.

use crate::event::{code, EffectId, PLAYER_NONE};
use crate::field::Field;
use crate::processor::Kind;

/// The state `SolveContinuous` carries: the reason effect and player it
/// borrowed, put back at the end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SolveContinuousState {
    pub reason_effect: Option<EffectId>,
    pub reason_player: u8,
}

impl Field {
    /// One step of `SolveContinuous`.
    pub(crate) fn solve_continuous_step(
        &mut self,
        step: u16,
        state: &mut SolveContinuousState,
    ) -> bool {
        match step {
            0 => self.sc_begin(state),
            1 => false,
            2 => self.sc_operation(),
            3 => self.sc_finish(state),
            4 => self.sc_next_delayed(),
            _ => true,
        }
    }

    /// Whether an effect's resolution is one that ends with a full adjust:
    /// a delayed effect, or one whose code *is* a phase.
    fn is_phase_driven(&self, effect: EffectId) -> bool {
        let Some(e) = self.effects.get(effect) else {
            return false;
        };
        if e.is_flag(crate::effect::flag::DELAY) {
            return true;
        }
        e.code & 0xffff_f000 == 0 && e.code & (code::PHASE | code::PHASE_START) != 0
    }

    fn sc_begin(&mut self, state: &mut SolveContinuousState) -> bool {
        let pending: Vec<_> = self.core.sub_solving_continuous.drain(..).collect();
        for chain in pending.into_iter().rev() {
            self.core.solving_continuous.push_front(chain);
        }
        let Some(chain) = self.core.solving_continuous.front().cloned() else {
            return true;
        };
        let effect = chain.triggering_effect;
        let player = chain.triggering_player;
        if !self.check_count_limit(effect, player) {
            self.core.solving_continuous.pop_front();
            return true;
        }
        self.core.continuous_chain.push_back(chain.clone());
        if self.is_phase_driven(effect) {
            self.core.conti_solving = true;
        }
        state.reason_effect = self.core.reason_effect;
        state.reason_player = self.core.reason_player;
        if self.effects.get(effect).is_none_or(|e| e.target.is_none()) {
            return false;
        }
        self.core.sub_solving_event.push_back(chain.evt);
        self.emplace(Kind::ExecuteTarget {
            resume: None,
            effect,
            player,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        false
    }

    /// Case 2: the operation, and the charge.
    ///
    /// **`dec_count` is inside the operation branch**, so an effect with a
    /// target but no operation is not charged. The count is for doing the
    /// thing, not for being asked.
    fn sc_operation(&mut self) -> bool {
        let Some(chain) = self.core.solving_continuous.front().cloned() else {
            return false;
        };
        let effect = chain.triggering_effect;
        let player = chain.triggering_player;
        if self
            .effects
            .get(effect)
            .is_none_or(|e| e.operation.is_none())
        {
            return false;
        }
        self.dec_count(effect, player);
        self.core.sub_solving_event.push_back(chain.evt);
        self.emplace(Kind::ExecuteOperation {
            resume: None,
            effect,
            player,
            subject: None,
            args: Vec::new(),
            was_disabled: false,
        });
        false
    }

    /// Case 3: put back what was borrowed.
    ///
    /// A phase-driven resolution **falls through to case 4** rather than
    /// ending, which is what starts the delayed queue's alternation. An
    /// ordinary one ends here.
    fn sc_finish(&mut self, state: &mut SolveContinuousState) -> bool {
        let Some(chain) = self.core.solving_continuous.front().cloned() else {
            return true;
        };
        self.core.reason_effect = state.reason_effect;
        self.core.reason_player = state.reason_player;
        self.core.continuous_chain.pop_back();
        self.core.solving_continuous.pop_front();
        if self.is_phase_driven(chain.triggering_effect) {
            self.core.conti_solving = false;
            self.adjust_all();
            return false;
        }
        true
    }

    /// Case 4: take the next delayed continuous effect, alternating sides.
    ///
    /// One at a time, re-emplacing the unit each round. The turn player goes
    /// first, and the side only changes when the current one's queue is
    /// empty — so a player with two delayed effects resolves both before the
    /// other gets one.
    fn sc_next_delayed(&mut self) -> bool {
        let tp = self.infos.turn_player;
        if self.core.conti_player == PLAYER_NONE {
            self.core.conti_player = tp;
        }
        if self.core.conti_player == tp {
            if let Some(chain) = self.core.delayed_continuous_tp.pop_front() {
                self.core.sub_solving_continuous.push_back(chain);
                self.emplace(Kind::SolveContinuous {
                    state: Default::default(),
                });
            } else {
                self.core.conti_player = 1 - tp;
            }
        }
        if self.core.conti_player == 1 - tp {
            if let Some(chain) = self.core.delayed_continuous_ntp.pop_front() {
                self.core.sub_solving_continuous.push_back(chain);
                self.emplace(Kind::SolveContinuous {
                    state: Default::default(),
                });
            } else if let Some(chain) = self.core.delayed_continuous_tp.pop_front() {
                self.core.conti_player = tp;
                self.core.sub_solving_continuous.push_back(chain);
                self.emplace(Kind::SolveContinuous {
                    state: Default::default(),
                });
            } else {
                self.core.conti_player = PLAYER_NONE;
            }
        }
        true
    }
}
