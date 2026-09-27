//! The chain: one activation, waiting to resolve.
//!
//! A translation of ocgcore's `chain`. Every activated effect in the game
//! becomes one of these, and the lists they sit in — which list, and in what
//! order — are how the rules' timing is actually expressed.

use crate::card::CardState;
use crate::effect::{flag, Ctx};
use crate::event::{CardId, EffectId, Event, PLAYER_NONE};
use crate::field::Field;
use std::collections::VecDeque;

/// A list of chains, in the reference's `std::list<chain>`.
///
/// The reference splices between these constantly — moving a chain from one
/// list to another without copying it — so the Rust type is chosen for the
/// same moves: push and pop at either end.
pub type ChainList = VecDeque<Chain>;

/// The chain stack: `chain_array`, a `std::vector<chain>` rather than a
/// list, because the machinery indexes it and walks it backwards.
pub type ChainArray = Vec<Chain>;

/// One activation.
///
/// The `triggering_*` fields are a snapshot taken when the chain is built,
/// not a look at the card now. That is the point of them: an effect that
/// triggers on a card leaving the field is asked about the card's location
/// when it triggered, and by the time the chain resolves the card is
/// somewhere else.
/// `optarget` (`field.h:43`) — one declared operation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpTarget {
    /// The cards, if the declaration named any.
    pub cards: Option<Vec<CardId>>,
    pub count: u8,
    pub player: u8,
    pub param: i32,
}

#[derive(Clone, Debug)]
pub struct Chain {
    /// The effect being activated.
    pub triggering_effect: EffectId,
    /// The event that allowed it.
    pub evt: Event,
    /// The card the effect is on, snapshotted at activation.
    pub triggering_state: CardState,

    /// Where this chain sits in the chain stack, counting from 1.
    pub chain_count: u8,
    /// A duel-unique id, allocated from `infos.field_id`.
    pub chain_id: u16,
    /// The `global_id` of the event that built this chain — how two chains
    /// built from one event are recognised as simultaneous.
    pub event_id: u32,

    /// Whose activation this is. Not always the controller: an effect with
    /// `EFFECT_FLAG_EVENT_PLAYER` takes the event's player instead.
    pub triggering_player: u8,
    /// The controller of the handling card, at activation.
    pub triggering_controler: u8,
    /// Its location, at activation, with the specific field zone folded in.
    pub triggering_location: u16,
    pub triggering_sequence: u32,
    pub triggering_position: u8,
    /// Its status mask at activation.
    pub triggering_status: u32,

    /// What the effect declared it would act on.
    pub target_cards: Vec<CardId>,
    pub target_player: u8,
    pub target_param: i32,
    /// `opinfos` — what the effect said it would do, by category, as
    /// `Duel.SetOperationInfo` records it. The core reads only the
    /// `CATEGORY_SPECIAL_SUMMON` entry, and only under
    /// `DUEL_CANNOT_SUMMON_OATH_OLD` / `DUEL_SPSUMMON_ONCE_OLD_NEGATE`,
    /// neither of which this configuration sets — so for now this is
    /// storage the library layer and the client read.
    pub opinfos: std::collections::BTreeMap<u64, OpTarget>,
    /// `possibleopinfos` — the same for what it *might* do.
    pub possible_opinfos: std::collections::BTreeMap<u64, OpTarget>,

    /// Set when something negated this chain, and by whom.
    pub disable_reason: Option<EffectId>,
    pub disable_player: u8,

    /// An operation substituted for the effect's own, when something has
    /// replaced what it does.
    pub replace_op: Option<crate::effect::Operation>,
    /// Chain counters this link placed, kept so they can be given back if
    /// the activation is negated.
    pub applied_chain_counters: Option<Vec<u32>>,
    /// A `chain_flag::*` mask.
    pub flag: u32,
    /// True when the handler was sent to the graveyard in the same batch as
    /// the event that triggered it — the reference's `was_just_sent`, which
    /// gates simultaneity-sensitive triggers.
    pub was_just_sent: bool,
}

impl Chain {
    /// A chain for an effect responding to an event, with nothing yet
    /// decided about targets or negation.
    pub fn new(triggering_effect: EffectId, evt: Event) -> Self {
        let event_id = evt.global_id;
        Self {
            triggering_effect,
            evt,
            triggering_state: CardState::default(),
            chain_count: 0,
            chain_id: 0,
            event_id,
            triggering_player: PLAYER_NONE,
            triggering_controler: PLAYER_NONE,
            triggering_location: 0,
            triggering_sequence: 0,
            triggering_position: 0,
            triggering_status: 0,
            target_cards: Vec::new(),
            target_player: PLAYER_NONE,
            target_param: 0,
            opinfos: std::collections::BTreeMap::new(),
            possible_opinfos: std::collections::BTreeMap::new(),
            disable_reason: None,
            disable_player: PLAYER_NONE,
            replace_op: None,
            applied_chain_counters: None,
            flag: 0,
            was_just_sent: false,
        }
    }

    /// `chain::set_triggering_state(card*)` — snapshot the handling card.
    ///
    /// The reference folds the *specific* field zone into
    /// `triggering_location` on top of the broad one, so a chain records not
    /// merely "a spell/trap zone" but which kind. That is deliberate and is
    /// kept: rules ask which.
    /// Takes the card's `CardState` — its location, status and reason —
    /// rather than the card: that is all the reference's
    /// `set_triggering_state(card*)` reads, and the state is a copy where a
    /// card snapshot was a whole `Card` built and dropped per chain link.
    pub fn set_triggering_state(&mut self, snap: &CardState) {
        use crate::board::location;
        let at = &snap.loc;
        self.triggering_controler = at.controller;
        let mut loc = u16::from(at.location);
        if at.location & location::ONFIELD != 0 {
            // The reference tests these in this order and stops at the
            // first match, so a field zone is recorded as FZONE and never
            // also as STZONE. The order is the behaviour.
            for specific in [
                location::FZONE,
                location::PZONE,
                location::STZONE,
                location::EMZONE,
                location::MMZONE,
            ] {
                if at.is_location(specific) {
                    loc |= specific;
                    break;
                }
            }
        }
        self.triggering_location = loc;
        self.triggering_sequence = at.sequence;
        self.triggering_position = at.position;
        self.triggering_status = snap.status;
        self.triggering_state = *snap;
    }
}

/// `CHAIN_*` — the per-chain flags, as the reference numbers them.
pub mod chain_flag {
    pub const DISABLE_ACTIVATE: u32 = 0x01;
    pub const DISABLE_EFFECT: u32 = 0x02;
    pub const HAND_EFFECT: u32 = 0x04;
    pub const CONTINUOUS_CARD: u32 = 0x08;
    pub const ACTIVATING: u32 = 0x10;
    /// Set on a trigger that fired from the hand or deck. Read by the
    /// non-public SEGOC rules and by the once-per-turn limit on summoning
    /// yourself from the hand.
    pub const HAND_TRIGGER: u32 = 0x20;
}

/// `field::check_chain_target(chaincount, pcard)` — whether a chain link's
/// effect accepts `card` as one of its targets.
///
/// The one place the reference passes a target function its **tenth**
/// argument. A target is normally asked two questions — `chk == 0`, "may
/// this be activated at all", and `chk == 1`, "record what you will do" —
/// and this is the third: `chk = 0` **with a card**, meaning "would this
/// one be a legal choice". Scripts test it first and return early:
/// `if chkc then return chkc==tc end`.
///
/// Two gates before the call, both from the reference: the effect must
/// carry `EFFECT_FLAG_CARD_TARGET`, and it must have a target function at
/// all. A `chaincount` past the end of the chain is refused; zero means
/// the last link, as everywhere else.
///
/// The context is built from the **link's** event, not from whatever is
/// happening now — the reference pushes `pchain->evt` field by field.
///
/// No card in the Goat pool reaches this: the only caller is the Lua
/// export `Duel.CheckChainTarget`, which `chain.lua` aliases as
/// `Chain.CanTarget` and no pool script uses. It is ported because the
/// two cards that carry a `chkc` branch are written against it, and
/// tested directly rather than through a card.
impl Field {
    pub fn check_chain_target(&mut self, chaincount: u8, card: CardId) -> bool {
        let n = self.core.current_chain.len();
        if usize::from(chaincount) > n {
            return false;
        }
        let idx = if chaincount == 0 {
            n
        } else {
            usize::from(chaincount)
        };
        if idx == 0 {
            return false;
        }
        let ch = &self.core.current_chain[idx - 1];
        let (effect, tp, evt) = (ch.triggering_effect, ch.triggering_player, ch.evt.clone());
        let Some(e) = self.effects.get(effect) else {
            return false;
        };
        let Some(target) = e.target else {
            return false;
        };
        if !e.is_flag(flag::CARD_TARGET) {
            return false;
        }
        let ctx = Ctx {
            reason_effect: effect,
            player: tp,
            event: &evt,
            card: None,
            args: &[],
        };
        // A `chkc` call is the legality question, and the reference asks
        // it as a plain call rather than through the coroutine: a target
        // that suspended here would have nothing to be resumed by.
        target(self, &ctx, false, Some(card))
            .finished()
            .is_some_and(|v| v != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::location;
    use crate::card::{status, Card};
    use crate::event::code;

    /// A chain carries the event's id forward, which is what makes two
    /// chains built from one event recognisably simultaneous.
    /// The values are the reference's. ACTIVATING and CONTINUOUS_CARD were
    /// transposed here for three PRs — both are read by `AddChain` and
    /// `SolveChain`, so the transposition was live.
    #[test]
    fn chain_flags_are_the_references() {
        assert_eq!(chain_flag::DISABLE_ACTIVATE, 0x01);
        assert_eq!(chain_flag::DISABLE_EFFECT, 0x02);
        assert_eq!(chain_flag::HAND_EFFECT, 0x04);
        assert_eq!(chain_flag::CONTINUOUS_CARD, 0x08);
        assert_eq!(chain_flag::ACTIVATING, 0x10);
        assert_eq!(chain_flag::HAND_TRIGGER, 0x20);
    }

    #[test]
    fn a_chain_inherits_the_events_id() {
        let mut e = Event::new(code::CHAIN_END);
        e.global_id = 42;
        let a = Chain::new(1, e.clone());
        let b = Chain::new(2, e);
        assert_eq!(a.event_id, 42);
        assert_eq!(b.event_id, a.event_id, "one event, one id");
    }

    mod set_triggering_state {
        use super::*;

        /// The snapshot is taken at activation and does not follow the card.
        #[test]
        fn it_snapshots_rather_than_follows() {
            let mut card = Card::new(44095762, 0);
            card.current.controller = 1;
            card.current.location = location::SZONE;
            card.current.sequence = 3;
            card.set_status(status::SET_TURN, true);

            let mut chain = Chain::new(0, Event::new(code::CHAIN_END));
            chain.set_triggering_state(&card.state());

            card.current.location = location::GRAVE;
            card.current.sequence = 0;
            card.set_status(status::SET_TURN, false);

            assert_eq!(chain.triggering_controler, 1);
            assert_eq!(chain.triggering_sequence, 3);
            assert_eq!(chain.triggering_status & status::SET_TURN, status::SET_TURN);
            assert_eq!(
                chain.triggering_location & u16::from(location::SZONE),
                u16::from(location::SZONE)
            );
        }

        /// The specific zone is folded in on top of the broad one: a chain
        /// records not merely "a spell/trap zone" but which kind, because
        /// rules ask which.
        #[test]
        fn the_specific_field_zone_is_folded_in() {
            let mut card = Card::new(5318639, 0);
            card.current.location = location::SZONE;
            card.current.sequence = 5; // the field zone's seat

            let mut chain = Chain::new(0, Event::new(code::CHAIN_END));
            chain.set_triggering_state(&card.state());

            assert_ne!(chain.triggering_location & u16::from(location::SZONE), 0);
            assert_ne!(
                chain.triggering_location & location::FZONE,
                0,
                "and which kind of spell/trap zone it was"
            );
        }

        /// Off the field there is no specific zone to fold in.
        #[test]
        fn nothing_is_folded_in_off_the_field() {
            let mut card = Card::new(5318639, 0);
            card.current.location = location::GRAVE;
            let mut chain = Chain::new(0, Event::new(code::CHAIN_END));
            chain.set_triggering_state(&card.state());
            assert_eq!(chain.triggering_location, u16::from(location::GRAVE));
        }
    }
}

#[cfg(test)]
mod check_chain_target_tests {
    use super::*;
    use crate::card::{card_type, Card, CardData};
    use crate::effect::{effect_type, Effect};
    use crate::script_api as api;

    /// A target function that answers the third question only: "is this
    /// the card I would pick?" — the shape every `chkc`-carrying script
    /// uses, `if chkc then return chkc==tc end`.
    fn only_card_one(
        _f: &mut Field,
        _ctx: &Ctx,
        _chk: bool,
        chkc: Option<CardId>,
    ) -> crate::effect::Yield {
        crate::effect::Yield::Done(i32::from(chkc == Some(1)))
    }

    /// A field with one chain link, whose effect is built by `tweak`.
    fn field_with_link(tweak: impl FnOnce(&mut Effect)) -> Field {
        let mut f = Field::new(8000);
        for i in 0..3u32 {
            let c = Card::with_data(
                CardData {
                    code: 100 + i,
                    type_: card_type::MONSTER,
                    ..Default::default()
                },
                0,
            );
            let id = f.new_card(c);
            f.add_card(0, id, crate::board::location::MZONE, i, false);
        }
        let mut e = Effect::new(effect_type::ACTIVATE, crate::event::code::FREE_CHAIN);
        e.owner = Some(0);
        e.handler = Some(0);
        e.target = Some(only_card_one);
        e.flag[0] |= flag::CARD_TARGET;
        tweak(&mut e);
        let e = f.new_effect(e);
        let mut ch = Chain::new(e, Event::new(crate::event::code::FREE_CHAIN));
        ch.triggering_player = 0;
        ch.chain_count = 1;
        f.core.current_chain.push(ch);
        f
    }

    /// **It asks the link's target function with the card**, and the
    /// answer is the function's.
    #[test]
    fn it_asks_the_targets_third_question() {
        let mut f = field_with_link(|_| {});
        assert!(
            api::check_chain_target(&mut f, 1, 1),
            "the card it would pick"
        );
        assert!(!api::check_chain_target(&mut f, 1, 2), "any other card");
    }

    /// **It asks the *asking* question, not the recording one.** The
    /// reference pushes `0` for `chk` alongside the card, so a target
    /// that behaves differently when told to record must not be told to.
    #[test]
    fn it_asks_with_chk_false() {
        fn remembers_chk(
            _f: &mut Field,
            _ctx: &Ctx,
            chk: bool,
            chkc: Option<CardId>,
        ) -> crate::effect::Yield {
            crate::effect::Yield::Done(i32::from(!chk && chkc == Some(1)))
        }
        let mut f = field_with_link(|e| e.target = Some(remembers_chk));
        assert!(
            api::check_chain_target(&mut f, 1, 1),
            "asked with chk = 0, as the reference does"
        );
    }

    /// **Zero means the last link**, as everywhere else in the chain
    /// accessors.
    #[test]
    fn zero_means_the_last_link() {
        let mut f = field_with_link(|_| {});
        assert!(api::check_chain_target(&mut f, 0, 1));
        assert!(!api::check_chain_target(&mut f, 0, 2));
    }

    /// **Two gates before the call.** Without `EFFECT_FLAG_CARD_TARGET`
    /// the question is not asked at all, and neither is it without a
    /// target function — in both cases the answer is no, not the
    /// function's.
    #[test]
    fn the_flag_and_the_function_are_both_required() {
        let mut without_flag = field_with_link(|e| e.flag[0] &= !flag::CARD_TARGET);
        assert!(
            !api::check_chain_target(&mut without_flag, 1, 1),
            "the card it would have picked, refused for want of the flag"
        );
        let mut without_target = field_with_link(|e| e.target = None);
        assert!(!api::check_chain_target(&mut without_target, 1, 1));
    }

    /// **A count past the end of the chain is refused**, and so is any
    /// count against an empty chain.
    #[test]
    fn a_count_past_the_end_is_refused() {
        let mut f = field_with_link(|_| {});
        assert!(!api::check_chain_target(&mut f, 2, 1), "one link, not two");
        let mut empty = Field::new(8000);
        assert!(!api::check_chain_target(&mut empty, 0, 1), "nothing to ask");
        assert!(!api::check_chain_target(&mut empty, 1, 1));
    }
}
