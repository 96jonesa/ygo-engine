//! Asking a player something: the `Select*` units.
//!
//! These are the seam between the engine and whoever is playing it. Every
//! one of them follows the same two-step shape, and the shape *is* the
//! interface:
//!
//! | step | |
//! |---|---|
//! | 0 | either answer the question outright, or **yield** |
//! | 1 | validate the answer; retry if it is not legal |
//!
//! The processor turns that into a yield because [`crate::processor::Kind`]
//! reports `needs_answer` for these units: step 0 returns "not finished",
//! `process` returns [`crate::processor::Status::Awaiting`], and nothing
//! advances until the host writes an answer into `core.returns` and calls
//! `process` again.
//!
//! ## Step 0 answering outright is the important half
//!
//! It is tempting to read these as "ask the player" and implement the yield
//! only. But every one of them **short-circuits when there is no real
//! choice**: `SelectPosition` with a single legal position, `SelectOption`
//! with no options, `SelectPlace` with a count of zero. The reference
//! answers and returns `TRUE` — the unit finishes without ever yielding.
//!
//! That matters twice over. It is the reference's behaviour, so a port that
//! always yields diverges. And it is exactly right for the solver this
//! engine is for: a decision point with one legal action is not a decision,
//! and presenting it as one inflates the game tree with nodes that have a
//! single child.
//!
//! ## Retry is a loop, not an error
//!
//! An illegal answer does not fail the unit. Step 1 emits `MSG_RETRY` and
//! returns "not finished" **without advancing the step**, so the same
//! validation runs again against the next answer. A host that sends nonsense
//! is asked again, forever, rather than corrupting the duel.
//!
//! ## `DUEL_SIMPLE_AI` is dead in this configuration
//!
//! Several of these units have a branch answering on player 1's behalf when
//! `DUEL_SIMPLE_AI` is set. The reference configuration this port targets —
//! `(DUEL_MODE_MR5 & ~DUEL_EMZONE) | DUEL_PSEUDO_SHUFFLE`, `0x2C810` — does
//! **not** set it (`0x40`). Those branches are transcribed as a named
//! predicate that is currently always false, rather than omitted, so that
//! turning the flag on later is a one-line change and the omission is not
//! mistaken for an oversight.

use crate::board::{location, position};
use crate::event::CardId;

/// What the generator rolled for a chance question — [`Field::draw_chance`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChanceSample {
    /// The coin faces, bit `i` set for heads on coin `i`.
    Coin(u32),
    /// The random selection's picks: ascending indices into the offered
    /// group.
    Random(Vec<usize>),
    /// The deck's top, as the shuffle left it: nothing to roll.
    DeckTop,
}
use crate::field::{Field, Message};

/// The `HINT_*` subtypes these units emit.
pub mod hint {
    pub const EVENT: u8 = 1;
    pub const MESSAGE: u8 = 2;
    pub const SELECTMSG: u8 = 3;
    pub const OPSELECTED: u8 = 4;
    pub const EFFECT: u8 = 5;
    pub const RACE: u8 = 6;
    pub const ATTRIB: u8 = 7;
    pub const CODE: u8 = 8;
    pub const NUMBER: u8 = 9;
    pub const CARD: u8 = 10;
    pub const ZONE: u8 = 11;
}

/// `HINTMSG_*` (`constant.lua:805-864`) — what a `HINT_SELECTMSG` is
/// asking a player for.
///
/// **Transcribed whole.** An earlier version carried only the two
/// entries the pool had reached, which is the shape that invites the
/// next one being guessed: the numbers are dense and consecutive
/// until they are not (there is a gap between 533 and 549, and
/// another between 556 and 560), so an entry inferred from its
/// neighbours is wrong without looking wrong. `tools/check_constants.py`
/// pins the boundaries.
#[allow(dead_code)]
pub mod hintmsg {
    /// `constant.lua:805`
    pub const RELEASE: u64 = 500;
    /// `constant.lua:806`
    pub const DISCARD: u64 = 501;
    /// `constant.lua:807`
    pub const DESTROY: u64 = 502;
    /// `constant.lua:808`
    pub const REMOVE: u64 = 503;
    /// `constant.lua:809`
    pub const TOGRAVE: u64 = 504;
    /// `constant.lua:810`
    pub const RTOHAND: u64 = 505;
    /// `constant.lua:811`
    pub const ATOHAND: u64 = 506;
    /// `constant.lua:812`
    pub const TODECK: u64 = 507;
    /// `constant.lua:813`
    pub const SUMMON: u64 = 508;
    /// `constant.lua:814`
    pub const SPSUMMON: u64 = 509;
    /// `constant.lua:815`
    pub const SET: u64 = 510;
    /// `constant.lua:816`
    pub const FMATERIAL: u64 = 511;
    /// `constant.lua:817`
    pub const SMATERIAL: u64 = 512;
    /// `constant.lua:818`
    pub const XMATERIAL: u64 = 513;
    /// `constant.lua:819`
    pub const FACEUP: u64 = 514;
    /// `constant.lua:820`
    pub const FACEDOWN: u64 = 515;
    /// `constant.lua:821`
    pub const ATTACK: u64 = 516;
    /// `constant.lua:822`
    pub const DEFENSE: u64 = 517;
    /// `constant.lua:823`
    pub const EQUIP: u64 = 518;
    /// `constant.lua:824`
    pub const REMOVEXYZ: u64 = 519;
    /// `constant.lua:825`
    pub const CONTROL: u64 = 520;
    /// `constant.lua:826`
    pub const DESREPLACE: u64 = 521;
    /// `constant.lua:827`
    pub const FACEUPATTACK: u64 = 522;
    /// `constant.lua:828`
    pub const FACEUPDEFENSE: u64 = 523;
    /// `constant.lua:829`
    pub const FACEDOWNATTACK: u64 = 524;
    /// `constant.lua:830`
    pub const FACEDOWNDEFENSE: u64 = 525;
    /// `constant.lua:831`
    pub const CONFIRM: u64 = 526;
    /// `constant.lua:832`
    pub const TOFIELD: u64 = 527;
    /// `constant.lua:833`
    pub const POSCHANGE: u64 = 528;
    /// `constant.lua:834`
    pub const SELF: u64 = 529;
    /// `constant.lua:835`
    pub const OPPO: u64 = 530;
    /// `constant.lua:836`
    pub const TRIBUTE: u64 = 531;
    /// `constant.lua:837`
    pub const DEATTACHFROM: u64 = 532;
    /// `constant.lua:838`
    pub const LMATERIAL: u64 = 533;
    /// `constant.lua:839`
    pub const ATTACKTARGET: u64 = 549;
    /// `constant.lua:840`
    pub const EFFECT: u64 = 550;
    /// `constant.lua:841`
    pub const TARGET: u64 = 551;
    /// `constant.lua:842`
    pub const COIN: u64 = 552;
    /// `constant.lua:843`
    pub const DICE: u64 = 553;
    /// `constant.lua:844`
    pub const CARDTYPE: u64 = 554;
    /// `constant.lua:845`
    pub const OPTION: u64 = 555;
    /// `constant.lua:846`
    pub const RESOLVEEFFECT: u64 = 556;
    /// `constant.lua:847`
    pub const SELECT: u64 = 560;
    /// `constant.lua:848`
    pub const POSITION: u64 = 561;
    /// `constant.lua:849`
    pub const ATTRIBUTE: u64 = 562;
    /// `constant.lua:850`
    pub const RACE: u64 = 563;
    /// `constant.lua:851`
    pub const CODE: u64 = 564;
    /// `constant.lua:852`
    pub const NUMBER: u64 = 565;
    /// `constant.lua:853`
    pub const EFFACTIVATE: u64 = 566;
    /// `constant.lua:854`
    pub const LVRANK: u64 = 567;
    /// `constant.lua:855`
    pub const RESOLVECARD: u64 = 568;
    /// `constant.lua:856`
    pub const ZONE: u64 = 569;
    /// `constant.lua:857`
    pub const DISABLEZONE: u64 = 570;
    /// `constant.lua:858`
    pub const TOZONE: u64 = 571;
    /// `constant.lua:859`
    pub const COUNTER: u64 = 572;
    /// `constant.lua:860`
    pub const NEGATE: u64 = 575;
    /// `constant.lua:861`
    pub const ATKDEF: u64 = 576;
    /// `constant.lua:862`
    pub const APPLYTO: u64 = 577;
    /// `constant.lua:863`
    pub const ATTACH: u64 = 578;
    /// `constant.lua:864`
    pub const RTOGRAVE: u64 = 579;
}

/// `EFFECT_CLIENT_MODE_*` — how the host presents an offered effect.
pub mod client_mode {
    pub const NORMAL: u8 = 0;
    pub const RESOLVE: u8 = 1;
    pub const RESET: u8 = 2;
}

impl Field {
    /// Whether the engine answers for player 1 rather than asking.
    ///
    /// `DUEL_SIMPLE_AI`, which the reference configuration does not set. See
    /// the module note: kept as a named predicate so the branches that
    /// depend on it read as the reference's rather than as missing.
    fn simple_ai_answers_for(&self, playerid: u8) -> bool {
        playerid == 1 && self.is_flag(crate::duel::flags::SIMPLE_AI)
    }

    /// `field::process(Processors::SelectYesNo&)` — a bare yes/no.
    /// Solver mode's coin question: ask, then accept any bitmask that fits
    /// in `count` bits.
    pub(crate) fn select_coin_step(&mut self, step: u16, player: u8, count: u8) -> bool {
        if step == 0 {
            self.messages.push(Message::SelectCoin { player, count });
            self.core.returns.set(-1);
            return false;
        }
        let answer = self.core.returns.get();
        if answer < 0 || (count < 31 && answer >= 1 << count) {
            self.messages.push(Message::Retry);
            return false;
        }
        true
    }

    /// Solver mode's deck-top question: the top `count` cards of `player`'s
    /// deck are about to be seen. The answer carries nothing — the host's
    /// work is done on the deck itself, with [`Field::set_deck_order`],
    /// before it answers.
    pub(crate) fn select_deck_top_step(&mut self, step: u16, player: u8, count: u32) -> bool {
        if step == 0 {
            self.messages.push(Message::SelectDeckTop { player, count });
            self.core.returns.set(-1);
            return false;
        }
        true
    }

    /// The faithful mode's random picks: `count` distinct indices into a
    /// group of `len`, drawn from the duel's generator exactly as
    /// `Group.RandomSelect` draws them — an index at a time into a set, so
    /// a repeat costs a roll and changes nothing. Ascending.
    pub(crate) fn roll_random_indices(&mut self, len: usize, count: usize) -> Vec<usize> {
        let mut picked = std::collections::BTreeSet::new();
        while picked.len() < count {
            picked.insert(self.rng.next_integer(0, len as i32 - 1) as usize);
        }
        picked.into_iter().collect()
    }

    /// Solver mode: answer the outstanding chance question from the duel's
    /// own generator, rolling exactly what the faithful mode would have
    /// rolled. A rollout that wants chance sampled rather than enumerated
    /// answers this way; it is also the equivalence the mode is tested by
    /// (`driver::chance_tests`): asked and answered so, a duel replays the
    /// faithful mode's trace to the line.
    ///
    /// Returns false, answering nothing, when the outstanding question is
    /// not a chance question.
    pub fn sample_chance(&mut self) -> bool {
        let Some(sample) = self.draw_chance() else {
            return false;
        };
        self.write_chance(&sample);
        true
    }

    /// The roll half of [`Field::sample_chance`]: what the generator says
    /// the outstanding chance question's answer is, without answering it.
    /// A deck-top question draws nothing — the deck is already in the order
    /// the shuffle rolled. `None` when no chance question is outstanding.
    pub fn draw_chance(&mut self) -> Option<ChanceSample> {
        use crate::processor::Kind;
        enum Ask {
            Coin(u8),
            Random(usize, usize),
            DeckTop,
        }
        let ask = match self.core.units.front().map(|u| &u.kind) {
            Some(Kind::SelectCoin { count, .. }) => Ask::Coin(*count),
            Some(Kind::SelectRandom { count, cards, .. }) => {
                Ask::Random(cards.len(), usize::from(*count))
            }
            Some(Kind::SelectDeckTop { .. }) => Ask::DeckTop,
            _ => return None,
        };
        Some(match ask {
            Ask::Coin(count) => ChanceSample::Coin(
                self.roll_coins(count)
                    .iter()
                    .enumerate()
                    .fold(0u32, |m, (i, &heads)| if heads { m | (1 << i) } else { m }),
            ),
            Ask::Random(len, count) => ChanceSample::Random(self.roll_random_indices(len, count)),
            Ask::DeckTop => ChanceSample::DeckTop,
        })
    }

    /// The answer half of [`Field::sample_chance`]: write a sample as the
    /// outstanding question's answer.
    pub fn write_chance(&mut self, sample: &ChanceSample) {
        match sample {
            ChanceSample::Coin(mask) => self.core.returns.set(*mask as i32),
            ChanceSample::Random(picks) => {
                self.core.returns.set_i32(0, 0);
                self.core.returns.set_i32(1, picks.len() as i32);
                for (i, idx) in picks.iter().enumerate() {
                    self.core.returns.set_i32(i + 2, *idx as i32);
                }
            }
            ChanceSample::DeckTop => self.core.returns.set(0),
        }
    }

    /// Solver mode's random-selection question: offer the cards, accept a
    /// card-selection answer of exactly `count` distinct indices.
    pub(crate) fn select_random_step(
        &mut self,
        step: u16,
        player: u8,
        count: u8,
        cards: Vec<CardId>,
    ) -> bool {
        if step == 0 {
            self.core.select_cards.clone_from(&cards);
            self.messages.push(Message::SelectRandom {
                player,
                count,
                cards,
            });
            self.core.returns.set(-1);
            return false;
        }
        if !self.parse_response_cards(false) {
            self.messages.push(Message::Retry);
            return false;
        }
        let picks = self.core.return_cards.list.clone();
        let mut distinct = picks.clone();
        distinct.sort_unstable();
        distinct.dedup();
        if picks.len() != usize::from(count) || distinct.len() != picks.len() {
            self.messages.push(Message::Retry);
            return false;
        }
        self.finish_random_selection(player, distinct);
        true
    }

    /// Record a random selection's picks for the operation that asked and
    /// announce them (`MSG_RANDOM_SELECTED`, `libgroup.cpp`).
    pub(crate) fn finish_random_selection(&mut self, player: u8, picks: Vec<CardId>) {
        let cards = picks.iter().map(|&c| self.get_info_location(c)).collect();
        self.messages
            .push(Message::RandomSelected { player, cards });
        self.core.random_selected = picks;
    }

    pub(crate) fn select_yes_no_step(&mut self, step: u16, player: u8, description: u64) -> bool {
        if step == 0 {
            if self.simple_ai_answers_for(player) {
                self.core.returns.set(1);
                return true;
            }
            self.messages.push(Message::SelectYesNo {
                player,
                description,
            });
            // -1 is "not answered yet", and it is what makes an unanswered
            // unit fail validation rather than read as a legal "no".
            self.core.returns.set(-1);
            return false;
        }
        let answer = self.core.returns.get();
        if answer != 0 && answer != 1 {
            self.messages.push(Message::Retry);
            return false;
        }
        true
    }

    /// `field::process(Processors::SelectChain&)` — which effect to chain.
    ///
    /// The sixth host question, and the one every response window ends at:
    /// `QuickEffect` offers this unconditionally, so nothing that opens a
    /// window can run without it.
    ///
    /// ## `-1` is the answer, and it is pre-set
    ///
    /// Step 0 writes `-1` **before** anything else and leaves it there.
    /// Declining is the default, and a host that answers nothing has
    /// declined. That is the opposite polarity from the other questions,
    /// where `-1` means "unanswered" and fails validation.
    ///
    /// ## `forced` changes what an answer of `-1` means
    ///
    /// A forced offer is a choice of *order*, not of whether — the effects
    /// are mandatory. So `-1` is rejected there and accepted here, while the
    /// range check below is the same in both cases.
    pub(crate) fn select_chain_step(
        &mut self,
        step: u16,
        player: u8,
        spe_count: u8,
        forced: bool,
    ) -> bool {
        if step == 0 {
            self.core.returns.set(-1);
            if self.simple_ai_answers_for(player) {
                // The reference's simple AI: take the offer unless this
                // player already has a link on the chain.
                if !self.core.select_chains.is_empty() {
                    let already_acting = self
                        .core
                        .current_chain
                        .iter()
                        .any(|c| c.triggering_player == 1);
                    if forced || !already_acting {
                        self.core.returns.set(0);
                    }
                }
                return true;
            }
            // `chain_operation_sort`: by the effect's id, which is the order
            // the effects were created in.
            let mut offered: Vec<crate::chain::Chain> = self.core.select_chains.drain(..).collect();
            offered.sort_by_key(|c| {
                self.effects
                    .get(c.triggering_effect)
                    .map_or(0, |e| e.id.get())
            });
            self.core.select_chains = offered.into();
            let chains = self
                .core
                .select_chains
                .iter()
                .map(|c| {
                    let effect = c.triggering_effect;
                    let handler = self
                        .effects
                        .get(effect)
                        .and_then(|e| e.get_handler(&self.cards));
                    crate::field::ChainOffer {
                        code: handler.map_or(0, |h| self.cards[h].data.code),
                        info: handler
                            .map(|h| self.get_info_location(h))
                            .unwrap_or_default(),
                        description: self.effects.get(effect).map_or(0, |e| e.description),
                        client_mode: self.client_mode(effect),
                    }
                })
                .collect();
            self.messages.push(Message::SelectChain {
                player,
                spe_count,
                forced,
                hint_timing: self.core.hint_timing[player as usize],
                opponent_hint_timing: self.core.hint_timing[1 - player as usize],
                chains,
            });
            return false;
        }
        let answer = self.core.returns.get();
        if !forced && answer == -1 {
            return true;
        }
        if answer < 0 || answer >= self.core.select_chains.len() as i32 {
            self.messages.push(Message::Retry);
            return false;
        }
        true
    }

    /// `effect::get_client_mode` — how the host should present an effect.
    pub(crate) fn client_mode(&self, effect: crate::event::EffectId) -> u8 {
        let Some(e) = self.effects.get(effect) else {
            return client_mode::NORMAL;
        };
        if e.is_flag(crate::effect::flag::FIELD_ONLY) {
            client_mode::RESOLVE
        } else if !e.is_type(crate::effect::effect_type::ACTIONS) {
            client_mode::RESET
        } else {
            client_mode::NORMAL
        }
    }

    /// `field::process(Processors::SelectEffectYesNo&)` — a yes/no *about a
    /// card*, which is what the host shows alongside the question.
    pub(crate) fn select_effect_yes_no_step(
        &mut self,
        step: u16,
        player: u8,
        card: CardId,
        description: u64,
    ) -> bool {
        if step == 0 {
            if self.simple_ai_answers_for(player) {
                self.core.returns.set(1);
                return true;
            }
            let info = self.get_info_location(card);
            let code = self.cards[card].data.code;
            self.messages.push(Message::SelectEffectYesNo {
                player,
                code,
                controller: info.controller,
                location: info.location,
                sequence: info.sequence,
                position: info.position,
                description,
            });
            self.core.returns.set(-1);
            return false;
        }
        let answer = self.core.returns.get();
        if answer != 0 && answer != 1 {
            self.messages.push(Message::Retry);
            return false;
        }
        true
    }

    /// `field::process(Processors::SelectOption&)` — choose among the
    /// options an effect offered.
    ///
    /// **No options is not an error.** The reference emits an empty
    /// select-message hint and finishes with the answer left at `-1`, which
    /// the caller reads as "there was nothing to choose".
    pub(crate) fn select_option_step(&mut self, step: u16, player: u8) -> bool {
        if step == 0 {
            self.core.returns.set(-1);
            if self.core.select_options.is_empty() {
                self.messages.push(Message::Hint {
                    kind: hint::SELECTMSG,
                    player,
                    value: 0,
                });
                return true;
            }
            if self.simple_ai_answers_for(player) {
                self.core.returns.set(0);
                return true;
            }
            let options = self.core.select_options.clone();
            self.messages
                .push(Message::SelectOption { player, options });
            return false;
        }
        let answer = self.core.returns.get();
        if answer < 0 || answer as usize >= self.core.select_options.len() {
            self.messages.push(Message::Retry);
            return false;
        }
        true
    }

    /// `field::process(Processors::SelectPosition&)` — which way up.
    ///
    /// Two short-circuits before any question is asked, and the second is
    /// the one that matters:
    ///
    /// - **No positions offered** answers face-up attack. Not an error: a
    ///   caller that has no opinion gets the default.
    /// - **Exactly one position offered** answers it. The mask is narrowed
    ///   to `0xf` first, and the test is against the four single-bit values
    ///   rather than `count_ones() == 1` — same thing, and written the
    ///   reference's way.
    pub(crate) fn select_position_step(
        &mut self,
        step: u16,
        player: u8,
        code: u32,
        positions: u8,
    ) -> bool {
        if step == 0 {
            if positions == 0 {
                self.core.returns.set(i32::from(position::FACEUP_ATTACK));
                return true;
            }
            let positions = positions & 0xf;
            if matches!(positions, 0x1 | 0x2 | 0x4 | 0x8) {
                self.core.returns.set(i32::from(positions));
                return true;
            }
            if self.simple_ai_answers_for(player) {
                // The reference's preference order, which is not the bit
                // order: face-up defence, face-up attack, face-down defence,
                // then face-down attack.
                let pick = if positions & 0x4 != 0 {
                    0x4
                } else if positions & 0x1 != 0 {
                    0x1
                } else if positions & 0x8 != 0 {
                    0x8
                } else {
                    0x2
                };
                self.core.returns.set(pick);
                return true;
            }
            self.messages.push(Message::SelectPosition {
                player,
                code,
                positions,
            });
            self.core.returns.set(0);
            return false;
        }
        let answer = self.core.returns.get();
        if answer & i32::from(positions) == 0 || !matches!(answer, 0x1 | 0x2 | 0x4 | 0x8) {
            self.messages.push(Message::Retry);
            return false;
        }
        true
    }
}

impl Field {
    /// `field::process(Processors::SelectPlace&)` — choose one or more zones.
    ///
    /// ## `flag` is a mask of what is *not* available
    ///
    /// The polarity is the thing to get right, and it is the opposite of the
    /// intuitive one. A **set** bit means that zone may not be chosen, and
    /// validation rejects an answer whose zone is already set. Each accepted
    /// choice then **sets its own bit**, which is how a two-zone selection is
    /// stopped from naming the same zone twice — the mask is reused as the
    /// running record of what this answer has already taken.
    ///
    /// The layout, for a zone belonging to the asking player:
    ///
    /// | bits | |
    /// |---|---|
    /// | 0-6 | their Monster Zones |
    /// | 8-12 | their Spell & Trap Zones |
    /// | +16 | the same, for the opponent |
    ///
    /// which is why `SendTo` builds its mask as
    /// `((flag << 8) & 0xff00) | 0xffffe0ff` — every bit set except the
    /// eight spell/trap bits it is offering.
    ///
    /// ## The sequence bound differs by one between the two rows
    ///
    /// `sequence > 7 - ismzone`: Monster Zones allow 0-6, Spell & Trap Zones
    /// 0-7. Writing a single bound for both admits a monster zone 7 that
    /// does not exist.
    ///
    /// ## Answers are read unsigned
    ///
    /// The unit writes its own sentinel with `set<int8_t>` and reads the
    /// host's answer with `at<uint8_t>`. That is deliberate: a player id of
    /// `255` must fail the `> 1` test, where reading it signed would give
    /// `-1` and pass.
    pub(crate) fn select_place_step(
        &mut self,
        step: u16,
        player: u8,
        flag: u32,
        count: u8,
        disable_field: bool,
    ) -> bool {
        if step == 0 {
            // Nothing to choose: a hint, and done. Not an error.
            if count == 0 {
                self.messages.push(Message::Hint {
                    kind: hint::SELECTMSG,
                    player,
                    value: 0,
                });
                return true;
            }
            if self.simple_ai_answers_for(player) {
                self.simple_ai_pick_place(player, flag);
                return true;
            }
            self.messages.push(Message::SelectPlace {
                player,
                count,
                flag,
                disable_field,
            });
            self.core.returns.set_i8(0, 0);
            return false;
        }

        // Validation walks the answer three bytes at a time: player,
        // location, sequence — per zone chosen.
        let mut flag = flag;
        let mut pt = 0usize;
        for _ in 0..count {
            let select_player = self.core.returns.at_u8(pt);
            if select_player > 1 {
                return self.retry();
            }
            let is_asking_player = select_player == player;
            let loc = self.core.returns.at_u8(pt + 1);
            if loc != location::MZONE && loc != location::SZONE {
                return self.retry();
            }
            let is_mzone = loc == location::MZONE;
            let sequence = self.core.returns.at_u8(pt + 2);
            // Monster zones 0-6, spell/trap zones 0-7.
            if sequence > 7 - u8::from(is_mzone) {
                return self.retry();
            }
            let mut bit = 1u32 << sequence;
            if !is_mzone {
                bit <<= 8;
            }
            if !is_asking_player {
                bit <<= 16;
            }
            // Set means unavailable — including zones this same answer has
            // already claimed.
            if bit & flag != 0 {
                return self.retry();
            }
            flag |= bit;
            pt += 3;
        }
        true
    }

    /// `MSG_RETRY` and "do not advance": the same question is asked again.
    fn retry(&mut self) -> bool {
        self.messages.push(Message::Retry);
        false
    }

    /// The `DUEL_SIMPLE_AI` answer for a zone choice — dead in this
    /// configuration, transcribed so it is not mistaken for an omission.
    ///
    /// Note the reference **inverts** the flag first and then searches for a
    /// set bit: it is looking for an *available* zone, and availability is
    /// the complement. Its per-row preference order is also not the bit
    /// order — it prefers the middle zones outward.
    fn simple_ai_pick_place(&mut self, player: u8, flag: u32) {
        let avail = !flag;
        let (side, loc, filter, pzone) = if avail & 0x7f != 0 {
            (1, location::MZONE, avail & 0x7f, false)
        } else if avail & 0x1f00 != 0 {
            (1, location::SZONE, (avail >> 8) & 0x1f, false)
        } else if avail & 0xc000 != 0 {
            (1, location::SZONE, (avail >> 14) & 0x3, true)
        } else if avail & 0x7f_0000 != 0 {
            (0, location::MZONE, (avail >> 16) & 0x7f, false)
        } else if avail & 0x1f00_0000 != 0 {
            (0, location::SZONE, (avail >> 24) & 0x1f, false)
        } else {
            (0, location::SZONE, (avail >> 30) & 0x3, true)
        };
        let seq: i8 = if !pzone {
            // 6, 5, 2, 1, 3, 0, 4 — the reference's order, middle outward.
            for (mask, s) in [
                (0x40, 6),
                (0x20, 5),
                (0x4, 2),
                (0x2, 1),
                (0x8, 3),
                (0x1, 0),
                (0x10, 4),
            ] {
                if filter & mask != 0 {
                    return self.answer_place(side, loc, s);
                }
            }
            0
        } else if filter & 0x1 != 0 {
            6
        } else if filter & 0x2 != 0 {
            7
        } else {
            0
        };
        let _ = player;
        self.answer_place(side, loc, seq);
    }

    fn answer_place(&mut self, side: i8, loc: u8, sequence: i8) {
        self.core.returns.set_i8(0, side);
        self.core.returns.set_i8(1, loc as i8);
        self.core.returns.set_i8(2, sequence);
    }
}

impl Field {
    /// `parse_response_cards` — decode a card-selection answer.
    ///
    /// The answer is variable-width, and the encoding is the interesting
    /// part. Slot 0 is a **type tag**:
    ///
    /// | type | |
    /// |---|---|
    /// | -1 | the player cancelled |
    /// | 0 | the indices are `u32` |
    /// | 1 | the indices are `u16` |
    /// | 2 | the indices are `u8` |
    /// | 3 | a bitset, one bit per offered card |
    ///
    /// Anything else is rejected, and the reference's test for that is worth
    /// transcribing rather than reinventing: `(type + 1) > 4` **unsigned**,
    /// which catches both ends at once because a type below -1 wraps to a
    /// huge number.
    ///
    /// ## All three index widths begin at byte 8
    ///
    /// The count sits at `u32` slot 1 — bytes 4-7 — so the indices start at
    /// byte 8 whatever their width. The reference writes that as slot `i+2`
    /// for `u32`, `i+4` for `u16`, `i+8` for `u8`, and those are the *same
    /// byte offset* once each is scaled: `(i+2)*4`, `(i+4)*2`, `(i+8)*1`.
    /// The three magic numbers are not three conventions, they are one.
    ///
    /// The bitset case starts at bit 32 for the same reason — after the
    /// four-byte type tag.
    ///
    /// ## Duplicates make the answer illegal, but the list is still trimmed
    ///
    /// The reference sorts, uniques, **returns false if anything was
    /// removed**, and resizes anyway. So a duplicate answer is rejected and
    /// retried — and the sort means the list a caller eventually receives is
    /// in `CardId` order, not the order the player named them.
    pub(crate) fn parse_response_cards(&mut self, cancelable: bool) -> bool {
        self.core.return_cards.list.clear();
        let ty = self.core.returns.at_i32(0);
        // `(type + 1) > 4` unsigned: catches type < -1 by wrapping.
        if (ty.wrapping_add(1) as u32) > 4 {
            return false;
        }
        if ty == -1 {
            if cancelable {
                self.core.return_cards.canceled = true;
                return true;
            }
            return false;
        }
        let offered = self.core.select_cards.clone();
        let mut list: Vec<CardId> = Vec::new();
        if ty == 3 {
            for (i, &card) in offered.iter().enumerate() {
                if self.core.returns.bit_get(i + 32) {
                    list.push(card);
                }
            }
        } else {
            let size = self.core.returns.at_u32(1);
            for i in 0..size as usize {
                let idx = match ty {
                    0 => self.core.returns.at_u32(i + 2) as usize,
                    1 => self.core.returns.at_u16(i + 4) as usize,
                    _ => self.core.returns.at_u8(i + 8) as usize,
                };
                if idx >= offered.len() {
                    return false;
                }
                list.push(offered[idx]);
            }
        }
        let before = list.len();
        list.sort_unstable();
        list.dedup();
        let no_duplicates = list.len() == before;
        self.core.return_cards.list = list;
        no_duplicates
    }

    /// `field::process(Processors::SelectCard&)` — choose cards from
    /// `core.select_cards`.
    ///
    /// Two short-circuits, both of which finish without asking: a maximum of
    /// zero, and nothing on offer.
    ///
    /// The bounds are **narrowed to what is actually available** before the
    /// question is asked — `max` down to the number offered, then `min` down
    /// to `max`. A caller asking for "two or three" when one card exists is
    /// asking for one, not failing.
    ///
    /// The offered list is **sorted into `card_operation_sort` order before
    /// being shown**, which is what makes the indices the host answers with
    /// meaningful: they index that order, not the order the caller happened
    /// to collect them in.
    pub(crate) fn select_card_step(
        &mut self,
        step: u16,
        player: u8,
        cancelable: bool,
        min: &mut u8,
        max: &mut u8,
    ) -> bool {
        if step == 0 {
            self.core.return_cards.clear();
            self.core.returns.clear();
            if *max == 0 || self.core.select_cards.is_empty() {
                self.messages.push(Message::Hint {
                    kind: hint::SELECTMSG,
                    player,
                    value: 0,
                });
                return true;
            }
            // Narrow to what is available, and write it back: step 1
            // validates against these, not against what the caller asked.
            *max = (*max).min(self.core.select_cards.len() as u8);
            *min = (*min).min(*max);
            let (min, max) = (*min, *max);
            if self.simple_ai_answers_for(player) {
                self.core.return_cards.list = self
                    .core
                    .select_cards
                    .iter()
                    .copied()
                    .take(min as usize)
                    .collect();
                return true;
            }
            let mut offered = std::mem::take(&mut self.core.select_cards);
            offered.sort_by(|&a, &b| {
                if self.card_operation_sort(a, b) {
                    std::cmp::Ordering::Less
                } else if self.card_operation_sort(b, a) {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            });
            self.core.select_cards = offered;
            self.messages.push(Message::SelectCard {
                player,
                cancelable: cancelable || min == 0,
                min,
                max,
                cards: self.core.select_cards.clone(),
            });
            return false;
        }

        let (min, max) = (*min, *max);
        if !self.parse_response_cards(cancelable || min == 0) {
            self.core.return_cards.clear();
            self.messages.push(Message::Retry);
            return false;
        }
        if self.core.return_cards.canceled {
            return true;
        }
        let chosen = self.core.return_cards.list.len();
        if chosen < min as usize || chosen > max as usize {
            self.core.return_cards.clear();
            self.messages.push(Message::Retry);
            return false;
        }
        true
    }
}

impl Field {
    /// `field::process(Processors::SelectTributeP&)` — choose tributes,
    /// where each card is worth a different number of them.
    ///
    /// The "P" is for the plain form: a single question, unlike
    /// `SelectTribute`, which builds a selection one card at a time.
    ///
    /// ## The bounds are narrowed three ways, and one of them is a constant
    ///
    /// `max` is capped at **5** before anything else — the number of Monster
    /// Zones, and therefore the most tributes that can ever be offered — then
    /// at the total the offered cards are *worth*, then `min` is brought down
    /// to `max`. The 5 is the reference's literal, and it is a rule rather
    /// than a buffer size: a caller asking for six tributes is asking for
    /// five.
    ///
    /// ## Counting is by worth, not by card
    ///
    /// The validation is the whole point of this unit existing separately
    /// from `SelectCard`: the **count** of chosen cards is checked against
    /// `max`, but their **total worth** is checked against `min`. A single
    /// double-tribute monster satisfies a minimum of two while being one
    /// card, and choosing three cards is too many even if they are worth
    /// only three between them.
    pub(crate) fn select_tribute_p_step(
        &mut self,
        step: u16,
        player: u8,
        cancelable: bool,
        min: &mut u8,
        max: &mut u8,
    ) -> bool {
        if step == 0 {
            self.core.returns.clear();
            self.core.return_cards.clear();
            if *max == 0 || self.core.select_cards.is_empty() {
                self.messages.push(Message::Hint {
                    kind: hint::SELECTMSG,
                    player,
                    value: 0,
                });
                return true;
            }
            let worth: u32 = self
                .core
                .select_cards
                .iter()
                .map(|&c| self.cards[c].release_param)
                .sum();
            // Five zones, so never more than five tributes.
            *max = (*max).min(5);
            *max = (*max).min(worth as u8);
            *min = (*min).min(*max);
            let (min_v, max_v) = (*min, *max);

            let mut offered = std::mem::take(&mut self.core.select_cards);
            offered.sort_by(|&a, &b| {
                if self.card_operation_sort(a, b) {
                    std::cmp::Ordering::Less
                } else if self.card_operation_sort(b, a) {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            });
            self.core.select_cards = offered;
            let cards = self
                .core
                .select_cards
                .iter()
                .map(|&c| (c, self.cards[c].release_param))
                .collect();
            self.messages.push(Message::SelectTribute {
                player,
                cancelable: cancelable || min_v == 0,
                min: min_v,
                max: max_v,
                cards,
            });
            return false;
        }

        let (min_v, max_v) = (*min, *max);
        if !self.parse_response_cards(cancelable || min_v == 0) {
            self.core.return_cards.clear();
            self.messages.push(Message::Retry);
            return false;
        }
        if self.core.return_cards.canceled {
            return true;
        }
        // Too many *cards*...
        if self.core.return_cards.list.len() > max_v as usize {
            self.core.return_cards.clear();
            self.messages.push(Message::Retry);
            return false;
        }
        // ...but not enough *worth*.
        let total: u32 = self
            .core
            .return_cards
            .list
            .iter()
            .map(|&c| self.cards[c].release_param)
            .sum();
        if total < u32::from(min_v) {
            self.core.return_cards.clear();
            self.messages.push(Message::Retry);
            return false;
        }
        true
    }

    /// `field::process(Processors::SelectUnselectCard&)` — choose **one**
    /// card, from what is on offer or from what is already chosen.
    ///
    /// The building block of an incremental selection: the caller shows two
    /// lists and gets back a single card, then decides what to show next.
    /// Choosing a card from `unselect_cards` is how a selection is undone.
    ///
    /// ## The two lists are one numbered sequence
    ///
    /// The answer is an index into `select` followed by `unselect`, and the
    /// unit resolves which side it lands on. A port that numbered them
    /// separately would undo a selection when the player meant to add one.
    ///
    /// ## `finishable` and `cancelable` are different permissions
    ///
    /// Both let the player answer `-1`, and the unit treats that answer the
    /// same way — but they mean different things to the caller: *cancelable*
    /// is "abandon this entirely", *finishable* is "I have chosen enough".
    /// The reference checks `cancelable || finishable` here and distinguishes
    /// them outside.
    ///
    /// ## The answer is two slots, and the first is a kind
    ///
    /// Slot 0 must be exactly `1` — anything else, including `0`, is a retry.
    /// Slot 1 is the index. That `> 1` test is not a bound on the index; it
    /// is a bound on a *tag*, and reading it as the index is the natural
    /// misreading.
    pub(crate) fn select_unselect_card_step(
        &mut self,
        step: u16,
        player: u8,
        cancelable: bool,
        min: u8,
        max: u8,
        finishable: bool,
    ) -> bool {
        if step == 0 {
            self.core.return_cards.clear();
            self.core.returns.clear();
            if self.core.select_cards.is_empty() && self.core.unselect_cards.is_empty() {
                self.messages.push(Message::Hint {
                    kind: hint::SELECTMSG,
                    player,
                    value: 0,
                });
                return true;
            }
            if self.simple_ai_answers_for(player) {
                if cancelable {
                    self.core.return_cards.canceled = true;
                } else {
                    let pick = self
                        .core
                        .select_cards
                        .first()
                        .copied()
                        .or_else(|| self.core.unselect_cards.first().copied());
                    if let Some(c) = pick {
                        self.core.return_cards.list.push(c);
                    }
                }
                return true;
            }
            let mut offered = std::mem::take(&mut self.core.select_cards);
            offered.sort_by(|&a, &b| {
                if self.card_operation_sort(a, b) {
                    std::cmp::Ordering::Less
                } else if self.card_operation_sort(b, a) {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            });
            self.core.select_cards = offered;
            self.messages.push(Message::SelectUnselectCard {
                player,
                finishable,
                cancelable,
                min,
                max,
                select: self.core.select_cards.clone(),
                unselect: self.core.unselect_cards.clone(),
            });
            return false;
        }

        let tag = self.core.returns.at_i32(0);
        if tag == -1 {
            if cancelable || finishable {
                self.core.return_cards.canceled = true;
                return true;
            }
            self.core.return_cards.clear();
            self.messages.push(Message::Retry);
            return false;
        }
        // Slot 0 is a *kind*, and the only legal kind is 1.
        if tag == 0 || tag > 1 {
            self.core.return_cards.clear();
            self.messages.push(Message::Retry);
            return false;
        }
        let total = self.core.select_cards.len() + self.core.unselect_cards.len();
        let idx = self.core.returns.at_i32(1);
        if idx < 0 || idx as usize >= total {
            self.messages.push(Message::Retry);
            return false;
        }
        let idx = idx as usize;
        // One sequence: past the end of `select` is an index into `unselect`.
        let chosen = if idx >= self.core.select_cards.len() {
            self.core.unselect_cards[idx - self.core.select_cards.len()]
        } else {
            self.core.select_cards[idx]
        };
        self.core.return_cards.list.push(chosen);
        true
    }

    /// `field::process(Processors::SortCard&)` — put these in an order.
    ///
    /// The answer is **one slot per card**, not a single index: slot `i`
    /// holds where card `i` should go. Validation therefore checks the
    /// whole array is a permutation — every value in range and none
    /// repeated — which is the only `Select*` that validates a shape
    /// rather than a value.
    ///
    /// `-1` in slot 0 means "don't sort", and is accepted before the
    /// permutation check runs.
    pub(crate) fn sort_card_step(&mut self, step: u16, player: u8, is_chain: bool) -> bool {
        if step == 0 {
            self.core.returns.clear();
            if self.simple_ai_answers_for(player) {
                self.core.returns.set_i8(0, -1);
                return true;
            }
            // **An empty list is not a question.** The hint is emitted with
            // a zero description and the unit finishes, leaving `returns`
            // cleared rather than answered.
            if self.core.select_cards.is_empty() {
                self.messages.push(Message::Hint {
                    kind: hint::SELECTMSG,
                    player,
                    value: 0,
                });
                return true;
            }
            let cards = self
                .core
                .select_cards
                .iter()
                .map(|&c| crate::field::IdleOffer {
                    code: self.cards[c].data.code,
                    controller: self.cards[c].current.controller,
                    location: self.cards[c].current.location,
                    sequence: self.cards[c].current.sequence,
                })
                .collect();
            self.messages.push(Message::Sort {
                player,
                is_chain,
                cards,
            });
            return false;
        }
        if self.core.returns.at_i8(0) == -1 {
            return true;
        }
        let n = self.core.select_cards.len();
        let mut seen = vec![false; n];
        for i in 0..n {
            let v = self.core.returns.at_i8(i);
            // **`v < 0` is load-bearing in the reference and redundant
            // here**, and the difference is the cast. The reference
            // compares `int8_t v` against a `uint8_t` size, which promotes
            // both to `int` — so `-2 >= 3` is false and a negative would
            // slip through without this test. In Rust `v as usize` on a
            // negative sign-extends to an enormous number, so the range
            // test catches it anyway.
            //
            // Kept because the redundancy is a property of the cast, not
            // of the rule: a later change to either would make the guard
            // matter again, with nothing to notice.
            if v < 0 || v as usize >= n || seen[v as usize] {
                self.messages.push(Message::Retry);
                return false;
            }
            seen[v as usize] = true;
        }
        true
    }

    /// `field::process(Processors::SortChain&)` — let a player choose the
    /// order their own chain links resolve in.
    ///
    /// **Which list is sorted is decided by comparing against the turn
    /// player**, not passed in: the turn player sorts `tpchain`, the other
    /// sorts `ntpchain`.
    pub(crate) fn sort_chain_step(&mut self, step: u16, player: u8) -> bool {
        let is_turn_player = player == self.infos.turn_player;
        if step == 0 {
            let chains: &crate::chain::ChainList = if is_turn_player {
                &self.core.tpchain
            } else {
                &self.core.ntpchain
            };
            self.core.select_cards = chains
                .iter()
                .filter_map(|c| {
                    self.effects
                        .get(c.triggering_effect)
                        .and_then(|e| e.get_handler(&self.cards))
                })
                .collect();
            self.emplace(crate::processor::Kind::SortCard {
                player,
                is_chain: true,
            });
            return false;
        }
        if self.core.returns.at_i8(0) == -1 {
            return true;
        }
        // The answer is a permutation: slot `i` says where chain `i` goes.
        let chains = if is_turn_player {
            &mut self.core.tpchain
        } else {
            &mut self.core.ntpchain
        };
        let mut order: Vec<(i8, crate::chain::Chain)> = chains
            .drain(..)
            .enumerate()
            .map(|(i, c)| (self.core.returns.at_i8(i), c))
            .collect();
        order.sort_by_key(|(k, _)| *k);
        let sorted: crate::chain::ChainList = order.into_iter().map(|(_, c)| c).collect();
        if is_turn_player {
            self.core.tpchain = sorted;
        } else {
            self.core.ntpchain = sorted;
        }
        true
    }

    /// `field::process(Processors::SelectIdleCmd&)` — the Main Phase menu.
    ///
    /// The largest question in the engine and the only one that offers
    /// *heterogeneous* choices: five lists of cards, a list of effects, and
    /// three plain options. `IdleCommand` has already built every one of
    /// them into `core`; this unit only presents them and validates what
    /// comes back.
    ///
    /// ## It never short-circuits
    ///
    /// Every other `Select*` answers outright when there is no real choice.
    /// This one does not — not even when the menu is empty, which can
    /// happen. The reference has no such branch, and the reason is that the
    /// menu is *never* empty in practice: `to_ep` is false only when a
    /// battle is available, so there is always at least one way out of the
    /// phase. A port that added a short-circuit here would be guessing at a
    /// case the reference has decided cannot arise.
    ///
    /// ## The sort happens on the way out, and it is the answer's meaning
    ///
    /// `core.select_chains` is sorted by `chain_operation_sort` **here**,
    /// inside the question, not where the list was built. So the index the
    /// host replies with means the sorted order, and `IdleCommand` reads
    /// back from the same sorted list. Sorting in the builder instead would
    /// be equivalent only by accident.
    ///
    /// ## `can_shuffle` is a permission, not a capability
    ///
    /// Shuffling the hand is offered only while `infos.can_shuffle` holds
    /// **and** the hand has more than one card. The first is spent once per
    /// Main Phase — `IdleCommand` clears it on use — and the second is the
    /// obvious degenerate case. Both are re-tested in the validation, which
    /// matters because the host could answer after something changed.
    pub(crate) fn select_idle_cmd_step(&mut self, step: u16, player: u8) -> bool {
        if step == 0 {
            let summonable = self.idle_offers(&self.core.summonable_cards.clone());
            let spsummonable = self.idle_offers(&self.core.spsummonable_cards.clone());
            let repositionable = self.idle_offers(&self.core.repositionable_cards.clone());
            let msetable = self.idle_offers(&self.core.msetable_cards.clone());
            let ssetable = self.idle_offers(&self.core.ssetable_cards.clone());
            // `chain_operation_sort`: by the effect's id, which is the order
            // the effects were created in. Same comparator `SelectChain`
            // uses.
            let mut offered: Vec<crate::chain::Chain> = self.core.select_chains.drain(..).collect();
            offered.sort_by_key(|c| {
                self.effects
                    .get(c.triggering_effect)
                    .map_or(0, |e| e.id.get())
            });
            self.core.select_chains = offered.into();
            let activatable = self
                .core
                .select_chains
                .iter()
                .map(|c| {
                    let effect = c.triggering_effect;
                    let handler = self
                        .effects
                        .get(effect)
                        .and_then(|e| e.get_handler(&self.cards));
                    crate::field::ChainOffer {
                        code: handler.map_or(0, |h| self.cards[h].data.code),
                        info: handler
                            .map(|h| self.get_info_location(h))
                            .unwrap_or_default(),
                        description: self.effects.get(effect).map_or(0, |e| e.description),
                        client_mode: self.client_mode(effect),
                    }
                })
                .collect();
            // The Battle Phase is offered from Main Phase 1 only. `to_bp`
            // can be true in Main Phase 2 — `IdleCommand` clears it there —
            // and the message tests the phase again anyway, which is the
            // reference's belt and braces, kept.
            let to_bp = self.infos.phase == crate::duel::phases::MAIN1 && self.core.to_bp;
            self.messages.push(Message::SelectIdleCmd {
                player,
                summonable,
                spsummonable,
                repositionable,
                msetable,
                ssetable,
                activatable,
                to_bp,
                to_ep: self.core.to_ep,
                can_shuffle: self.can_shuffle_hand(player),
            });
            return false;
        }
        let answer = self.core.returns.get();
        let kind = (answer as u32) & 0xffff;
        let index = ((answer as u32) >> 16) as usize;
        let legal = match kind {
            0 => index < self.core.summonable_cards.len(),
            1 => index < self.core.spsummonable_cards.len(),
            2 => index < self.core.repositionable_cards.len(),
            3 => index < self.core.msetable_cards.len(),
            4 => index < self.core.ssetable_cards.len(),
            5 => index < self.core.select_chains.len(),
            6 => self.infos.phase == crate::duel::phases::MAIN1 && self.core.to_bp,
            7 => self.core.to_ep,
            8 => self.can_shuffle_hand(player),
            _ => false,
        };
        if !legal {
            self.messages.push(Message::Retry);
            return false;
        }
        true
    }

    /// May this player shuffle their hand right now? Asked twice — once to
    /// build the offer and once to validate the answer — so it is one
    /// function rather than two copies of the same conjunction.
    fn can_shuffle_hand(&self, player: u8) -> bool {
        self.infos.can_shuffle && self.players[player as usize].hand.len() > 1
    }

    /// The four fields `MSG_SELECT_IDLECMD` writes per card. **Not**
    /// `get_info_location`: this reads `current` directly, so an Xyz
    /// material is reported where it physically is rather than where its
    /// overlay owner is. Nothing in the Main Phase's lists can be an
    /// overlay, so the two agree — but the reference writes `current`, and
    /// that is what is transcribed.
    fn idle_offers(&self, cards: &[CardId]) -> Vec<crate::field::IdleOffer> {
        cards
            .iter()
            .map(|&c| crate::field::IdleOffer {
                code: self.cards[c].data.code,
                controller: self.cards[c].current.controller,
                location: self.cards[c].current.location,
                sequence: self.cards[c].current.sequence,
            })
            .collect()
    }

    /// One step of `AnnounceRace` — declare `count` monster Types out of
    /// the `available` mask.
    ///
    /// ## A count of zero is not a question
    ///
    /// It emits a `HINT_SELECTMSG` of **0** and finishes. That looks like
    /// a stray message and is the reference's way of clearing a select
    /// prompt that was put up in expectation of a question that turned out
    /// not to be asked. Skipping it leaves the prompt on screen.
    ///
    /// ## The answer is validated twice, and both are retries
    ///
    /// A mask with bits outside `available`, or with the wrong *number* of
    /// bits, is refused with `MSG_RETRY` rather than corrected. Checking
    /// only the count would accept a legal-sized answer naming races the
    /// prompt never offered.
    ///
    /// ## Sixty-four bits
    ///
    /// The race table outgrew 32, so the answer is read with
    /// `at<uint64_t>`. Reading it as a `u32` silently drops every race
    /// above the thirty-second and the popcount check then fails for a
    /// reason that has nothing to do with the answer.
    pub(crate) fn announce_race_step(
        &mut self,
        step: u16,
        playerid: u8,
        count: u8,
        available: u64,
    ) -> bool {
        if step == 0 {
            if count == 0 {
                self.messages.push(Message::Hint {
                    kind: hint::SELECTMSG,
                    player: playerid,
                    value: 0,
                });
                return true;
            }
            self.messages.push(Message::AnnounceRace {
                player: playerid,
                count,
                available,
            });
            return false;
        }

        let selected = self.core.returns.at_u64(0);
        if selected & !available != 0 || u32::from(count) != selected.count_ones() {
            self.messages.push(Message::Retry);
            return false;
        }
        self.messages.push(Message::Hint {
            kind: hint::RACE,
            player: playerid,
            value: selected,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_type, status, Card, CardData};
    use crate::processor::{Kind, Status};

    fn field() -> Field {
        Field::new(8000)
    }

    fn card_in(f: &mut Field, loc: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 18036057,
                type_: card_type::MONSTER,
                ..Default::default()
            },
            0,
        );
        c.current.controller = 0;
        c.current.position = position::FACEUP_ATTACK;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        let seat = f.cards.len() as u32 - 1;
        f.add_card(0, id, loc, seat, false);
        id
    }

    /// Run until the unit yields or finishes.
    fn run(f: &mut Field) -> Status {
        for _ in 0..64 {
            match f.process() {
                Status::Continue => continue,
                other => return other,
            }
        }
        panic!("did not settle");
    }

    /// **A sampled coin is the generator's coin.** The solver mode asked
    /// and answered by `sample_chance` rolls what the faithful mode rolls
    /// from the same seed, and leaves the generator where it would be.
    #[test]
    fn a_sampled_coin_is_the_generators_coin() {
        let mut plain = field();
        plain.toss_coin(None, 0, 0, 3);
        assert!(!matches!(run(&mut plain), Status::Awaiting));
        let rolled = plain
            .messages
            .iter()
            .find_map(|m| match m {
                Message::TossCoin { results, .. } => Some(results.clone()),
                _ => None,
            })
            .expect("the faithful toss");

        let mut f = field();
        f.set_chance_mode(true);
        f.toss_coin(None, 0, 0, 3);
        assert_eq!(run(&mut f), Status::Awaiting);
        assert!(matches!(
            f.messages.last(),
            Some(Message::SelectCoin { .. })
        ));
        assert!(f.sample_chance(), "a chance question was outstanding");
        assert!(
            !matches!(run(&mut f), Status::Awaiting),
            "and it was answered"
        );
        let sampled = f
            .messages
            .iter()
            .find_map(|m| match m {
                Message::TossCoin { results, .. } => Some(results.clone()),
                _ => None,
            })
            .expect("the sampled toss");
        assert_eq!(sampled, rolled, "the same faces");
        assert_eq!(f.rng, plain.rng, "and the generator is where it would be");
    }

    /// **A sampled random pick is the generator's pick.** Same shape: the
    /// picks and the generator's state both match the faithful mode's.
    #[test]
    fn a_sampled_random_pick_is_the_generators_pick() {
        let mut plain = field();
        let cards: Vec<CardId> = (0..4)
            .map(|_| card_in(&mut plain, location::HAND))
            .collect();
        crate::script_api::random_select_request(&mut plain, &cards, 0, 2);
        let rolled = crate::script_api::random_selected(&mut plain);
        assert_eq!(rolled.len(), 2);

        let mut f = field();
        let cards: Vec<CardId> = (0..4).map(|_| card_in(&mut f, location::HAND)).collect();
        f.set_chance_mode(true);
        crate::script_api::random_select_request(&mut f, &cards, 0, 2);
        assert_eq!(run(&mut f), Status::Awaiting);
        assert!(matches!(
            f.messages.last(),
            Some(Message::SelectRandom { .. })
        ));
        assert!(f.sample_chance());
        assert!(!matches!(run(&mut f), Status::Awaiting));
        let sampled = crate::script_api::random_selected(&mut f);
        assert_eq!(sampled, rolled, "the same picks");
        assert_eq!(f.rng, plain.rng, "and the generator is where it would be");
    }

    /// Nothing outstanding, or a player's question: `sample_chance`
    /// answers nothing and says so.
    #[test]
    fn sample_chance_declines_anything_but_a_chance_question() {
        let mut f = field();
        assert!(!f.sample_chance(), "nothing outstanding");
        f.set_chance_mode(true);
        f.emplace(Kind::SelectYesNo {
            player: 0,
            description: 0,
        });
        assert_eq!(run(&mut f), Status::Awaiting);
        f.core.returns.set(7);
        assert!(!f.sample_chance(), "a player's question");
        assert_eq!(f.core.returns.get(), 7, "left untouched");
    }

    fn sent(f: &Field, want: &str) -> bool {
        f.messages
            .iter()
            .any(|m| format!("{m:?}").starts_with(want))
    }

    /// The round trip the whole interface rests on: the unit yields, the
    /// host writes an answer, the unit accepts it and finishes.
    #[test]
    fn a_question_yields_then_accepts_an_answer() {
        let mut f = field();
        f.emplace(Kind::SelectYesNo {
            player: 0,
            description: 42,
        });
        assert_eq!(run(&mut f), Status::Awaiting, "it stopped to ask");
        assert!(sent(&f, "SelectYesNo"));
        assert_eq!(
            f.core.returns.get(),
            -1,
            "the unanswered sentinel, so a missing answer fails validation"
        );

        f.core.returns.set(1);
        assert_eq!(run(&mut f), Status::End, "answered, and the queue drained");
        assert_eq!(f.core.returns.get(), 1);
    }

    /// An illegal answer is not an error: the same question stands, and the
    /// unit does not advance.
    #[test]
    fn an_illegal_answer_is_asked_again() {
        let mut f = field();
        f.emplace(Kind::SelectYesNo {
            player: 0,
            description: 1,
        });
        assert_eq!(run(&mut f), Status::Awaiting);

        f.core.returns.set(7);
        assert_eq!(run(&mut f), Status::Awaiting, "still waiting");
        assert!(sent(&f, "Retry"));

        f.core.returns.set(0);
        assert_eq!(run(&mut f), Status::End);
    }

    mod select_chain {
        use super::*;
        use crate::chain::Chain;
        use crate::effect::{effect_type, flag, Effect};
        use crate::event::{code, Event};

        /// An offered chain on a card, with a chosen effect id.
        fn offer(f: &mut Field, id: u32, ty: u16, flags: u32) -> CardId {
            let card = card_in(f, location::MZONE);
            let mut e = Effect::new(ty, code::FREE_CHAIN);
            e.owner = Some(card);
            e.handler = Some(card);
            e.flag[0] = flags;
            e.description = u64::from(id) + 500;
            let eid = f.new_effect(e);
            if let Some(e) = f.effects.get_mut(eid) {
                e.id.set(id);
            }
            let mut chain = Chain::new(eid, Event::new(code::FREE_CHAIN));
            chain.triggering_player = 0;
            f.core.select_chains.push_back(chain);
            card
        }

        /// Step 0 pre-sets the declining answer and asks.
        #[test]
        fn the_declining_answer_is_written_before_the_question() {
            let mut f = field();
            offer(&mut f, 1, effect_type::QUICK_O, 0);
            assert!(!f.select_chain_step(0, 0, 0, false), "it yields");
            assert_eq!(f.core.returns.get(), -1, "declining is the default");
            assert!(sent(&f, "SelectChain"));
        }

        /// An unanswered offer reads as declining — the polarity that is the
        /// opposite of every other question here.
        #[test]
        fn leaving_it_unanswered_declines() {
            let mut f = field();
            offer(&mut f, 1, effect_type::QUICK_O, 0);
            f.select_chain_step(0, 0, 0, false);
            assert!(f.select_chain_step(1, 0, 0, false), "accepted");
            assert_eq!(f.core.returns.get(), -1);
        }

        /// A **forced** offer is a choice of order, not of whether, so the
        /// same answer is rejected.
        #[test]
        fn a_forced_offer_cannot_be_declined() {
            let mut f = field();
            offer(&mut f, 1, effect_type::QUICK_O, 0);
            offer(&mut f, 2, effect_type::QUICK_O, 0);
            f.select_chain_step(0, 0, 0, true);
            assert!(!f.select_chain_step(1, 0, 0, true), "retried");
            assert!(sent(&f, "Retry"));
        }

        #[test]
        fn an_answer_past_the_end_is_retried() {
            let mut f = field();
            offer(&mut f, 1, effect_type::QUICK_O, 0);
            f.select_chain_step(0, 0, 0, false);
            f.core.returns.set(1);
            assert!(!f.select_chain_step(1, 0, 0, false));
            assert!(sent(&f, "Retry"));

            f.core.returns.set(0);
            assert!(f.select_chain_step(1, 0, 0, false), "in range");
        }

        /// `chain_operation_sort`: the offers are presented by effect id,
        /// whatever order they were gathered in.
        #[test]
        fn the_offers_are_sorted_by_effect_id() {
            let mut f = field();
            offer(&mut f, 9, effect_type::QUICK_O, 0);
            offer(&mut f, 3, effect_type::QUICK_O, 0);
            f.select_chain_step(0, 0, 0, false);
            let ids: Vec<u32> = f
                .core
                .select_chains
                .iter()
                .map(|c| f.effects.get(c.triggering_effect).map_or(0, |e| e.id.get()))
                .collect();
            assert_eq!(ids, vec![3, 9]);
        }

        /// `get_client_mode`, all three arms.
        #[test]
        fn the_client_mode_says_how_to_present_the_effect() {
            let mut f = field();
            offer(&mut f, 1, effect_type::QUICK_O, flag::FIELD_ONLY);
            offer(&mut f, 2, effect_type::SINGLE, 0);
            // `EFFECT_TYPE_ACTIONS` is its own bit, not the union of the
            // action types — an activated effect carries it explicitly, and
            // one that does not is presented as a reset rather than as an
            // effect the player can pick.
            offer(&mut f, 3, effect_type::ACTIONS | effect_type::QUICK_O, 0);
            f.select_chain_step(0, 0, 0, false);
            let modes: Vec<u8> = match f.messages.last() {
                Some(Message::SelectChain { chains, .. }) => {
                    chains.iter().map(|c| c.client_mode).collect()
                }
                other => panic!("{other:?}"),
            };
            assert_eq!(
                modes,
                vec![
                    client_mode::RESOLVE,
                    client_mode::RESET,
                    client_mode::NORMAL
                ]
            );
        }

        /// Both players' hint timings ride along, the asked player's first.
        #[test]
        fn both_hint_timings_are_sent() {
            let mut f = field();
            offer(&mut f, 1, effect_type::QUICK_O, 0);
            f.core.hint_timing = [0x40, 0x80];
            f.select_chain_step(0, 1, 0, false);
            match f.messages.last() {
                Some(Message::SelectChain {
                    hint_timing,
                    opponent_hint_timing,
                    ..
                }) => {
                    assert_eq!(*hint_timing, 0x80, "the asked player's");
                    assert_eq!(*opponent_hint_timing, 0x40);
                }
                other => panic!("{other:?}"),
            }
        }
    }

    mod select_position {
        use super::*;

        /// No positions offered is not an error — it answers the default.
        #[test]
        fn no_positions_answers_faceup_attack() {
            let mut f = field();
            f.emplace(Kind::SelectPosition {
                player: 0,
                code: 1,
                positions: 0,
            });
            assert_eq!(run(&mut f), Status::End, "never asked");
            assert_eq!(f.core.returns.get(), i32::from(position::FACEUP_ATTACK));
        }

        /// The short-circuit that matters for a solver: one legal position
        /// is not a decision, and no node is created for it.
        #[test]
        fn a_single_legal_position_is_not_asked_about() {
            let mut f = field();
            f.emplace(Kind::SelectPosition {
                player: 0,
                code: 1,
                positions: position::FACEUP_DEFENSE,
            });
            assert_eq!(run(&mut f), Status::End, "answered without yielding");
            assert_eq!(f.core.returns.get(), i32::from(position::FACEUP_DEFENSE));
            assert!(!sent(&f, "SelectPosition"));
        }

        #[test]
        fn a_real_choice_is_asked_about() {
            let mut f = field();
            f.emplace(Kind::SelectPosition {
                player: 0,
                code: 1,
                positions: position::FACEUP_ATTACK | position::FACEUP_DEFENSE,
            });
            assert_eq!(run(&mut f), Status::Awaiting);
            assert!(sent(&f, "SelectPosition"));

            f.core.returns.set(i32::from(position::FACEUP_DEFENSE));
            assert_eq!(run(&mut f), Status::End);
        }

        /// An answer outside what was offered is refused.
        #[test]
        fn a_position_that_was_not_offered_is_refused() {
            let mut f = field();
            f.emplace(Kind::SelectPosition {
                player: 0,
                code: 1,
                positions: position::FACEUP_ATTACK | position::FACEUP_DEFENSE,
            });
            run(&mut f);
            f.core.returns.set(i32::from(position::FACEDOWN_DEFENSE));
            assert_eq!(run(&mut f), Status::Awaiting);
            assert!(sent(&f, "Retry"));
        }
    }

    mod select_option {
        use super::*;

        /// No options finishes with the answer left at -1, which the caller
        /// reads as "there was nothing to choose".
        #[test]
        fn no_options_finishes_without_asking() {
            let mut f = field();
            f.emplace(Kind::SelectOption { player: 0 });
            assert_eq!(run(&mut f), Status::End);
            assert_eq!(f.core.returns.get(), -1);
            assert!(sent(&f, "Hint"));
            assert!(!sent(&f, "SelectOption"));
        }

        #[test]
        fn an_out_of_range_choice_is_refused() {
            let mut f = field();
            f.core.select_options = vec![10, 20];
            f.emplace(Kind::SelectOption { player: 0 });
            assert_eq!(run(&mut f), Status::Awaiting);

            f.core.returns.set(2);
            assert_eq!(run(&mut f), Status::Awaiting, "only 0 and 1 exist");
            assert!(sent(&f, "Retry"));

            f.core.returns.set(1);
            assert_eq!(run(&mut f), Status::End);
        }
    }

    mod select_effect_yes_no {
        use super::*;

        #[test]
        fn it_carries_the_cards_location() {
            let mut f = field();
            let c = card_in(&mut f, location::MZONE);
            f.emplace(Kind::SelectEffectYesNo {
                player: 0,
                description: 97,
                card: c,
            });
            assert_eq!(run(&mut f), Status::Awaiting);
            assert!(sent(&f, "SelectEffectYesNo"));

            f.core.returns.set(1);
            assert_eq!(run(&mut f), Status::End);
        }
    }

    mod select_place {
        use super::*;

        /// A count of zero is a hint and a finish, not a question.
        #[test]
        fn a_count_of_zero_asks_nothing() {
            let mut f = field();
            f.emplace(Kind::SelectPlace {
                player: 0,
                flag: 0,
                count: 0,
                disable_field: false,
            });
            assert_eq!(run(&mut f), Status::End);
            assert!(!sent(&f, "SelectPlace"));
        }

        /// Answer with player, location, sequence — three `i8` slots.
        fn answer(f: &mut Field, player: i8, loc: u8, seq: i8) {
            f.core.returns.set_i8(0, player);
            f.core.returns.set_i8(1, loc as i8);
            f.core.returns.set_i8(2, seq);
        }

        #[test]
        fn a_free_zone_is_accepted() {
            let mut f = field();
            f.emplace(Kind::SelectPlace {
                player: 0,
                flag: 0,
                count: 1,
                disable_field: false,
            });
            assert_eq!(run(&mut f), Status::Awaiting);
            answer(&mut f, 0, location::SZONE, 3);
            assert_eq!(run(&mut f), Status::End);
        }

        /// A **set** bit means unavailable — the polarity that is the
        /// opposite of the intuitive one.
        #[test]
        fn a_flagged_zone_is_refused() {
            let mut f = field();
            // bit for own SZONE seat 3 is 1 << (8 + 3).
            f.emplace(Kind::SelectPlace {
                player: 0,
                flag: 1 << 11,
                count: 1,
                disable_field: false,
            });
            run(&mut f);
            answer(&mut f, 0, location::SZONE, 3);
            assert_eq!(run(&mut f), Status::Awaiting);
            assert!(sent(&f, "Retry"));
        }

        /// The sequence bound differs by one between the rows: monster zones
        /// allow 0-6, spell/trap zones 0-7.
        #[test]
        fn the_sequence_bound_differs_between_the_rows() {
            let mut f = field();
            f.emplace(Kind::SelectPlace {
                player: 0,
                flag: 0,
                count: 1,
                disable_field: false,
            });
            run(&mut f);
            answer(&mut f, 0, location::MZONE, 7);
            assert_eq!(
                run(&mut f),
                Status::Awaiting,
                "monster zone 7 does not exist"
            );

            let mut g = field();
            g.emplace(Kind::SelectPlace {
                player: 0,
                flag: 0,
                count: 1,
                disable_field: false,
            });
            run(&mut g);
            answer(&mut g, 0, location::SZONE, 7);
            assert_eq!(run(&mut g), Status::End, "but spell/trap zone 7 does");
        }

        /// A player id above 1 is refused — and this is where reading the
        /// answer *unsigned* matters: 255 must fail the `> 1` test rather
        /// than read back as -1 and pass it.
        #[test]
        fn an_impossible_player_is_refused() {
            let mut f = field();
            f.emplace(Kind::SelectPlace {
                player: 0,
                flag: 0,
                count: 1,
                disable_field: false,
            });
            run(&mut f);
            f.core.returns.set_i8(0, -1); // reads back as 255 unsigned
            f.core.returns.set_i8(1, location::SZONE as i8);
            f.core.returns.set_i8(2, 0);
            assert_eq!(run(&mut f), Status::Awaiting);
            assert!(sent(&f, "Retry"));
        }

        /// The mask doubles as a record of what this answer already took, so
        /// the same zone cannot be chosen twice in one selection.
        #[test]
        fn the_same_zone_cannot_be_chosen_twice() {
            let mut f = field();
            f.emplace(Kind::SelectPlace {
                player: 0,
                flag: 0,
                count: 2,
                disable_field: false,
            });
            run(&mut f);
            answer(&mut f, 0, location::SZONE, 2);
            f.core.returns.set_i8(3, 0);
            f.core.returns.set_i8(4, location::SZONE as i8);
            f.core.returns.set_i8(5, 2);
            assert_eq!(run(&mut f), Status::Awaiting, "the second names the first");
            assert!(sent(&f, "Retry"));

            f.core.returns.set_i8(5, 4);
            assert_eq!(run(&mut f), Status::End, "a different seat is fine");
        }

        /// A bad location is refused outright.
        #[test]
        fn a_location_that_is_not_a_field_row_is_refused() {
            let mut f = field();
            f.emplace(Kind::SelectPlace {
                player: 0,
                flag: 0,
                count: 1,
                disable_field: false,
            });
            run(&mut f);
            answer(&mut f, 0, location::GRAVE, 0);
            assert_eq!(run(&mut f), Status::Awaiting);
        }
    }

    mod select_card {
        use super::*;

        fn offer(f: &mut Field, n: usize) -> Vec<CardId> {
            let cards: Vec<CardId> = (0..n).map(|_| card_in(f, location::HAND)).collect();
            f.core.select_cards = cards.clone();
            cards
        }

        /// Answer with `u32` indices: type 0, count at u32 slot 1, indices
        /// from u32 slot 2 — which is byte 8.
        fn answer_u32(f: &mut Field, idx: &[u32]) {
            f.core.returns.set_i32(0, 0);
            f.core.returns.set_i32(1, idx.len() as i32);
            for (i, &v) in idx.iter().enumerate() {
                f.core.returns.set_i32(i + 2, v as i32);
            }
        }

        #[test]
        fn nothing_on_offer_finishes_without_asking() {
            let mut f = field();
            f.emplace(Kind::SelectCard {
                player: 0,
                cancelable: false,
                min: 1,
                max: 1,
            });
            assert_eq!(run(&mut f), Status::End);
            assert!(!sent(&f, "SelectCard"));
        }

        #[test]
        fn a_choice_is_asked_about_and_accepted() {
            let mut f = field();
            let cards = offer(&mut f, 3);
            f.emplace(Kind::SelectCard {
                player: 0,
                cancelable: false,
                min: 1,
                max: 2,
            });
            assert_eq!(run(&mut f), Status::Awaiting);
            answer_u32(&mut f, &[0, 2]);
            assert_eq!(run(&mut f), Status::End);
            assert_eq!(f.core.return_cards.list.len(), 2);
            assert!(f.core.return_cards.list.contains(&cards[0]));
        }

        /// The bounds are narrowed to what is available: asking for two or
        /// three when one card exists is asking for one.
        #[test]
        fn the_bounds_narrow_to_what_is_offered() {
            let mut f = field();
            offer(&mut f, 1);
            f.emplace(Kind::SelectCard {
                player: 0,
                cancelable: false,
                min: 2,
                max: 3,
            });
            assert_eq!(run(&mut f), Status::Awaiting);
            answer_u32(&mut f, &[0]);
            assert_eq!(run(&mut f), Status::End, "one is enough");
        }

        /// An index past the end of the offer is refused.
        ///
        /// A minimum of **zero** for the same reason as the duplicate test:
        /// with a minimum of one, dropping the bad index leaves an empty
        /// list that the count check refuses anyway.
        #[test]
        fn an_out_of_range_index_is_refused() {
            let mut f = field();
            offer(&mut f, 2);
            f.emplace(Kind::SelectCard {
                player: 0,
                cancelable: false,
                min: 0,
                max: 2,
            });
            run(&mut f);
            answer_u32(&mut f, &[5]);
            assert_eq!(run(&mut f), Status::Awaiting);
            assert!(sent(&f, "Retry"));
        }

        /// The same card named twice is an illegal answer.
        ///
        /// The bounds are deliberately `1..=2`, not `2..=2`: with a minimum
        /// of two the *count* check would refuse the trimmed one-card list
        /// anyway, and the test would pass whether or not duplicates were
        /// detected. It has to be possible for the trimmed answer to be
        /// otherwise legal.
        #[test]
        fn a_duplicate_choice_is_refused() {
            let mut f = field();
            offer(&mut f, 3);
            f.emplace(Kind::SelectCard {
                player: 0,
                cancelable: false,
                min: 1,
                max: 2,
            });
            run(&mut f);
            answer_u32(&mut f, &[1, 1]);
            assert_eq!(run(&mut f), Status::Awaiting);
            assert!(sent(&f, "Retry"));
        }

        /// Too few is refused even when the indices are all legal.
        #[test]
        fn fewer_than_the_minimum_is_refused() {
            let mut f = field();
            offer(&mut f, 3);
            f.emplace(Kind::SelectCard {
                player: 0,
                cancelable: false,
                min: 2,
                max: 3,
            });
            run(&mut f);
            answer_u32(&mut f, &[0]);
            assert_eq!(run(&mut f), Status::Awaiting);
        }

        /// Cancelling is distinct from choosing nothing, and is only
        /// accepted when the selection allows it.
        #[test]
        fn cancelling_is_only_accepted_when_allowed() {
            let mut f = field();
            offer(&mut f, 2);
            f.emplace(Kind::SelectCard {
                player: 0,
                cancelable: false,
                min: 1,
                max: 1,
            });
            run(&mut f);
            f.core.returns.set_i32(0, -1);
            assert_eq!(run(&mut f), Status::Awaiting, "not cancelable");

            let mut g = field();
            let cards: Vec<CardId> = (0..2).map(|_| card_in(&mut g, location::HAND)).collect();
            g.core.select_cards = cards;
            g.emplace(Kind::SelectCard {
                player: 0,
                cancelable: true,
                min: 1,
                max: 1,
            });
            run(&mut g);
            g.core.returns.set_i32(0, -1);
            assert_eq!(run(&mut g), Status::End);
            assert!(g.core.return_cards.canceled);
            assert!(g.core.return_cards.list.is_empty());
        }

        /// A minimum of zero makes the selection cancelable even when the
        /// caller did not say so.
        #[test]
        fn a_minimum_of_zero_allows_cancelling() {
            let mut f = field();
            offer(&mut f, 2);
            f.emplace(Kind::SelectCard {
                player: 0,
                cancelable: false,
                min: 0,
                max: 2,
            });
            run(&mut f);
            f.core.returns.set_i32(0, -1);
            assert_eq!(run(&mut f), Status::End);
            assert!(f.core.return_cards.canceled);
        }

        /// All four index encodings name the same cards, because the count
        /// sits at u32 slot 1 and every width therefore starts at byte 8.
        #[test]
        fn the_four_encodings_agree() {
            let want = 2usize;
            // type 0: u32 indices
            let mut a = field();
            let ca = offer(&mut a, 4);
            a.emplace(Kind::SelectCard {
                player: 0,
                cancelable: false,
                min: 1,
                max: 1,
            });
            run(&mut a);
            answer_u32(&mut a, &[want as u32]);
            run(&mut a);

            // type 1: u16 indices, from u16 slot 4 == byte 8
            let mut b = field();
            let cb = offer(&mut b, 4);
            b.emplace(Kind::SelectCard {
                player: 0,
                cancelable: false,
                min: 1,
                max: 1,
            });
            run(&mut b);
            b.core.returns.set_i32(0, 1);
            b.core.returns.set_i32(1, 1);
            b.core.returns.set_i8(8, want as i8);
            b.core.returns.set_i8(9, 0);
            run(&mut b);

            // type 2: u8 indices, from u8 slot 8 == byte 8
            let mut c = field();
            let cc = offer(&mut c, 4);
            c.emplace(Kind::SelectCard {
                player: 0,
                cancelable: false,
                min: 1,
                max: 1,
            });
            run(&mut c);
            c.core.returns.set_i32(0, 2);
            c.core.returns.set_i32(1, 1);
            c.core.returns.set_i8(8, want as i8);
            run(&mut c);

            // type 3: a bitset starting at bit 32 == byte 4
            let mut d = field();
            let cd = offer(&mut d, 4);
            d.emplace(Kind::SelectCard {
                player: 0,
                cancelable: false,
                min: 1,
                max: 1,
            });
            run(&mut d);
            d.core.returns.set_i32(0, 3);
            d.core.returns.set_i8(4, 1 << want);
            run(&mut d);

            assert_eq!(a.core.return_cards.list, vec![ca[want]]);
            assert_eq!(
                b.core.return_cards.list,
                vec![cb[want]],
                "u16 indices start at byte 8 too"
            );
            assert_eq!(c.core.return_cards.list, vec![cc[want]], "and u8 indices");
            assert_eq!(
                d.core.return_cards.list,
                vec![cd[want]],
                "and the bitset at bit 32"
            );
        }

        /// A negative tag below -1 must be refused *before* it reaches the
        /// index decoding — where it would fall into the catch-all `u8` arm
        /// and name a real card. The bounds are `0..=2` so that the decoded
        /// answer would otherwise be accepted.
        #[test]
        fn a_negative_tag_does_not_decode_as_bytes() {
            let mut f = field();
            offer(&mut f, 3);
            f.emplace(Kind::SelectCard {
                player: 0,
                cancelable: false,
                min: 0,
                max: 2,
            });
            run(&mut f);
            f.core.returns.set_i32(0, -2);
            f.core.returns.set_i32(1, 1);
            f.core.returns.set_i8(8, 0);
            assert_eq!(run(&mut f), Status::Awaiting, "refused, not decoded");
            assert!(f.core.return_cards.list.is_empty());
        }

        /// A type tag outside -1..=3 is refused, including a very negative
        /// one — which is what the reference's unsigned `(type + 1) > 4`
        /// catches at both ends in a single test.
        #[test]
        fn an_impossible_type_tag_is_refused() {
            for bad in [4i32, 99, -2, i32::MIN] {
                let mut f = field();
                offer(&mut f, 2);
                f.emplace(Kind::SelectCard {
                    player: 0,
                    cancelable: false,
                    min: 1,
                    max: 1,
                });
                run(&mut f);
                f.core.returns.set_i32(0, bad);
                assert_eq!(run(&mut f), Status::Awaiting, "type {bad}");
            }
        }
    }

    mod select_tribute_p {
        use super::*;

        /// The cards go in the graveyard, not the Monster Zone: the helper
        /// seats by index and there are only seven Monster Zones, while the
        /// five-zone cap has to be tested with more cards than that. The
        /// unit reads only `release_param` and the sort order, so where they
        /// are does not matter to it.
        fn worth(f: &mut Field, n: usize, params: &[u32]) -> Vec<CardId> {
            let cards: Vec<CardId> = (0..n).map(|_| card_in(f, location::GRAVE)).collect();
            for (i, &c) in cards.iter().enumerate() {
                f.cards[c].release_param = params.get(i).copied().unwrap_or(1);
            }
            f.core.select_cards = cards.clone();
            cards
        }

        fn answer(f: &mut Field, idx: &[u32]) {
            f.core.returns.set_i32(0, 0);
            f.core.returns.set_i32(1, idx.len() as i32);
            for (i, &v) in idx.iter().enumerate() {
                f.core.returns.set_i32(i + 2, v as i32);
            }
        }

        /// The count is checked against `max` and the **worth** against
        /// `min` — which is why this unit exists apart from `SelectCard`.
        /// One double-tribute monster satisfies a minimum of two.
        #[test]
        fn one_card_can_satisfy_a_minimum_of_two() {
            let mut f = field();
            let cards = worth(&mut f, 2, &[2, 1]);
            f.emplace(Kind::SelectTributeP {
                player: 0,
                cancelable: false,
                min: 2,
                max: 2,
            });
            assert_eq!(run(&mut f), Status::Awaiting);
            // The unit **sorts** what it offers, so the answer's indices
            // refer to that order and not to the order the caller collected
            // them in. A host reads the offered list back; so does this.
            let idx = f
                .core
                .select_cards
                .iter()
                .position(|&c| c == cards[0])
                .expect("the double-worth card is on offer") as u32;
            answer(&mut f, &[idx]);
            assert_eq!(run(&mut f), Status::End, "worth two, though it is one card");
        }

        /// `max` bounds the **count**, not the worth.
        ///
        /// Two cards worth two each are four tributes' worth but only two
        /// cards, so a maximum of three accepts them. Testing with cards
        /// worth one apiece cannot tell the two readings apart, because then
        /// count and worth are the same number.
        #[test]
        fn the_maximum_bounds_the_count_not_the_worth() {
            let mut f = field();
            worth(&mut f, 2, &[2, 2]);
            f.emplace(Kind::SelectTributeP {
                player: 0,
                cancelable: false,
                min: 0,
                max: 3,
            });
            run(&mut f);
            answer(&mut f, &[0, 1]);
            assert_eq!(
                run(&mut f),
                Status::End,
                "two cards is within three, though they are worth four"
            );
        }

        /// And too many cards is refused.
        #[test]
        fn too_many_cards_is_refused() {
            let mut f = field();
            worth(&mut f, 3, &[1, 1, 1]);
            f.emplace(Kind::SelectTributeP {
                player: 0,
                cancelable: false,
                min: 2,
                max: 2,
            });
            run(&mut f);
            answer(&mut f, &[0, 1, 2]);
            assert_eq!(run(&mut f), Status::Awaiting, "three cards, max is two");
            assert!(sent(&f, "Retry"));
        }

        /// Too little worth is refused.
        ///
        /// `min` and `max` must both be 2: asking for `min: 2, max: 1` is
        /// asking for **one**, because step 0 narrows `min` down to `max`.
        /// A test written that way passes without exercising the worth
        /// check at all.
        #[test]
        fn too_little_worth_is_refused() {
            let mut f = field();
            worth(&mut f, 2, &[1, 1]);
            f.emplace(Kind::SelectTributeP {
                player: 0,
                cancelable: false,
                min: 2,
                max: 2,
            });
            run(&mut f);
            answer(&mut f, &[0]);
            assert_eq!(
                run(&mut f),
                Status::Awaiting,
                "one card, worth one, min is two"
            );
            assert!(sent(&f, "Retry"));
        }

        /// `max` is capped at five — the number of Monster Zones — before
        /// anything else. A caller asking for six is asking for five.
        #[test]
        fn the_maximum_is_capped_at_five_zones() {
            let mut f = field();
            // eight cards, each worth one
            worth(&mut f, 8, &[1; 8]);
            f.emplace(Kind::SelectTributeP {
                player: 0,
                cancelable: false,
                min: 0,
                max: 8,
            });
            run(&mut f);
            answer(&mut f, &[0, 1, 2, 3, 4, 5]);
            assert_eq!(run(&mut f), Status::Awaiting, "six is more than five");
            assert!(sent(&f, "Retry"));

            f.core.returns.clear();
            answer(&mut f, &[0, 1, 2, 3, 4]);
            assert_eq!(run(&mut f), Status::End, "five is allowed");
        }

        /// And capped again at what the offered cards are **worth**.
        ///
        /// Observable through the `min` it drags down with it: with two
        /// cards worth one each and a request for three, the cap brings
        /// `max` to 2 and then `min` to 2, so taking both satisfies it.
        /// Without the cap `min` stays at 3 and the worth check refuses a
        /// selection that is all there is to take.
        #[test]
        fn the_worth_cap_drags_the_minimum_down_with_it() {
            let mut f = field();
            worth(&mut f, 2, &[1, 1]);
            f.emplace(Kind::SelectTributeP {
                player: 0,
                cancelable: false,
                min: 3,
                max: 5,
            });
            run(&mut f);
            answer(&mut f, &[0, 1]);
            assert_eq!(
                run(&mut f),
                Status::End,
                "asking for three when two exist is asking for two"
            );
        }
    }

    mod select_unselect_card {
        use super::*;

        /// The two lists are one numbered sequence: an index past the end of
        /// `select` lands in `unselect`. Numbering them separately would undo
        /// a selection when the player meant to add one.
        #[test]
        fn the_two_lists_are_one_numbered_sequence() {
            let mut f = field();
            let a = card_in(&mut f, location::MZONE);
            let b = card_in(&mut f, location::MZONE);
            let first_chosen = card_in(&mut f, location::MZONE);
            let second_chosen = card_in(&mut f, location::MZONE);
            f.core.select_cards = vec![a, b];
            // Two already-chosen cards, and the test picks the *second*: with
            // one, an implementation that always reached for `unselect[0]`
            // would look correct.
            f.core.unselect_cards = vec![first_chosen, second_chosen];

            f.emplace(Kind::SelectUnselectCard {
                player: 0,
                cancelable: false,
                min: 0,
                max: 3,
                finishable: false,
            });
            assert_eq!(run(&mut f), Status::Awaiting);
            // `select` holds two, so index 3 is `unselect[1]`.
            f.core.returns.set_i32(0, 1);
            f.core.returns.set_i32(1, 3);
            assert_eq!(run(&mut f), Status::End);
            assert_eq!(
                f.core.return_cards.list,
                vec![second_chosen],
                "index 3 is the second of the already-chosen, not the first"
            );
        }

        /// Slot 0 is a **kind**, not the index: only `1` is legal, and `0`
        /// is a retry rather than "the first card".
        #[test]
        fn slot_zero_is_a_kind_not_an_index() {
            let mut f = field();
            let a = card_in(&mut f, location::MZONE);
            f.core.select_cards = vec![a];
            f.emplace(Kind::SelectUnselectCard {
                player: 0,
                cancelable: false,
                min: 0,
                max: 1,
                finishable: false,
            });
            run(&mut f);
            for bad in [0i32, 2, 7] {
                f.core.returns.set_i32(0, bad);
                f.core.returns.set_i32(1, 0);
                assert_eq!(run(&mut f), Status::Awaiting, "kind {bad}");
            }
            f.core.returns.set_i32(0, 1);
            f.core.returns.set_i32(1, 0);
            assert_eq!(run(&mut f), Status::End);
        }

        /// `finishable` permits the same `-1` answer as `cancelable`, even
        /// when the selection is not cancelable.
        #[test]
        fn finishable_permits_the_same_answer_as_cancelable() {
            let mut f = field();
            let a = card_in(&mut f, location::MZONE);
            f.core.select_cards = vec![a];
            f.emplace(Kind::SelectUnselectCard {
                player: 0,
                cancelable: false,
                min: 0,
                max: 1,
                finishable: true,
            });
            run(&mut f);
            f.core.returns.set_i32(0, -1);
            assert_eq!(run(&mut f), Status::End);
            assert!(f.core.return_cards.canceled);

            // Neither permission: the same answer is refused.
            let mut g = field();
            let b = card_in(&mut g, location::MZONE);
            g.core.select_cards = vec![b];
            g.emplace(Kind::SelectUnselectCard {
                player: 0,
                cancelable: false,
                min: 0,
                max: 1,
                finishable: false,
            });
            run(&mut g);
            g.core.returns.set_i32(0, -1);
            assert_eq!(run(&mut g), Status::Awaiting);
            assert!(sent(&g, "Retry"));
        }

        /// Both lists empty finishes without asking.
        #[test]
        fn nothing_on_either_list_asks_nothing() {
            let mut f = field();
            f.emplace(Kind::SelectUnselectCard {
                player: 0,
                cancelable: false,
                min: 0,
                max: 1,
                finishable: false,
            });
            assert_eq!(run(&mut f), Status::End);
            assert!(!sent(&f, "SelectUnselectCard"));
        }

        /// An index past both lists is refused.
        #[test]
        fn an_index_past_both_lists_is_refused() {
            let mut f = field();
            let a = card_in(&mut f, location::MZONE);
            f.core.select_cards = vec![a];
            f.emplace(Kind::SelectUnselectCard {
                player: 0,
                cancelable: false,
                min: 0,
                max: 1,
                finishable: false,
            });
            run(&mut f);
            f.core.returns.set_i32(0, 1);
            f.core.returns.set_i32(1, 5);
            assert_eq!(run(&mut f), Status::Awaiting);
        }
    }
    mod announce_race {
        use super::*;

        /// Race bits chosen above 32 on purpose: the table outgrew a
        /// `u32`, and a port that read the answer as one drops them.
        const HIGH: u64 = 1 << 40;
        const LOW: u64 = 1 << 3;

        fn ask(f: &mut Field, count: u8, available: u64) -> bool {
            f.announce_race_step(0, 0, count, available)
        }

        fn answer(f: &mut Field, count: u8, available: u64, picked: u64) -> bool {
            f.core.returns.set_u64(0, picked);
            f.announce_race_step(1, 0, count, available)
        }

        fn retried(f: &Field) -> bool {
            matches!(f.messages.last(), Some(Message::Retry))
        }

        /// **A count of zero is not a question.** It clears the select
        /// prompt with a hint of 0 and finishes.
        #[test]
        fn a_count_of_zero_clears_the_prompt_instead_of_asking() {
            let mut f = field();
            assert!(ask(&mut f, 0, LOW | HIGH), "the unit is finished");
            assert!(
                !f.messages
                    .iter()
                    .any(|m| matches!(m, Message::AnnounceRace { .. })),
                "nothing was asked"
            );
            assert_eq!(
                f.messages.last(),
                Some(&Message::Hint {
                    kind: hint::SELECTMSG,
                    player: 0,
                    value: 0,
                }),
                "the prompt is cleared"
            );
        }

        /// Otherwise it asks, carrying the mask as offered.
        #[test]
        fn it_asks_with_the_available_mask() {
            let mut f = field();
            assert!(!ask(&mut f, 2, LOW | HIGH), "the unit waits for an answer");
            assert_eq!(
                f.messages.last(),
                Some(&Message::AnnounceRace {
                    player: 0,
                    count: 2,
                    available: LOW | HIGH,
                })
            );
        }

        /// **A legal answer is echoed back as `HINT_RACE`.**
        #[test]
        fn a_legal_answer_is_announced() {
            let mut f = field();
            assert!(answer(&mut f, 2, LOW | HIGH, LOW | HIGH));
            assert_eq!(
                f.messages.last(),
                Some(&Message::Hint {
                    kind: hint::RACE,
                    player: 0,
                    value: LOW | HIGH,
                })
            );
        }

        /// **A race above bit 31 survives the round trip.** Reading the
        /// answer as a `u32` drops it, and the popcount check then fails
        /// for a reason that has nothing to do with the answer.
        #[test]
        fn a_race_above_the_thirty_second_bit_is_kept() {
            let mut f = field();
            assert!(answer(&mut f, 1, HIGH, HIGH), "accepted");
            assert!(!retried(&f));
            assert_eq!(
                f.messages.last(),
                Some(&Message::Hint {
                    kind: hint::RACE,
                    player: 0,
                    value: HIGH,
                })
            );
        }

        /// **Bits outside the offer are refused**, even when the count is
        /// right — checking only the count would accept a legal-sized
        /// answer naming races never offered.
        #[test]
        fn a_race_that_was_not_offered_is_refused() {
            let mut f = field();
            assert!(!answer(&mut f, 1, LOW, HIGH), "the unit asks again");
            assert!(retried(&f));
            assert!(
                !f.messages.iter().any(|m| matches!(
                    m,
                    Message::Hint {
                        kind: hint::RACE,
                        ..
                    }
                )),
                "and announces nothing"
            );
        }

        /// **The wrong number of races is refused**, in both directions.
        #[test]
        fn the_wrong_number_of_races_is_refused() {
            for (count, picked) in [(2u8, LOW), (1, LOW | HIGH)] {
                let mut f = field();
                assert!(
                    !answer(&mut f, count, LOW | HIGH, picked),
                    "count {count} against {picked:#x}"
                );
                assert!(retried(&f), "count {count}");
            }
        }

        /// An empty answer to a real question is refused too, rather than
        /// read as declining.
        #[test]
        fn declining_is_not_an_option() {
            let mut f = field();
            assert!(!answer(&mut f, 1, LOW | HIGH, 0));
            assert!(retried(&f));
        }
    }

    mod select_idle_cmd {
        use super::*;
        use crate::duel::phases;
        use crate::field::{IdleOffer, Message};

        /// Pack an answer the way the host does: kind in the low half,
        /// index in the high half.
        fn answer(kind: u32, index: u32) -> i32 {
            ((index << 16) | kind) as i32
        }

        fn menu(f: &Field) -> Option<&Message> {
            f.messages
                .iter()
                .find(|m| matches!(m, Message::SelectIdleCmd { .. }))
        }

        /// The whole menu, with something in every list, arrives in one
        /// message — and the offers carry the card's *current* location,
        /// not a rewritten one.
        #[test]
        fn the_menu_is_one_message_carrying_every_list() {
            let mut f = field();
            f.infos.phase = phases::MAIN1;
            let a = card_in(&mut f, location::HAND);
            let b = card_in(&mut f, location::MZONE);
            f.core.summonable_cards = vec![a];
            f.core.repositionable_cards = vec![b];
            f.core.to_bp = true;
            f.core.to_ep = true;

            f.emplace(Kind::SelectIdleCmd { player: 0 });
            assert_eq!(run(&mut f), Status::Awaiting);
            let Some(Message::SelectIdleCmd {
                summonable,
                repositionable,
                spsummonable,
                to_bp,
                to_ep,
                ..
            }) = menu(&f)
            else {
                panic!("no menu");
            };
            assert_eq!(
                summonable,
                &vec![IdleOffer {
                    code: f.cards[a].data.code,
                    controller: 0,
                    location: location::HAND,
                    sequence: f.cards[a].current.sequence,
                }]
            );
            assert_eq!(repositionable.len(), 1);
            assert_eq!(repositionable[0].location, location::MZONE);
            assert!(spsummonable.is_empty(), "an empty list is still a list");
            assert!(*to_bp && *to_ep);
        }

        /// **It does not short-circuit.** Every other `Select*` answers
        /// outright when there is nothing to choose; with every list empty
        /// and every flag down this one still yields.
        #[test]
        fn an_empty_menu_still_asks() {
            let mut f = field();
            f.emplace(Kind::SelectIdleCmd { player: 0 });
            assert_eq!(run(&mut f), Status::Awaiting);
            assert!(menu(&f).is_some());
        }

        /// An index into a list is accepted when the list is that long and
        /// refused when it is not — the same kind, either side of the edge.
        #[test]
        fn an_index_is_checked_against_its_own_list() {
            let mut f = field();
            let a = card_in(&mut f, location::HAND);
            f.core.msetable_cards = vec![a];

            f.emplace(Kind::SelectIdleCmd { player: 0 });
            run(&mut f);
            f.core.returns.set(answer(3, 1));
            assert_eq!(
                run(&mut f),
                Status::Awaiting,
                "one card, so index 1 is past"
            );
            f.core.returns.set(answer(3, 0));
            assert_eq!(run(&mut f), Status::End);
        }

        /// The lists are **separate** namespaces: an index legal in one is
        /// not thereby legal in another.
        #[test]
        fn an_index_legal_in_one_list_is_not_legal_in_another() {
            let mut f = field();
            let a = card_in(&mut f, location::HAND);
            f.core.msetable_cards = vec![a];

            f.emplace(Kind::SelectIdleCmd { player: 0 });
            run(&mut f);
            f.core.returns.set(answer(4, 0));
            assert_eq!(run(&mut f), Status::Awaiting, "ssetable is empty");
        }

        /// Kind 9 and above is refused. The reference's `t > 8` — and 8 is
        /// legal, so the boundary is where it looks.
        #[test]
        fn an_unknown_kind_is_refused() {
            let mut f = field();
            f.emplace(Kind::SelectIdleCmd { player: 0 });
            run(&mut f);
            f.core.returns.set(answer(9, 0));
            assert_eq!(run(&mut f), Status::Awaiting);
        }

        /// Leaving for the Battle Phase needs **both** `to_bp` and Main
        /// Phase 1. `to_bp` alone, in Main Phase 2, is refused — and the
        /// offer is withheld too, which is the same test in the message.
        #[test]
        fn to_battle_needs_main_phase_1_as_well_as_to_bp() {
            let mut f = field();
            f.core.to_bp = true;
            f.infos.phase = phases::MAIN2;

            f.emplace(Kind::SelectIdleCmd { player: 0 });
            run(&mut f);
            let Some(Message::SelectIdleCmd { to_bp, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(!to_bp, "not offered in Main Phase 2");
            f.core.returns.set(answer(6, 0));
            assert_eq!(run(&mut f), Status::Awaiting);

            let mut f = field();
            f.core.to_bp = true;
            f.infos.phase = phases::MAIN1;
            f.emplace(Kind::SelectIdleCmd { player: 0 });
            run(&mut f);
            f.core.returns.set(answer(6, 0));
            assert_eq!(run(&mut f), Status::End);
        }

        /// Leaving for the End Phase reads `to_ep` and nothing else — no
        /// phase test, unlike the battle option — and `to_ep` down refuses.
        #[test]
        fn to_end_reads_only_to_ep() {
            for (to_ep, phase) in [(true, phases::MAIN2), (true, phases::MAIN1)] {
                let mut f = field();
                f.infos.phase = phase;
                f.core.to_ep = to_ep;
                f.emplace(Kind::SelectIdleCmd { player: 0 });
                run(&mut f);
                f.core.returns.set(answer(7, 0));
                assert_eq!(run(&mut f), Status::End, "no phase test on this one");
            }

            let mut f = field();
            f.core.to_ep = false;
            f.emplace(Kind::SelectIdleCmd { player: 0 });
            run(&mut f);
            let Some(Message::SelectIdleCmd { to_ep, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(!to_ep, "not offered");
            f.core.returns.set(answer(7, 0));
            assert_eq!(run(&mut f), Status::Awaiting, "and refused if sent anyway");
        }

        /// Shuffling needs the permission **and** more than one card, and
        /// both halves are re-tested against the answer — so a hand that
        /// shrank between the offer and the reply is caught.
        #[test]
        fn shuffling_needs_the_permission_and_a_hand_worth_shuffling() {
            let mut f = field();
            card_in(&mut f, location::HAND);
            card_in(&mut f, location::HAND);
            f.infos.can_shuffle = true;

            f.emplace(Kind::SelectIdleCmd { player: 0 });
            run(&mut f);
            let Some(Message::SelectIdleCmd { can_shuffle, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(can_shuffle);

            // The hand shrinks after the offer was made.
            f.players[0].hand.truncate(1);
            f.core.returns.set(answer(8, 0));
            assert_eq!(run(&mut f), Status::Awaiting, "re-tested, not trusted");
        }

        /// A single card in hand is not worth shuffling, permission or no.
        #[test]
        fn one_card_is_not_a_hand_to_shuffle() {
            let mut f = field();
            card_in(&mut f, location::HAND);
            f.infos.can_shuffle = true;
            f.emplace(Kind::SelectIdleCmd { player: 0 });
            run(&mut f);
            let Some(Message::SelectIdleCmd { can_shuffle, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert!(!can_shuffle);
            f.core.returns.set(answer(8, 0));
            assert_eq!(run(&mut f), Status::Awaiting);
        }

        /// The permission alone is not enough either.
        #[test]
        fn a_spent_permission_refuses_a_full_hand() {
            let mut f = field();
            card_in(&mut f, location::HAND);
            card_in(&mut f, location::HAND);
            f.infos.can_shuffle = false;
            f.emplace(Kind::SelectIdleCmd { player: 0 });
            run(&mut f);
            f.core.returns.set(answer(8, 0));
            assert_eq!(run(&mut f), Status::Awaiting);
        }

        /// **The sort is the answer's meaning.** The offers come back in
        /// effect-id order regardless of the order they were gathered in,
        /// and `core.select_chains` is left in that same order so the
        /// index the host replies with indexes the list it saw.
        #[test]
        fn the_activate_list_is_sorted_and_left_sorted() {
            use crate::chain::Chain;
            use crate::effect::{effect_type, Effect};

            let mut f = field();
            let a = card_in(&mut f, location::SZONE);
            let b = card_in(&mut f, location::SZONE);
            // `b`'s effect is created first, so it sorts first.
            let mut eb = Effect::new(effect_type::ACTIVATE, 0);
            eb.owner = Some(b);
            eb.handler = Some(b);
            eb.description = 111;
            let eb = f.new_effect(eb);
            let mut ea = Effect::new(effect_type::ACTIVATE, 0);
            ea.owner = Some(a);
            ea.handler = Some(a);
            ea.description = 222;
            let ea = f.new_effect(ea);

            // Gathered in the other order.
            for e in [ea, eb] {
                f.core
                    .select_chains
                    .push_back(Chain::new(e, crate::event::Event::new(0)));
            }

            f.emplace(Kind::SelectIdleCmd { player: 0 });
            run(&mut f);
            let Some(Message::SelectIdleCmd { activatable, .. }) = menu(&f) else {
                panic!("no menu");
            };
            assert_eq!(
                activatable
                    .iter()
                    .map(|c| c.description)
                    .collect::<Vec<_>>(),
                vec![111, 222],
                "by effect id, not by gather order"
            );
            assert_eq!(
                f.core.select_chains[0].triggering_effect, eb,
                "and the list itself is left sorted, so index 0 means the same card"
            );
        }
    }
    mod sorting {
        use super::*;
        use crate::chain::Chain;
        use crate::effect::{effect_type, Effect};
        use crate::event::{EffectId, Event};
        use crate::field::Message;

        fn cards(f: &mut Field, n: usize) -> Vec<CardId> {
            (0..n).map(|_| card_in(f, location::MZONE)).collect()
        }

        /// **An empty list is not a question**, and the unit finishes with
        /// `returns` **cleared** — not answered, and not holding whatever
        /// the previous question left.
        ///
        /// The clearing is the part worth testing: a caller that reads
        /// slot 0 afterwards (`SortChain` does, looking for the decline
        /// sentinel) would otherwise act on a stale answer.
        #[test]
        fn an_empty_list_asks_nothing_and_clears_the_answer() {
            let mut f = field();
            // A stale answer from some earlier question.
            f.core.returns.set_i8(0, 7);
            f.emplace(Kind::SortCard {
                player: 0,
                is_chain: false,
            });
            assert_eq!(run(&mut f), Status::End);
            assert!(sent(&f, "Hint"));
            assert!(!sent(&f, "Sort"));
            assert_eq!(f.core.returns.at_i8(0), 0, "cleared, not stale");
        }

        /// The ordinary case: the list is offered and a permutation is
        /// accepted.
        #[test]
        fn a_permutation_is_accepted() {
            let mut f = field();
            f.core.select_cards = cards(&mut f, 3);
            f.emplace(Kind::SortCard {
                player: 0,
                is_chain: false,
            });
            assert_eq!(run(&mut f), Status::Awaiting);
            assert!(sent(&f, "Sort"));

            for (i, v) in [2i8, 0, 1].iter().enumerate() {
                f.core.returns.set_i8(i, *v);
            }
            assert_eq!(run(&mut f), Status::End);
        }

        /// **The answer is one slot per card, and must be a permutation.**
        /// A repeated value is refused — which is the only `Select*` that
        /// validates a shape rather than a single value.
        #[test]
        fn a_repeated_value_is_refused() {
            let mut f = field();
            f.core.select_cards = cards(&mut f, 3);
            f.emplace(Kind::SortCard {
                player: 0,
                is_chain: false,
            });
            run(&mut f);
            for (i, v) in [0i8, 1, 1].iter().enumerate() {
                f.core.returns.set_i8(i, *v);
            }
            assert_eq!(run(&mut f), Status::Awaiting, "1 appears twice");

            for (i, v) in [0i8, 1, 2].iter().enumerate() {
                f.core.returns.set_i8(i, *v);
            }
            assert_eq!(run(&mut f), Status::End);
        }

        /// A value out of range is refused, either side.
        #[test]
        fn an_out_of_range_value_is_refused() {
            for bad in [-2i8, 3] {
                let mut f = field();
                f.core.select_cards = cards(&mut f, 3);
                f.emplace(Kind::SortCard {
                    player: 0,
                    is_chain: false,
                });
                run(&mut f);
                for (i, v) in [0i8, 1, bad].iter().enumerate() {
                    f.core.returns.set_i8(i, *v);
                }
                assert_eq!(run(&mut f), Status::Awaiting, "{bad} is not a slot");
            }
        }

        /// **`-1` in slot 0 means "don't sort"**, and is accepted before
        /// the permutation check runs — so the rest of the array is not
        /// looked at.
        #[test]
        fn minus_one_declines_without_validating_the_rest() {
            let mut f = field();
            f.core.select_cards = cards(&mut f, 3);
            f.emplace(Kind::SortCard {
                player: 0,
                is_chain: false,
            });
            run(&mut f);
            f.core.returns.set_i8(0, -1);
            // Deliberate nonsense in the remaining slots.
            f.core.returns.set_i8(1, 99);
            f.core.returns.set_i8(2, 99);
            assert_eq!(run(&mut f), Status::End, "declined, not validated");
        }

        /// The chain flag travels with the message, since the two sort
        /// messages differ only in the opcode the host sees.
        #[test]
        fn the_chain_flag_reaches_the_message() {
            let mut f = field();
            f.core.select_cards = cards(&mut f, 2);
            f.emplace(Kind::SortCard {
                player: 0,
                is_chain: true,
            });
            run(&mut f);
            let is_chain = f.messages.iter().find_map(|m| match m {
                Message::Sort { is_chain, .. } => Some(*is_chain),
                _ => None,
            });
            assert_eq!(is_chain, Some(true));
        }

        fn chain_of(f: &mut Field, n: usize) -> Vec<EffectId> {
            (0..n)
                .map(|_| {
                    let c = card_in(f, location::SZONE);
                    let mut e = Effect::new(effect_type::ACTIVATE, 0);
                    e.owner = Some(c);
                    e.handler = Some(c);
                    f.new_effect(e)
                })
                .collect()
        }

        /// **`SortChain` sorts the list belonging to the player asked**,
        /// decided by comparing against the turn player rather than passed
        /// in.
        #[test]
        fn the_turn_player_sorts_their_own_list() {
            let mut f = field();
            f.infos.turn_player = 0;
            let es = chain_of(&mut f, 3);
            for &e in &es {
                f.core.tpchain.push_back(Chain::new(e, Event::new(0)));
            }
            f.emplace(Kind::SortChain { player: 0 });
            assert_eq!(run(&mut f), Status::Awaiting, "it asks");

            // Reverse them.
            for (i, v) in [2i8, 1, 0].iter().enumerate() {
                f.core.returns.set_i8(i, *v);
            }
            assert_eq!(run(&mut f), Status::End);
            let order: Vec<EffectId> = f.core.tpchain.iter().map(|c| c.triggering_effect).collect();
            assert_eq!(order, vec![es[2], es[1], es[0]], "reversed");
            assert!(f.core.ntpchain.is_empty(), "and the other list untouched");
        }

        /// The non-turn player sorts the *other* list.
        #[test]
        fn the_other_player_sorts_the_other_list() {
            let mut f = field();
            f.infos.turn_player = 0;
            let es = chain_of(&mut f, 2);
            for &e in &es {
                f.core.ntpchain.push_back(Chain::new(e, Event::new(0)));
            }
            f.emplace(Kind::SortChain { player: 1 });
            run(&mut f);
            for (i, v) in [1i8, 0].iter().enumerate() {
                f.core.returns.set_i8(i, *v);
            }
            assert_eq!(run(&mut f), Status::End);
            let order: Vec<EffectId> = f
                .core
                .ntpchain
                .iter()
                .map(|c| c.triggering_effect)
                .collect();
            assert_eq!(order, vec![es[1], es[0]]);
        }

        /// **Declining leaves the order alone.**
        #[test]
        fn declining_leaves_the_order_alone() {
            let mut f = field();
            f.infos.turn_player = 0;
            let es = chain_of(&mut f, 3);
            for &e in &es {
                f.core.tpchain.push_back(Chain::new(e, Event::new(0)));
            }
            f.emplace(Kind::SortChain { player: 0 });
            run(&mut f);
            // The remaining slots hold a **real reordering**, so that
            // "declined" and "sorted" are distinguishable. Leaving them at
            // zero makes the sort a no-op and the test vacuous.
            f.core.returns.set_i8(0, -1);
            f.core.returns.set_i8(1, 2);
            f.core.returns.set_i8(2, 1);
            assert_eq!(run(&mut f), Status::End);
            let order: Vec<EffectId> = f.core.tpchain.iter().map(|c| c.triggering_effect).collect();
            assert_eq!(order, es, "unchanged, despite the slots below");
        }
    }
}
