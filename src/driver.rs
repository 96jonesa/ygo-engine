//! Running a duel: [`Driver`].
//!
//! Everything above this is a processor unit that answers questions with
//! whatever a test put in `core.returns`. This is the piece that builds a
//! duel, starts it, and keeps it moving — the first thing in the port that
//! plays a game rather than exercising one machine.
//!
//! ## The engine asks; the driver answers
//!
//! [`Field::process`] returns [`Status::Awaiting`] when the unit at the
//! front wants a player's answer, and the **last message** is the question.
//! The driver reads it, decides, writes into `core.returns`, and calls
//! `process` again. That loop is the whole interface.
//!
//! Two things about it are easy to get wrong, and both cost a test run to
//! find:
//!
//! - **`Retry` does not say what it is retrying.** An illegal answer makes
//!   the unit emit `Retry` and ask the *same* question again, but the
//!   question message is not re-sent. A driver that reads only the last
//!   message sees `Retry`, has nothing to answer, and hangs. So the
//!   outstanding question has to be remembered.
//! - **Not every message is a question.** `Hint`, `CardSelected` and the
//!   rest are announcements, and several arrive *after* the question they
//!   relate to. Scanning backwards for the most recent *question* is the
//!   only reading that works.
//!
//! ## The policy is a trait, and deliberately small
//!
//! [`Policy`] is what decides. The driver handles the protocol — which
//! question, what shape of answer, the retry bookkeeping — and the policy
//! only chooses. That split is what lets the differential harness drive
//! this engine and ocgcore from one set of decisions.

use crate::board::location;
use crate::card::{Card, CardData};
use crate::event::CardId;
use crate::field::{Field, Message};
use crate::processor::{Kind, Status};

/// What a player decides, given a question.
///
/// Every method has a default that picks the first legal answer, so a
/// policy need only override what it cares about. The defaults are chosen
/// to keep a duel *moving* — decline optional things, take the first of
/// anything compulsory — which is what makes the simplest possible duel
/// run without a policy at all.
pub trait Policy {
    /// A yes/no. Default **no**: an optional thing declined is always
    /// legal, where an optional thing accepted may not be.
    fn yes_no(&mut self, _player: u8, _description: u64) -> bool {
        false
    }

    /// Solver mode's coin question: a bitmask, bit `i` set for heads on
    /// coin `i`. Never asked in the faithful mode.
    fn coin(&mut self, _player: u8, _count: u8) -> u32 {
        0
    }

    /// Solver mode's random-selection question: `count` distinct indices
    /// into `cards`. Never asked in the faithful mode.
    fn random_pick(&mut self, _player: u8, _cards: &[CardId], count: u8) -> Vec<usize> {
        (0..usize::from(count)).collect()
    }

    /// Solver mode's deck-top question: the top `count` cards of `player`'s
    /// deck are about to be seen; `deck` is the whole pile, bottom-first.
    /// Return a permutation of it to impose, or `None` to keep the order.
    /// Never asked in the faithful mode.
    fn deck_order(&mut self, _player: u8, _count: u32, _deck: &[CardId]) -> Option<Vec<CardId>> {
        None
    }

    /// Whether to use an **optional effect** that is offering itself —
    /// `MSG_SELECT_EFFECTYN`, which names the card asking.
    ///
    /// Kept apart from `yes_no` because the two have opposite safe
    /// answers. A bare yes/no is safest declined: an optional thing
    /// refused is always legal, where accepting may not be. But an
    /// optional *trigger* declined is simply never exercised, and a
    /// policy whose job is coverage should take it.
    fn effect_yes_no(&mut self, _player: u8, _description: u64) -> bool {
        false
    }

    /// One of `core.select_options`. Default: the first.
    ///
    /// `odd_turn` is the same alternation switch the idle menu carries,
    /// and for the same reason: a card offering a choice of effects has
    /// one of them never taken if the policy always answers first.
    fn option(&mut self, _player: u8, _count: usize, _odd_turn: bool) -> usize {
        0
    }

    /// Which cards, from those offered. Default: the first `min`, which is
    /// the smallest legal answer.
    fn cards(&mut self, _player: u8, offered: &[CardId], min: u8, _max: u8) -> Vec<CardId> {
        offered.iter().take(min as usize).copied().collect()
    }

    /// Which chain link to activate, or `None` to decline. Default:
    /// decline — a driver that always chains never finishes a turn.
    fn chain(&mut self, _player: u8, _count: usize, forced: bool) -> Option<usize> {
        forced.then_some(0)
    }

    /// The Main Phase menu. `(kind, index)`, packed by the caller.
    /// Default: leave for the End Phase, or the Battle Phase if the End
    /// Phase is closed.
    fn idle(&mut self, menu: &IdleMenu) -> (u32, usize) {
        if menu.to_ep {
            (7, 0)
        } else if menu.to_bp {
            (6, 0)
        } else {
            // Neither exit is open, which happens only when a monster must
            // attack. Go to the battle.
            (6, 0)
        }
    }

    /// The Battle Phase menu. Default: leave.
    fn battle(&mut self, menu: &BattleMenu) -> (u32, usize) {
        if menu.to_m2 {
            (2, 0)
        } else {
            (3, 0)
        }
    }

    /// Where to put a card. Default: a free monster seat on the asked
    /// player's **own** side.
    ///
    /// Own side is the part that is easy to get wrong. The answer names a
    /// player, and naming the *other* one is legal — the reference checks
    /// the opponent's zones 16 bits up the same mask rather than refusing
    /// outright. So a default that hardcodes player 0 is not rejected as
    /// nonsense; it is read as "put it in player 0's field", which is
    /// forbidden for player 1's summon, and the answer is refused forever.
    fn place(&mut self, player: u8, _flag: u32) -> (u8, u8, i8) {
        (player, location::MZONE, 0)
    }

    /// Which way up. Default: face-up attack.
    fn position(&mut self, _player: u8, positions: u8) -> u8 {
        for p in [0x1u8, 0x2, 0x4, 0x8] {
            if positions & p != 0 {
                return p;
            }
        }
        0x1
    }

    /// An ordering. Default: `-1`, which declines to sort.
    fn sort(&mut self, _player: u8, _count: usize) -> Option<Vec<i8>> {
        None
    }

    /// `MSG_SELECT_UNSELECT_CARD` — toggle one card of the combined
    /// select-then-unselect list, or `None` to stop (legal only while the
    /// question says it is finishable or cancelable).
    ///
    /// The default is what the driver did before the hook existed: stop
    /// as soon as stopping is legal, otherwise toggle the first card.
    fn unselect(
        &mut self,
        _player: u8,
        _total: usize,
        finishable: bool,
        _cancelable: bool,
    ) -> Option<usize> {
        if finishable {
            None
        } else {
            Some(0)
        }
    }

    /// Declare `count` monster Types out of the `available` mask. Default:
    /// the lowest `count` bits that are actually offered.
    ///
    /// Taking the lowest *`count` bits of a `u64`* instead would name races
    /// the prompt never offered, and the engine refuses that with a retry
    /// rather than correcting it.
    fn announce_race(&mut self, _player: u8, count: u8, available: u64) -> u64 {
        let mut picked = 0u64;
        let mut left = count;
        for bit in 0..64 {
            if left == 0 {
                break;
            }
            if available & (1 << bit) != 0 {
                picked |= 1 << bit;
                left -= 1;
            }
        }
        picked
    }
}

/// What the Main Phase offered, flattened for a policy.
#[derive(Clone, Debug)]
pub struct IdleMenu {
    pub player: u8,
    /// Whether this turn is one the play policy Sets monsters on, rather
    /// than summoning them. Carried on the menu because the policy sees
    /// nothing else of the duel.
    pub set_monster_this_turn: bool,
    pub summonable: usize,
    pub spsummonable: usize,
    pub repositionable: usize,
    pub msetable: usize,
    pub ssetable: usize,
    pub activatable: usize,
    pub to_bp: bool,
    pub to_ep: bool,
    pub can_shuffle: bool,
}

/// What the Battle Phase offered.
#[derive(Clone, Debug)]
pub struct BattleMenu {
    pub player: u8,
    pub activatable: usize,
    pub attackable: usize,
    pub to_m2: bool,
    pub to_ep: bool,
}

/// A policy that takes every default: decline everything optional, leave
/// every phase as soon as allowed. The duel it plays is dull and short,
/// which is exactly what a first end-to-end test wants.
#[derive(Default)]
pub struct PassPolicy;
impl Policy for PassPolicy {}

/// A policy that actually plays: summon when it can, attack when it can.
///
/// The point of it is coverage. A duel where both players pass exercises
/// the turn structure and nothing else — it never enters `SummonRule`,
/// never reaches the damage step, never moves a card to the field. Those
/// are the units most recently ported and the ones a differential run most
/// needs to put under load, so the harness runs this one too.
///
/// Still deterministic, and still the *first* of everything offered: a
/// differential comparison needs both engines to make identical choices,
/// which rules out anything random.
#[derive(Default)]
pub struct PlayPolicy;

impl Policy for PlayPolicy {
    fn idle(&mut self, menu: &IdleMenu) -> (u32, usize) {
        // 5 = activate, 1 = special summon by a card's own printed
        // procedure, 4 = set a Spell/Trap, 3 = set a monster, 0 = summon,
        // 6 = to the Battle Phase, 7 = to the End Phase, as
        // `SelectIdleCmd` numbers them. Activation first: it is the thing
        // a vanilla duel never exercises. Then Set — a Trap can only ever
        // be activated from the row, and a Spell that is not activatable
        // now (Dark Hole on an empty field) gets its set-then-activate
        // path walked.
        //
        // **A rule summon goes second**, above every once-a-turn move,
        // because it is not one: a monster that summons itself costs the
        // turn nothing, and a policy that took it only when nothing else
        // was on offer would almost never reach it. It sits below
        // activation so that a monster already on the field still uses
        // its effect.
        //
        // **Monsters alternate by turn.** A normal summon and a monster
        // Set are the same once-a-turn move, so preferring either one
        // always would mean the other never happens — and a policy that
        // never Sets a monster never produces a face-down, so no flip
        // effect in the pool is ever reached. Alternating on the turn
        // number gives both paths every other turn. It is an arbitrary
        // rule, which is the point: this policy exists for coverage, not
        // for play, and the only thing it must be is deterministic.
        if menu.activatable > 0 {
            (5, 0)
        } else if menu.spsummonable > 0 {
            (1, 0)
        } else if menu.ssetable > 0 {
            (4, 0)
        } else if menu.msetable > 0 && menu.set_monster_this_turn {
            (3, 0)
        } else if menu.summonable > 0 {
            (0, 0)
        } else if menu.msetable > 0 {
            (3, 0)
        } else if menu.to_bp {
            (6, 0)
        } else if menu.to_ep {
            (7, 0)
        } else {
            (6, 0)
        }
    }

    /// **The branches of a multi-effect card alternate by turn**, the
    /// same device the monster Set/summon choice uses and for the same
    /// reason. Enemy Controller is the case that needs it: its
    /// position-change branch is available whenever its take-control
    /// branch is, so a policy that always answers first would never once
    /// pay the tribute.
    fn option(&mut self, _player: u8, count: usize, odd_turn: bool) -> usize {
        if odd_turn {
            count.saturating_sub(1)
        } else {
            0
        }
    }

    /// Activate the first thing offered in a response window. This is
    /// what makes a Set Trap fire; declining everything would leave the
    /// trigger effects never resolved.
    fn chain(&mut self, _player: u8, count: usize, _forced: bool) -> Option<usize> {
        (count > 0).then_some(0)
    }

    /// Use an optional effect that offers itself. Same reasoning as
    /// `chain`: an optional trigger declined is never exercised, and a
    /// card like D.D. Warrior Lady would go through a whole duel doing
    /// nothing.
    fn effect_yes_no(&mut self, _player: u8, _description: u64) -> bool {
        true
    }

    /// Accept a bare yes/no too, for the same reason and against the
    /// trait's default.
    ///
    /// The default is **no**, and rightly: a bare yes/no is asked in the
    /// middle of a resolution, and declining is always legal where
    /// accepting may not be. But that default makes a whole class of card
    /// half-dead on the harness — every "you may also do X" clause, which
    /// is usually the half that distinguishes the card from a plainer one.
    /// Dust Tornado destroys a Spell or Trap and then *offers* to Set one
    /// from the hand; with a declining policy the offer is refused in
    /// every duel ever traced, and the Set path is compared against
    /// ocgcore exactly never.
    ///
    /// This is the policy whose job is coverage, so it says yes. The
    /// passing policy keeps the default.
    fn yes_no(&mut self, _player: u8, _description: u64) -> bool {
        true
    }

    fn battle(&mut self, menu: &BattleMenu) -> (u32, usize) {
        // 1 = attack, 2 = to Main 2, 3 = to the End Phase.
        if menu.attackable > 0 {
            (1, 0)
        } else if menu.to_m2 {
            (2, 0)
        } else {
            (3, 0)
        }
    }
}

/// Why the driver stopped.
/// A player who chooses **uniformly at random among the legal answers**,
/// from a generator both engines can be handed.
///
/// This is the fuzzing policy. The scripted policies reach what they
/// were written to reach; a random one reaches what the rules allow, and
/// the differential harness turns every game it plays into a comparison.
///
/// ## The generator is the contract
///
/// `tools/differential.py` re-implements this policy against ocgcore's
/// wire protocol, drawing from the **same** generator (SplitMix64, whose
/// outputs are pinned by a test on each side) in the **same order**: one
/// draw per decision, and a fixed draw protocol for each kind of
/// question, written out on the method it belongs to. Two engines that
/// ask the same questions in the same order therefore make the same
/// choices, and the first question they disagree about is where the
/// trace diverges — which is the point. A policy that drew a different
/// number of times on the two sides would desynchronise the generators
/// and every later line would differ for no reason.
///
/// ## Every answer is legal by construction
///
/// A random answer the engine rejects is re-asked without a fresh
/// question (`Retry`), and the two engines' retry behaviour is not part
/// of the trace. So the policy never gives one: it chooses from the
/// offered lists, within `min..=max`, among the offered position bits,
/// and declines a selection only when the question says it may.
///
/// What it leaves deterministic: the seat a card is placed in (the first
/// free one, both sides), the tribute pick (in offer order until the
/// value is met), sort orders and race announcements. Randomising those
/// is a later step; none of them changes which rule fires.
#[derive(Clone, Debug)]
pub struct RandomPolicy {
    state: u64,
}

impl RandomPolicy {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// SplitMix64 — one output per call. Chosen for being ten lines in
    /// any language; the Python mirror is the same ten lines.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// One draw, reduced to `0..n`. `n == 0` draws anyway (so the two
    /// sides stay in step) and answers zero.
    pub fn below(&mut self, n: usize) -> usize {
        let r = self.next_u64();
        if n == 0 {
            0
        } else {
            (r % n as u64) as usize
        }
    }
}

impl Policy for RandomPolicy {
    /// One draw: odd is yes.
    fn yes_no(&mut self, _player: u8, _description: u64) -> bool {
        self.next_u64() % 2 == 1
    }

    /// One draw: odd is yes.
    fn effect_yes_no(&mut self, _player: u8, _description: u64) -> bool {
        self.next_u64() % 2 == 1
    }

    /// One draw over the options.
    fn option(&mut self, _player: u8, count: usize, _odd_turn: bool) -> usize {
        self.below(count)
    }

    /// One draw for how many (`min..=max`, capped at the offer), then a
    /// partial Fisher–Yates over the offer's indices with one draw per
    /// pick; the picks are reported in ascending index order.
    fn cards(&mut self, _player: u8, offered: &[CardId], min: u8, max: u8) -> Vec<CardId> {
        let n = offered.len();
        let lo = usize::from(min).min(n);
        let hi = usize::from(max).min(n).max(lo);
        let k = lo + self.below(hi - lo + 1);
        let mut idx: Vec<usize> = (0..n).collect();
        for i in 0..k {
            let j = i + self.below(n - i);
            idx.swap(i, j);
        }
        let mut picks: Vec<usize> = idx[..k].to_vec();
        picks.sort_unstable();
        picks.into_iter().map(|i| offered[i]).collect()
    }

    /// One draw over the chains **plus one** for declining, unless the
    /// answer is forced — then one draw over the chains alone. Nothing to
    /// choose from: no draw.
    fn chain(&mut self, _player: u8, count: usize, forced: bool) -> Option<usize> {
        if count == 0 {
            return forced.then_some(0);
        }
        if forced {
            return Some(self.below(count));
        }
        let r = self.below(count + 1);
        (r < count).then_some(r)
    }

    /// One draw over the flat list of every offered move: each summonable
    /// card (0), each rule Special Summon (1), each repositionable card
    /// (2), each monster Set (3), each Spell/Trap Set (4), each activation
    /// (5), then to the Battle Phase (6) and to the End Phase (7) when
    /// offered. Shuffling the hand (8) is never taken.
    fn idle(&mut self, menu: &IdleMenu) -> (u32, usize) {
        let mut flat: Vec<(u32, usize)> = Vec::new();
        for (kind, n) in [
            (0, menu.summonable),
            (1, menu.spsummonable),
            (2, menu.repositionable),
            (3, menu.msetable),
            (4, menu.ssetable),
            (5, menu.activatable),
        ] {
            flat.extend((0..n).map(|i| (kind, i)));
        }
        if menu.to_bp {
            flat.push((6, 0));
        }
        if menu.to_ep {
            flat.push((7, 0));
        }
        if flat.is_empty() {
            return (6, 0);
        }
        let r = self.below(flat.len());
        flat[r]
    }

    /// One draw over: each attack (1), each activation (0), then to Main
    /// Phase 2 (2) and to the End Phase (3) when offered.
    fn battle(&mut self, menu: &BattleMenu) -> (u32, usize) {
        let mut flat: Vec<(u32, usize)> = Vec::new();
        flat.extend((0..menu.attackable).map(|i| (1, i)));
        flat.extend((0..menu.activatable).map(|i| (0, i)));
        if menu.to_m2 {
            flat.push((2, 0));
        }
        if menu.to_ep {
            flat.push((3, 0));
        }
        if flat.is_empty() {
            return (3, 0);
        }
        let r = self.below(flat.len());
        flat[r]
    }

    /// One draw over the offered position bits.
    fn position(&mut self, _player: u8, positions: u8) -> u8 {
        let bits: Vec<u8> = [0x1u8, 0x2, 0x4, 0x8]
            .into_iter()
            .filter(|b| positions & b != 0)
            .collect();
        if bits.is_empty() {
            return 0x1;
        }
        bits[self.below(bits.len())]
    }

    /// One draw over `total + 1` when stopping is legal — the extra slot
    /// is "stop" — and over `total` when it is not.
    fn unselect(
        &mut self,
        _player: u8,
        total: usize,
        finishable: bool,
        cancelable: bool,
    ) -> Option<usize> {
        let may_stop = finishable || cancelable;
        if may_stop {
            let r = self.below(total + 1);
            (r < total).then_some(r)
        } else {
            Some(self.below(total))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Somebody won. **The engine does not stop on its own** — the
    /// reference emits `MSG_WIN` and keeps processing, and it is the
    /// host's job to notice and stop calling. A driver that does not watch
    /// for this runs a decided duel forever.
    Win { player: u8, reason: u8 },
    /// The processor queue emptied.
    Finished,
    /// The step budget ran out. Not an error — a duel with no win
    /// condition reachable runs forever, so every driver needs one.
    OutOfSteps,
    /// The same question was asked repeatedly with no progress, which
    /// means the policy keeps giving an illegal answer.
    Stuck,
}

/// Builds a duel and keeps it moving.
#[derive(Clone)]
pub struct Driver<P: Policy> {
    pub field: Field,
    pub policy: P,
    /// The question currently outstanding. **Not** simply the last
    /// message: a `Retry` does not repeat the question, so it has to be
    /// remembered across the retry.
    outstanding: Option<Message>,
    retries: u32,
    /// How much of `field.messages` has been scanned for a win.
    seen_messages: usize,
    /// The comparable record of this duel, if one is being kept.
    pub trace: Option<crate::trace::Trace>,
    /// Drop the message log after every step. A search never reads it —
    /// the driver answers each question from the step that asked it — so
    /// a solver-shaped run keeps the log from growing and from being
    /// cloned. Incompatible with tracing, which reads the log.
    pub discard_messages: bool,
    /// Calls to `process` so far — the unit `examples/bench.rs` times.
    pub steps: u64,
    /// Questions answered so far (retries included).
    pub questions: u64,
    /// Solver mode: answer the chance questions from the duel's own
    /// generator ([`Field::sample_chance`]) instead of the policy — a
    /// sampled rollout, and the faithful mode's trace to the line.
    pub sample_chance: bool,
}

/// A duel about to start: the reference configuration, both decks dealt
/// face-down into the Main Decks in list order, and `Startup` queued. What
/// [`Driver::with_decks_and_seed`] and [`crate::game::Game::new`] both
/// begin from.
pub fn new_duel(decks: [Vec<CardData>; 2], seed: [u64; 4]) -> Field {
    new_duel_with_lp(decks, seed, 8000)
}

/// [`new_duel`] with each player's starting life points: a small venue for
/// a solver's tests is a short game, and life points are the shortest
/// lever.
pub fn new_duel_with_lp(decks: [Vec<CardData>; 2], seed: [u64; 4], lp: i32) -> Field {
    let mut field = Field::with_flags_and_seed(lp, crate::duel::REFERENCE_CONFIGURATION, seed);
    for (player, deck) in decks.into_iter().enumerate() {
        let player = player as u8;
        for data in deck {
            // **A decklist carries its Extra Deck**, and the sorting
            // happens a layer down: `field::add_card` sends a card
            // whose printed type belongs in the Extra Deck there
            // whatever pile it was asked for. The oracle applies the
            // same rule to the same list (`tools/oracle/core.py`), so
            // both engines deal a Fusion Monster to the same place —
            // and a test below holds that, because a card dealt into
            // an opening hand on one engine and not the other is not
            // a comparison.
            let mut c = Card::with_data(data, player);
            c.current.controller = player;
            let id = field.new_card(c);
            field.add_card(player, id, location::DECK, 0, false);
            // `OCG_DuelNewCard` sets `current.position = info.pos` after the
            // add, and the harness passes `POS_FACEDOWN_DEFENSE` for every
            // card. A card that never leaves the deck or extra deck keeps
            // it, and `MSG_MOVE` reports it as the position it left from.
            field.cards[id].current.position = crate::board::position::FACEDOWN_DEFENSE;
            let fid = field.next_field_id_raw();
            field.cards[id].fieldid = fid;
            field.cards[id].fieldid_r = fid;
            // The card's script, if the pool has one — `initial_effect`.
            field.initialize_card(id);
        }
    }
    field.push_back(Kind::Startup);
    field
}

/// Which messages are questions. Everything else — `Hint`,
/// `CardSelected`, `ShuffleHand` — is an announcement, and several
/// arrive *after* the question they belong to.
pub(crate) fn is_question(m: &Message) -> bool {
    matches!(
        m,
        Message::SelectYesNo { .. }
            | Message::SelectCoin { .. }
            | Message::SelectRandom { .. }
            | Message::SelectDeckTop { .. }
            | Message::SelectEffectYesNo { .. }
            | Message::SelectOption { .. }
            | Message::SelectCard { .. }
            | Message::SelectUnselectCard { .. }
            | Message::SelectChain { .. }
            | Message::SelectPlace { .. }
            | Message::SelectPosition { .. }
            | Message::SelectIdleCmd { .. }
            | Message::SelectBattleCmd { .. }
            | Message::SelectTribute { .. }
            | Message::Sort { .. }
            | Message::AnnounceRace { .. }
    )
}

impl<P: Policy> Driver<P> {
    /// One card's printed data, as a differential run has to state it.
    ///
    /// The reference reads card data from its own database, so a
    /// comparison has to name a card the reference *knows* and give this
    /// engine the same numbers. Inventing a code here and hoping is how a
    /// harness diverges on line one for a reason that is not a bug.
    pub fn deck_card(
        code: u32,
        level: u32,
        attack: i32,
        defense: i32,
        attribute: u32,
        race: u64,
    ) -> CardData {
        CardData {
            code,
            // `TYPE_MONSTER | TYPE_NORMAL` — 17, the reference's value for
            // a vanilla.
            type_: crate::card::card_type::MONSTER | crate::card::card_type::NORMAL,
            level,
            attack,
            defense,
            attribute,
            race,
            ..Default::default()
        }
    }

    /// A duel with two decks of one named card.
    ///
    /// Vanilla on purpose: with no effects there is nothing to chain and
    /// nothing to resolve, so what this exercises is the *turn structure* —
    /// phases, priority, the battle loop — rather than any card. That is
    /// the layer to get running first, and the layer a differential run
    /// should agree on before any card is translated.
    pub fn with_deck(policy: P, data: CardData, deck_size: u32) -> Self {
        Self::with_deck_and_seed(policy, data, deck_size, Field::default_seed())
    }

    /// The same, with the random state named.
    ///
    /// A differential run has to set this: the two engines' *default*
    /// seeds differ, which is invisible until something actually draws —
    /// the first coin toss, in practice.
    pub fn with_deck_and_seed(policy: P, data: CardData, deck_size: u32, seed: [u64; 4]) -> Self {
        let deck: Vec<CardData> = (0..deck_size).map(|_| data.clone()).collect();
        Self::with_decks_and_seed(policy, [deck.clone(), deck], seed)
    }

    /// Two decks of named cards, in **decklist order**: each card is added
    /// at sequence 0, which is what `OCG_DuelNewCard` does, so the last
    /// card listed is the top of the deck on both engines. Adding at an
    /// increasing sequence instead owes a shuffle per card (sequence ≥ 2
    /// is "top, and shuffle") — harmless with forty identical cards, and
    /// a different duel with a mixed deck.
    pub fn with_decks_and_seed(policy: P, decks: [Vec<CardData>; 2], seed: [u64; 4]) -> Self {
        let field = new_duel(decks, seed);
        Self {
            field,
            policy,
            outstanding: None,
            retries: 0,
            seen_messages: 0,
            trace: None,
            discard_messages: false,
            sample_chance: false,
            steps: 0,
            questions: 0,
        }
    }

    /// The duel the tests use: forty copies of one 1700-attack vanilla.
    /// `5053103` is a code the reference's card data knows, so the same
    /// deck can be built on both sides.
    pub fn vanilla(policy: P, deck_size: u32) -> Self {
        Self::with_deck(policy, Self::vanilla_data(), deck_size)
    }

    /// The vanilla the harness deals, transcribed **whole** from
    /// `tools/oracle/carddata.py`:
    ///
    /// ```text
    /// OcgCard(5053103, _NORMAL, 4, 1700, 1000, ATTRIBUTE_EARTH, RACE_BEASTWARRIOR)
    /// ```
    ///
    /// The attribute and race were missing until Reinforcement of the
    /// Army — the first card to read a race — went looking for them. They
    /// had never mattered, which is exactly why they were wrong: the
    /// reference reads this card from its own database and always had the
    /// full line, so the two engines had been disagreeing silently about
    /// what the deck was made of.
    pub fn vanilla_data() -> CardData {
        Self::deck_card(
            5_053_103,
            4,
            1700,
            1000,
            crate::card::attribute::EARTH,
            crate::card::race::BEASTWARRIOR,
        )
    }

    /// Keep a comparable trace of this duel. Off by default: a duel that
    /// is not being compared should not pay for one.
    pub fn tracing(mut self) -> Self {
        self.trace = Some(crate::trace::Trace::default());
        self
    }

    /// A solver-shaped driver: no trace, and the message log dropped after
    /// every step.
    pub fn discarding_messages(mut self) -> Self {
        self.discard_messages = true;
        self
    }

    /// The trace so far, rendered.
    pub fn rendered_trace(&self) -> String {
        self.trace.as_ref().map(|t| t.render()).unwrap_or_default()
    }

    /// Run until somebody wins, the duel ends, or the budget runs out.
    pub fn run(&mut self, max_steps: u32) -> Stop {
        for _ in 0..max_steps {
            // What was answered this step, if anything — recorded after
            // the question's own messages, so the trace reads in order.
            let mut answered = None;
            self.steps += 1;
            match self.field.process() {
                Status::Continue => {
                    self.outstanding = None;
                    self.retries = 0;
                }
                Status::Awaiting => {
                    self.questions += 1;
                    if !self.answer() {
                        return Stop::Stuck;
                    }
                    answered = self.outstanding.clone();
                }
                Status::End => return Stop::Finished,
            }
            if let Some(t) = self.trace.as_mut() {
                t.absorb(&self.field.messages);
                if let Some(q) = &answered {
                    t.answered(q, &self.field.core.returns);
                }
            }
            if let Some(win) = self.check_win() {
                return win;
            }
            if self.discard_messages {
                // Everything this step wrote has been read: the question was
                // answered above, the win checked, the trace (if any) absorbed.
                self.field.messages.clear();
                self.seen_messages = 0;
            }
        }
        Stop::OutOfSteps
    }

    /// Scan the messages produced since the last check for a win.
    ///
    /// Scanning **only the new ones** matters: the message log is never
    /// trimmed, so re-scanning from the start makes every later step
    /// re-find the same win, and is quadratic besides.
    fn check_win(&mut self) -> Option<Stop> {
        let found = self.field.messages[self.seen_messages..]
            .iter()
            .find_map(|m| match m {
                Message::Win { player, reason } => Some(Stop::Win {
                    player: *player,
                    reason: *reason,
                }),
                _ => None,
            });
        self.seen_messages = self.field.messages.len();
        found
    }

    /// Find the outstanding question and answer it.
    ///
    /// Returns false when the policy has been asked the same thing too
    /// many times without the engine accepting it — which is a policy bug,
    /// not an engine one, and worth distinguishing from a hang.
    fn answer(&mut self) -> bool {
        static PROBE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *PROBE.get_or_init(|| std::env::var_os("PORT_PROBE").is_some()) {
            if let Some(q) = self.field.messages.last() {
                if let Some(line) = crate::trace::line_for(q) {
                    eprintln!("PROBE ask {line}");
                }
            }
        }
        // A `Retry` means the *previous* question stands. Anything else
        // means look for the most recent question in the log.
        let retried = matches!(self.field.messages.last(), Some(Message::Retry));
        if retried {
            self.retries += 1;
            if self.retries > 8 {
                return false;
            }
        } else {
            self.outstanding = self
                .field
                .messages
                .iter()
                .rev()
                .find(|m| is_question(m))
                .cloned();
            self.retries = 0;
        }
        let Some(question) = self.outstanding.clone() else {
            return false;
        };
        if self.sample_chance && self.field.sample_chance() {
            return true;
        }
        self.decide(&question);
        true
    }

    fn decide(&mut self, question: &Message) {
        let r = &mut self.field.core.returns;
        match question {
            Message::SelectYesNo {
                player,
                description,
            } => {
                let yes = self.policy.yes_no(*player, *description);
                r.set(i32::from(yes));
            }
            Message::SelectCoin { player, count } => {
                let mask = self.policy.coin(*player, *count);
                r.set(mask as i32);
            }
            Message::SelectRandom {
                player,
                count,
                cards,
            } => {
                let picks = self.policy.random_pick(*player, cards, *count);
                r.set_i32(0, 0);
                r.set_i32(1, picks.len() as i32);
                for (i, idx) in picks.iter().enumerate() {
                    r.set_i32(i + 2, *idx as i32);
                }
            }
            Message::SelectDeckTop { player, count } => {
                let deck = self.field.players[usize::from(*player)].main.clone();
                if let Some(order) = self.policy.deck_order(*player, *count, &deck) {
                    assert!(
                        self.field.set_deck_order(*player, &order),
                        "the policy's deck order is a permutation of the deck"
                    );
                }
                self.field.core.returns.set(0);
            }
            Message::SelectEffectYesNo {
                player,
                description,
                ..
            } => {
                let yes = self.policy.effect_yes_no(*player, *description);
                r.set(i32::from(yes));
            }
            Message::SelectOption { player, options } => {
                let n = options.len();
                let odd = self.field.infos.turn_id % 2 == 1;
                let pick = self.policy.option(*player, n, odd).min(n.saturating_sub(1));
                r.set(pick as i32);
            }
            Message::SelectCard {
                player,
                cards,
                min,
                max,
                ..
            } => {
                let chosen = self.policy.cards(*player, cards, *min, *max);
                // **The answer is a tagged encoding, not a list.** Slot 0
                // is a *type* saying how wide the indices are (0 = u32,
                // 1 = u16, 2 = u8, 3 = a bitmask from bit 32), or `-1` to
                // cancel. For type 0, slot 1 is the count and slots 2.. are
                // indices **into the offered list**, not card ids.
                //
                // Reading `SelectCard` as "write the chosen cards into
                // returns" produces something the engine rejects forever,
                // which is a retry loop rather than an error.
                let indices: Vec<usize> = chosen
                    .iter()
                    .filter_map(|id| cards.iter().position(|c| c == id))
                    .collect();
                self.field.core.return_cards.clear();
                self.field.core.returns.set_i32(0, 0);
                self.field.core.returns.set_i32(1, indices.len() as i32);
                for (i, idx) in indices.iter().enumerate() {
                    self.field.core.returns.set_i32(i + 2, *idx as i32);
                }
            }
            Message::SelectUnselectCard {
                player,
                finishable,
                cancelable,
                select,
                unselect,
                ..
            } => {
                let total = select.len() + unselect.len();
                let pick = self
                    .policy
                    .unselect(*player, total, *finishable, *cancelable);
                let r = &mut self.field.core.returns;
                match pick {
                    None => r.set_i32(0, -1),
                    Some(i) => {
                        r.set_i32(0, 1);
                        r.set_i32(1, i as i32);
                    }
                }
            }
            Message::SelectChain {
                player,
                chains,
                forced,
                ..
            } => {
                let pick = self.policy.chain(*player, chains.len(), *forced);
                r.set(pick.map_or(-1, |i| i as i32));
            }
            Message::SelectPlace { player, flag, .. } => {
                let (p, loc, seq) = self.policy.place(*player, *flag);
                // A question about the Spell & Trap row forbids every
                // monster seat, so the row is read off the mask rather
                // than assumed: activating a Spell asks for a Spell seat.
                let loc = if Self::first_free_seat(*flag, loc, p == *player).is_some() {
                    loc
                } else if loc == location::MZONE {
                    location::SZONE
                } else {
                    location::MZONE
                };
                let seq = Self::first_free_seat(*flag, loc, p == *player).unwrap_or(seq);
                r.set_i8(0, p as i8);
                r.set_i8(1, loc as i8);
                r.set_i8(2, seq);
                let _ = loc;
            }
            Message::SelectPosition {
                player, positions, ..
            } => {
                let p = self.policy.position(*player, *positions);
                r.set(i32::from(p));
            }
            Message::SelectIdleCmd {
                player,
                summonable,
                spsummonable,
                repositionable,
                msetable,
                ssetable,
                activatable,
                to_bp,
                to_ep,
                can_shuffle,
            } => {
                let menu = IdleMenu {
                    player: *player,
                    // Odd turns Set, even turns summon. The turn id is the
                    // only clock the policy can see, and it advances once
                    // per turn on both engines alike.
                    set_monster_this_turn: self.field.infos.turn_id % 2 == 1,
                    summonable: summonable.len(),
                    spsummonable: spsummonable.len(),
                    repositionable: repositionable.len(),
                    msetable: msetable.len(),
                    ssetable: ssetable.len(),
                    activatable: activatable.len(),
                    to_bp: *to_bp,
                    to_ep: *to_ep,
                    can_shuffle: *can_shuffle,
                };
                let (kind, index) = self.policy.idle(&menu);
                self.field
                    .core
                    .returns
                    .set(((index as u32) << 16 | kind) as i32);
            }
            Message::SelectBattleCmd {
                player,
                activatable,
                attackable,
                to_m2,
                to_ep,
            } => {
                let menu = BattleMenu {
                    player: *player,
                    activatable: activatable.len(),
                    attackable: attackable.len(),
                    to_m2: *to_m2,
                    to_ep: *to_ep,
                };
                let (kind, index) = self.policy.battle(&menu);
                self.field
                    .core
                    .returns
                    .set(((index as u32) << 16 | kind) as i32);
            }
            Message::SelectTribute {
                cards, min, max, ..
            } => {
                // **The tagged `SelectCard` encoding**, not the two-slot
                // form `SelectUnselectCard` uses: the reference decodes
                // this with `parse_response_cards` (`playerop.cpp`).
                //
                // And `min` counts **release value**, not cards — a
                // monster may be worth more than one tribute — so the
                // answer takes offers in order until their release values
                // reach `min`, never more than `max` cards.
                let (min, max) = (u32::from(*min), usize::from(*max));
                let mut picks: Vec<usize> = Vec::new();
                let mut value = 0u32;
                for (i, (_, release)) in cards.iter().enumerate() {
                    if value >= min || picks.len() >= max {
                        break;
                    }
                    picks.push(i);
                    value += (*release).max(1);
                }
                self.field.core.return_cards.clear();
                self.field.core.returns.set_i32(0, 0);
                self.field.core.returns.set_i32(1, picks.len() as i32);
                for (i, idx) in picks.iter().enumerate() {
                    self.field.core.returns.set_i32(i + 2, *idx as i32);
                }
            }
            Message::AnnounceRace {
                player,
                count,
                available,
            } => {
                let picked = self.policy.announce_race(*player, *count, *available);
                // A 64-bit mask, written with `set<uint64_t>` — the race
                // table outgrew 32 bits.
                self.field.core.returns.set_u64(0, picked);
            }
            Message::Sort { player, cards, .. } => {
                let n = cards.len();
                match self.policy.sort(*player, n) {
                    None => self.field.core.returns.set_i8(0, -1),
                    Some(order) => {
                        for (i, v) in order.iter().enumerate() {
                            self.field.core.returns.set_i8(i, *v);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The lowest seat the mask leaves available.
    ///
    /// `SelectPlace`'s flag marks the seats that are **not** usable, and
    /// the four rows are packed one byte each: own monsters, own
    /// spell/trap, opponent monsters, opponent spell/trap. So finding a
    /// free seat means picking the right byte and then the lowest clear
    /// bit in it.
    /// The lowest seat the mask leaves available, or `None` if there is
    /// none.
    ///
    /// The layout is `SelectPlace`'s response check, read backwards
    /// (`playerop.cpp`): a seat's bit is `1 << sequence`, shifted 8 more
    /// for the Spell/Trap row and 16 more for the **other** player's side,
    /// and a *set* bit means forbidden. The monster row has seven seats and
    /// the Spell/Trap row eight — `sequence > 7 - ismzone` — so scanning a
    /// flat eight would offer a monster seat that does not exist.
    pub(crate) fn first_free_seat(flag: u32, loc: u8, own_side: bool) -> Option<i8> {
        let mzone = loc != location::SZONE;
        let shift = u32::from(!mzone) * 8 + u32::from(!own_side) * 16;
        let seats = if mzone { 7 } else { 8 };
        (0..seats)
            .find(|s| flag & (1 << (s + shift)) == 0)
            .map(|s| s as i8)
    }
}

#[cfg(test)]
mod random_policy_tests {
    use super::*;

    /// **The generator is pinned**, on both sides: these are SplitMix64's
    /// first three outputs for seed 0, and `tools/differential.py`
    /// asserts the same three at start-up. A generator that drifted on
    /// one side would desynchronise every random game after the first
    /// draw, and the divergence would look like an engine bug.
    #[test]
    fn splitmix64_is_pinned() {
        let mut r = RandomPolicy::new(0);
        assert_eq!(r.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(r.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(r.next_u64(), 0x06C4_5D18_8009_454F);
    }

    /// Every answer is legal by construction, across many seeds.
    #[test]
    fn every_answer_is_legal() {
        for seed in 0..64u64 {
            let mut r = RandomPolicy::new(seed);
            let offered: Vec<CardId> = (10..17).collect();
            for (min, max) in [(0u8, 0u8), (1, 1), (1, 3), (2, 9), (0, 2), (7, 7)] {
                let picks = r.cards(0, &offered, min, max);
                let lo = usize::from(min).min(offered.len());
                let hi = usize::from(max).min(offered.len()).max(lo);
                assert!(
                    picks.len() >= lo && picks.len() <= hi,
                    "{min}..={max} gave {picks:?}"
                );
                let mut sorted = picks.clone();
                sorted.sort_unstable();
                sorted.dedup();
                assert_eq!(sorted, picks, "distinct, ascending");
                assert!(picks.iter().all(|c| offered.contains(c)));
            }
            for count in 0..4 {
                let free = r.chain(0, count, false);
                assert!(free.is_none_or(|i| i < count));
                let forced = r.chain(0, count, true);
                assert_eq!(forced.is_some(), count > 0 || true, "forced always answers");
                assert!(forced.is_none_or(|i| i < count.max(1)));
            }
            let menu = IdleMenu {
                player: 0,
                set_monster_this_turn: false,
                summonable: 2,
                spsummonable: 0,
                repositionable: 1,
                msetable: 1,
                ssetable: 3,
                activatable: 0,
                to_bp: true,
                to_ep: false,
                can_shuffle: true,
            };
            let (kind, index) = r.idle(&menu);
            let limit = [2, 0, 1, 1, 3, 0].get(kind as usize).copied();
            match kind {
                0..=5 => assert!(index < limit.unwrap(), "{kind}:{index}"),
                6 => assert_eq!(index, 0),
                _ => panic!("shuffling or the End Phase were not offered"),
            }
            let battle = BattleMenu {
                player: 0,
                activatable: 1,
                attackable: 2,
                to_m2: false,
                to_ep: true,
            };
            let (kind, index) = r.battle(&battle);
            match kind {
                1 => assert!(index < 2),
                0 => assert_eq!(index, 0),
                3 => assert_eq!(index, 0),
                _ => panic!("Main Phase 2 was not offered"),
            }
            let pos = r.position(0, 0x5);
            assert!(pos == 0x1 || pos == 0x4);
            assert!(r.unselect(0, 3, false, false).is_some_and(|i| i < 3));
            assert!(r.unselect(0, 3, true, false).is_none_or(|i| i < 3));
            let _ = r.option(0, 3, false);
        }
    }

    /// **Declining only when allowed.** Over many draws, `chain` declines
    /// sometimes when free and never when forced; `unselect` stops
    /// sometimes when it may and never when it may not.
    #[test]
    fn it_declines_only_when_it_may() {
        let mut r = RandomPolicy::new(7);
        let mut declined = 0;
        for _ in 0..200 {
            if r.chain(0, 2, false).is_none() {
                declined += 1;
            }
            assert!(r.chain(0, 2, true).is_some());
            assert!(r.unselect(0, 2, false, false).is_some());
        }
        assert!(declined > 20 && declined < 180, "{declined} of 200");
        let stopped = (0..200)
            .filter(|_| r.unselect(0, 2, false, true).is_none())
            .count();
        assert!(stopped > 20 && stopped < 180, "{stopped} of 200");
    }

    /// The same seed, the same choices — the property the mirror rests on.
    #[test]
    fn the_same_seed_makes_the_same_choices() {
        let offered: Vec<CardId> = (0..9).collect();
        let mut a = RandomPolicy::new(99);
        let mut b = RandomPolicy::new(99);
        for _ in 0..50 {
            assert_eq!(a.cards(0, &offered, 1, 4), b.cards(0, &offered, 1, 4));
            assert_eq!(a.chain(0, 3, false), b.chain(0, 3, false));
            assert_eq!(a.yes_no(0, 0), b.yes_no(0, 0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A placement that forbids every monster seat is answered with a
    /// Spell seat.** Activating a Spell asks exactly that, and answering a
    /// monster seat is refused forever.
    #[test]
    fn a_spell_row_placement_is_answered_in_the_spell_row() {
        let mut d = Driver::vanilla(PassPolicy, 40);
        // Every monster seat forbidden; Spell seats 0 and 1 taken.
        let flag = 0x7f | (0b11 << 8) | 0xe080_e080u32;
        d.field.messages.push(Message::SelectPlace {
            player: 0,
            flag,
            count: 1,
            disable_field: false,
        });
        d.decide(&Message::SelectPlace {
            player: 0,
            flag,
            count: 1,
            disable_field: false,
        });
        let r = &d.field.core.returns;
        assert_eq!(r.at_i8(1) as u8, location::SZONE, "the Spell row");
        assert_eq!(r.at_i8(2), 2, "the first free Spell seat");
    }

    /// **A duel plays to a conclusion.**
    ///
    /// Two vanilla decks and a policy that declines everything: nobody
    /// attacks, nobody summons, so the only way it can end is **deck-out**
    /// — both players draw until one cannot, and that is a real win
    /// condition reached by playing rather than by a unit test.
    #[test]
    fn a_duel_plays_to_a_deck_out() {
        let mut d = Driver::vanilla(PassPolicy, 40);
        let stop = d.run(500_000);
        match stop {
            Stop::Win { reason, .. } => {
                assert_eq!(reason, 2, "reason 2 is running out of cards");
                // 40 cards, 5 dealt, one drawn per turn from turn 2 —
                // so the deck runs out in the seventies.
                assert!(
                    (60..=90).contains(&d.field.infos.turn_id),
                    "ended on turn {}",
                    d.field.infos.turn_id
                );
            }
            other => panic!("expected a deck-out win, got {other:?}"),
        }
    }

    /// **The engine does not stop itself.** It emits the win and carries
    /// on; noticing is the host's job. Stepping past the win shows the
    /// machine still running.
    #[test]
    fn the_engine_keeps_going_after_a_win() {
        let mut d = Driver::vanilla(PassPolicy, 40);
        let stop = d.run(500_000);
        assert!(matches!(stop, Stop::Win { .. }));
        let turn = d.field.infos.turn_id;
        // The driver stopped, but the field is still perfectly steppable.
        for _ in 0..50 {
            if d.field.process() == Status::End {
                break;
            }
        }
        assert!(
            d.field.infos.turn_id >= turn,
            "the engine went on without complaint"
        );
    }

    /// **The first duel this port has ever run.**
    ///
    /// Two vanilla decks, a policy that declines everything, and a step
    /// budget. What it proves is not that the rules are right — nothing
    /// here checks a rule — but that the turn structure holds together:
    /// `Startup` deals, `Turn` cycles the phases, `IdleCommand` and
    /// `BattleCommand` ask and are answered, and nothing panics.
    #[test]
    fn a_duel_runs_without_panicking() {
        let mut d = Driver::vanilla(PassPolicy, 40);
        let stop = d.run(20_000);
        assert_ne!(stop, Stop::Stuck, "the policy kept giving illegal answers");
        assert!(
            d.field.infos.turn_id > 1,
            "at least one full turn, got turn {}",
            d.field.infos.turn_id
        );
    }

    /// **Both players get opening hands**, and the first turn does not
    /// draw — `DUEL_1ST_TURN_DRAW` is off in this configuration.
    #[test]
    fn the_opening_hands_are_dealt() {
        let mut d = Driver::vanilla(PassPolicy, 40);
        // Stop as soon as the first turn begins: running further would
        // reach turn 2, which *does* draw, and 6 cards would pass an
        // assertion meant for 5.
        for _ in 0..2_000 {
            if d.field.infos.turn_id >= 1 {
                break;
            }
            if d.field.process() == Status::End {
                break;
            }
        }
        assert_eq!(d.field.players[0].hand.len(), 5, "player 0 opening hand");
        assert_eq!(d.field.players[1].hand.len(), 5, "player 1 opening hand");
        assert_eq!(d.field.players[0].main.len(), 35, "and taken from the deck");
    }

    /// **The turn passes between players.** A driver that declines
    /// everything should still cycle turns, and the turn player should
    /// alternate.
    #[test]
    fn turns_alternate() {
        let mut d = Driver::vanilla(PassPolicy, 40);
        // Sampled **one step at a time**: a `run(n)` spans several turns,
        // so consecutive samples of it can land on the same player and the
        // test proves nothing.
        let mut seen: Vec<(i16, u8)> = Vec::new();
        for _ in 0..40_000 {
            let before = d.field.infos.turn_id;
            match d.field.process() {
                Status::Continue => {
                    d.outstanding = None;
                    d.retries = 0;
                }
                Status::Awaiting => {
                    if !d.answer() {
                        break;
                    }
                }
                Status::End => break,
            }
            if d.field.infos.turn_id != before {
                seen.push((d.field.infos.turn_id, d.field.infos.turn_player));
                if seen.len() >= 4 {
                    break;
                }
            }
        }
        assert!(seen.len() >= 3, "saw {seen:?}");
        assert_eq!(
            seen.iter().map(|&(_, p)| p).collect::<Vec<_>>()[..3],
            [0, 1, 0],
            "the turn alternates"
        );
        assert_eq!(
            seen.iter().map(|&(t, _)| t).collect::<Vec<_>>()[..3],
            [1, 2, 3],
            "and the turn id counts up"
        );
    }

    /// **A duel reaches the Battle Phase** when the policy asks for it,
    /// which is the path `BattleCommand` sits on.
    #[test]
    fn the_battle_phase_is_reachable() {
        /// Go to the Battle Phase from Main 1, then leave it.
        struct ToBattle {
            entered: bool,
        }
        impl Policy for ToBattle {
            fn idle(&mut self, menu: &IdleMenu) -> (u32, usize) {
                if menu.to_bp && !self.entered {
                    self.entered = true;
                    return (6, 0);
                }
                (7, 0)
            }
        }
        let mut d = Driver::vanilla(ToBattle { entered: false }, 40);
        let mut reached = false;
        for _ in 0..40 {
            d.run(2_000);
            if d.field.infos.phase == crate::duel::phases::BATTLE_STEP
                || d.field.infos.phase == crate::duel::phases::BATTLE_START
            {
                reached = true;
                break;
            }
            if d.policy.entered {
                reached = true;
                break;
            }
        }
        assert!(reached, "never reached the Battle Phase");
    }

    /// **`Retry` is survivable.** A policy that gives an illegal answer
    /// is asked again rather than hanging the driver, and the driver gives
    /// up rather than looping forever.
    #[test]
    fn an_illegal_answer_is_retried_then_abandoned() {
        /// Always answers an idle command that does not exist.
        struct Nonsense;
        impl Policy for Nonsense {
            fn idle(&mut self, _menu: &IdleMenu) -> (u32, usize) {
                (9, 0) // kind 9 does not exist
            }
        }
        let mut d = Driver::vanilla(Nonsense, 40);
        let stop = d.run(20_000);
        assert_eq!(
            stop,
            Stop::Stuck,
            "an always-illegal policy is reported, not hung"
        );
    }

    /// The seat search reads the mask the way `SelectPlace` writes it:
    /// a **set** bit means unavailable, and the rows are one byte each.
    #[test]
    fn the_seat_search_reads_the_mask_as_unavailable() {
        // Monster row: seats 0 and 1 taken.
        assert_eq!(
            Driver::<PassPolicy>::first_free_seat(0b11, location::MZONE, true),
            Some(2)
        );
        // Spell row lives in the second byte.
        assert_eq!(
            Driver::<PassPolicy>::first_free_seat(0b111 << 8, location::SZONE, true),
            Some(3)
        );
        // Nothing free at all.
        assert_eq!(
            Driver::<PassPolicy>::first_free_seat(0xff, location::MZONE, true),
            None
        );
    }

    /// **The other player's zones are 16 bits up.** Reading the low half
    /// for them finds a free seat that belongs to somebody else, and the
    /// answer is refused — which looks like a hang rather than a mistake.
    #[test]
    fn the_seat_search_looks_up_the_mask_for_the_other_side() {
        // Own side full, the other side's first two seats taken. Read as
        // "own", this mask says there is nothing free at all.
        let flag = 0x7f | (0b11 << 16);
        assert_eq!(
            Driver::<PassPolicy>::first_free_seat(flag, location::MZONE, true),
            None
        );
        assert_eq!(
            Driver::<PassPolicy>::first_free_seat(flag, location::MZONE, false),
            Some(2)
        );
        // And the other side's Spell row is 8 further up again.
        assert_eq!(
            Driver::<PassPolicy>::first_free_seat(0b1 << 24, location::SZONE, false),
            Some(1)
        );
    }

    /// **The monster row has seven seats, the Spell/Trap row eight.** The
    /// reference's own check is `sequence > 7 - ismzone`, so a scan of a
    /// flat eight offers seat 7 of a row that does not have one.
    #[test]
    fn the_monster_row_stops_one_seat_short_of_the_spell_row() {
        assert_eq!(
            Driver::<PassPolicy>::first_free_seat(0x7f, location::MZONE, true),
            None,
            "seven monster seats: 0x7f is full"
        );
        assert_eq!(
            Driver::<PassPolicy>::first_free_seat(0x7f << 8, location::SZONE, true),
            Some(7),
            "eight spell seats: the eighth is still free"
        );
    }

    /// **A player places on their own side.** The default that named
    /// player 0 was not rejected as nonsense — it was read as "put it in
    /// player 0's field", forbidden for player 1's summon, and retried
    /// until the driver gave up.
    #[test]
    fn the_default_placement_is_the_asked_players_own_side() {
        let mut policy = PassPolicy;
        for player in 0..2u8 {
            let (named, loc, _) = policy.place(player, 0);
            assert_eq!(named, player, "player {player} places on their own side");
            assert_eq!(loc, location::MZONE);
        }
    }
}

#[cfg(test)]
mod play_policy_tests {
    use super::*;

    fn menu(activatable: usize, ssetable: usize, summonable: usize) -> IdleMenu {
        IdleMenu {
            player: 0,
            set_monster_this_turn: false,
            summonable,
            spsummonable: 0,
            repositionable: 0,
            msetable: 0,
            ssetable,
            activatable,
            to_bp: true,
            to_ep: true,
            can_shuffle: false,
        }
    }

    /// **The vanilla the harness deals carries the reference's whole
    /// printed line**, not just the numbers a test happened to need.
    ///
    /// Pinned because a wrong value here is invisible: the two engines
    /// stay internally consistent with whatever they were told, and the
    /// disagreement only surfaces when some card finally reads the field —
    /// which for `race` was the twentieth card in the pool.
    #[test]
    fn the_harness_vanilla_matches_the_reference_database() {
        let v = Driver::<PassPolicy>::vanilla_data();
        assert_eq!(v.code, 5_053_103);
        assert_eq!(
            v.type_,
            crate::card::card_type::MONSTER | crate::card::card_type::NORMAL
        );
        assert_eq!((v.level, v.attack, v.defense), (4, 1700, 1000));
        assert_eq!(v.attribute, crate::card::attribute::EARTH);
        assert_eq!(v.race, crate::card::race::BEASTWARRIOR);
    }

    /// **A decklist is sorted into two piles by printed type.** A Fusion
    /// Monster goes to the Extra Deck and everything else to the main
    /// one, which is the rule the harness's oracle applies to the same
    /// list — a card dealt into an opening hand on one engine and not the
    /// other is not a comparison.
    #[test]
    fn a_fusion_monster_is_dealt_to_the_extra_deck() {
        use crate::card::card_type as ct;
        let vanilla = Driver::<PassPolicy>::vanilla_data();
        let fusion = CardData {
            code: 87_751_584,
            type_: ct::MONSTER | ct::FUSION | ct::EFFECT,
            level: 8,
            ..Default::default()
        };
        let deck = vec![vanilla.clone(), fusion.clone(), vanilla, fusion];
        let d = Driver::with_decks_and_seed(PassPolicy, [deck.clone(), deck], [1, 2, 3, 4]);
        for p in 0..2usize {
            assert_eq!(
                d.field.players[p].extra.len(),
                2,
                "both Fusion Monsters, player {p}"
            );
            assert_eq!(d.field.players[p].main.len(), 2, "and both vanillas");
            for &c in &d.field.players[p].extra {
                assert!(
                    d.field.cards[c].data.is_type(ct::FUSION),
                    "only fusions in the Extra Deck"
                );
            }
        }
    }

    /// **A rule summon outranks every once-a-turn move and is outranked
    /// by activation.** It costs the turn nothing, so taking it only as a
    /// last resort would mean almost never taking it — and every card
    /// with a printed summon procedure would go unexercised.
    #[test]
    fn a_rule_summon_comes_second() {
        let mut p = PlayPolicy;
        let mut m = menu(0, 1, 1);
        m.msetable = 1;
        m.spsummonable = 1;
        assert_eq!(p.idle(&m), (1, 0), "above Set and summon");
        m.activatable = 1;
        assert_eq!(p.idle(&m), (5, 0), "but below activation");
    }

    /// **A monster is Set on one turn and summoned on the next.** The two
    /// are the same once-a-turn move, so a policy that preferred either
    /// always would never do the other — and never Setting a monster
    /// means never producing a face-down, so no flip effect in the pool
    /// would ever be reached.
    #[test]
    fn monsters_alternate_between_set_and_summon() {
        let mut p = PlayPolicy;
        let mut set_turn = menu(0, 0, 1);
        set_turn.msetable = 1;
        set_turn.set_monster_this_turn = true;
        assert_eq!(p.idle(&set_turn), (3, 0), "a Set turn Sets");
        let mut summon_turn = set_turn.clone();
        summon_turn.set_monster_this_turn = false;
        assert_eq!(p.idle(&summon_turn), (0, 0), "and the next summons");
    }

    /// **Either is taken when only one is on offer**, whichever turn it
    /// is — the alternation chooses between them, it does not forbid
    /// one.
    #[test]
    fn whichever_is_available_is_taken() {
        let mut p = PlayPolicy;
        let mut only_set = menu(0, 0, 0);
        only_set.msetable = 1;
        only_set.set_monster_this_turn = false;
        assert_eq!(
            p.idle(&only_set),
            (3, 0),
            "a summon turn still Sets if it must"
        );
        let mut only_summon = menu(0, 0, 1);
        only_summon.set_monster_this_turn = true;
        assert_eq!(
            p.idle(&only_summon),
            (0, 0),
            "and a Set turn still summons if it must"
        );
    }

    /// **Activating and Setting a Spell still come first.** The monster
    /// choice is the last of the four, on either kind of turn.
    #[test]
    fn the_spell_moves_still_come_first() {
        let mut p = PlayPolicy;
        let mut everything = menu(1, 1, 1);
        everything.msetable = 1;
        everything.set_monster_this_turn = true;
        assert_eq!(p.idle(&everything), (5, 0), "activate first");
        everything.activatable = 0;
        assert_eq!(p.idle(&everything), (4, 0), "then the Spell/Trap Set");
        everything.ssetable = 0;
        assert_eq!(p.idle(&everything), (3, 0), "then the monster");
    }

    /// **Activate, then Set, then summon** — the order the harness mirrors,
    /// so a Trap in hand is Set before the monster that would trigger it.
    #[test]
    fn the_main_phase_order_is_activate_set_summon() {
        let mut p = PlayPolicy;
        assert_eq!(p.idle(&menu(1, 1, 1)), (5, 0), "activate first");
        assert_eq!(p.idle(&menu(0, 1, 1)), (4, 0), "then Set");
        assert_eq!(p.idle(&menu(0, 0, 1)), (0, 0), "then summon");
        assert_eq!(p.idle(&menu(0, 0, 0)), (6, 0), "then leave");
    }

    /// **Both kinds of yes/no get a yes from the playing policy**, and
    /// neither from the passing one.
    ///
    /// They remain separate trait methods with opposite *defaults*, and
    /// this policy overrides both deliberately. It used to decline the
    /// bare one, on the reasoning that refusing is always legal — true,
    /// and the reason the default stands, but it made every "you may also
    /// do X" clause in the pool invisible to the harness. Dust Tornado's
    /// Set-from-hand half is reached only through this answer.
    #[test]
    fn the_playing_policy_accepts_both_kinds_of_yes_no() {
        let mut p = PlayPolicy;
        assert!(p.effect_yes_no(0, 0), "take the optional trigger");
        assert!(p.yes_no(0, 0), "and the bare yes/no");
        assert!(
            !PassPolicy.effect_yes_no(0, 0),
            "the pass policy declines both"
        );
        assert!(!PassPolicy.yes_no(0, 0));
    }

    /// **The two questions route to different hooks.** `MSG_SELECT_EFFECTYN`
    /// must reach `effect_yes_no`; sending it to `yes_no` would decline
    /// every optional trigger while looking correct.
    #[test]
    fn the_two_yes_no_questions_route_apart() {
        /// Says yes only to the effect question.
        #[derive(Default)]
        struct Split;
        impl Policy for Split {
            fn effect_yes_no(&mut self, _player: u8, _description: u64) -> bool {
                true
            }
        }
        let mut d = Driver::with_deck(Split, Driver::<Split>::deck_card(1, 4, 0, 0, 0, 0), 5);
        d.decide(&crate::field::Message::SelectEffectYesNo {
            player: 0,
            code: 7,
            controller: 0,
            location: 4,
            sequence: 0,
            position: 1,
            description: 0,
        });
        assert_eq!(
            d.field.core.returns.at_i32(0),
            1,
            "the effect hook answered"
        );
        d.decide(&crate::field::Message::SelectYesNo {
            player: 0,
            description: 0,
        });
        assert_eq!(d.field.core.returns.at_i32(0), 0, "the plain hook answered");
    }

    /// **The first thing offered in a response window is activated**;
    /// with nothing offered the window is declined.
    #[test]
    fn a_response_window_activates_the_first_offer() {
        let mut p = PlayPolicy;
        assert_eq!(p.chain(0, 2, false), Some(0));
        assert_eq!(p.chain(0, 1, true), Some(0));
        assert_eq!(p.chain(0, 0, false), None);
        assert_eq!(
            PassPolicy.chain(0, 2, false),
            None,
            "the pass policy still declines"
        );
    }
}

#[cfg(test)]
mod answer_encoding_tests {
    use super::*;
    use crate::field::Message;

    fn driver() -> Driver<PassPolicy> {
        Driver::with_deck(
            PassPolicy,
            Driver::<PassPolicy>::deck_card(1, 4, 0, 0, 0, 0),
            5,
        )
    }

    /// **A tribute answer uses the tagged card encoding.**
    /// `SelectTribute` is decoded by `parse_response_cards` in the
    /// reference (`playerop.cpp`), the same decoder `SelectCard` uses —
    /// *not* the two-slot form of `SelectUnselectCard`. Answering it the
    /// other way reads as a width tag of 1 and a count of 0, an **empty**
    /// selection, which the engine refuses forever: a retry loop rather
    /// than an error, and invisible until some card tribute summons.
    #[test]
    fn a_tribute_answer_uses_the_tagged_card_encoding() {
        let mut d = driver();
        d.decide(&Message::SelectTribute {
            player: 0,
            cancelable: true,
            min: 2,
            max: 2,
            cards: vec![(7, 1), (8, 1), (9, 1)],
        });
        let r = &d.field.core.returns;
        assert_eq!(r.at_i32(0), 0, "a width tag, not the unselect kind");
        assert_eq!(r.at_i32(1), 2, "two cards chosen");
        assert_eq!((r.at_i32(2), r.at_i32(3)), (0, 1), "the first two offers");
    }

    /// **`min` counts release value, not cards.** One monster worth two
    /// tributes satisfies a `min` of two on its own, and taking a second
    /// would be both wasteful and, at `max` of one, illegal.
    #[test]
    fn a_tribute_answer_counts_release_value() {
        let mut d = driver();
        d.decide(&Message::SelectTribute {
            player: 0,
            cancelable: true,
            min: 2,
            max: 2,
            cards: vec![(7, 2), (8, 1)],
        });
        let r = &d.field.core.returns;
        assert_eq!(r.at_i32(1), 1, "one card is worth both tributes");
        assert_eq!(r.at_i32(2), 0);
    }

    /// **A card that declares no release value still counts as one.**
    /// Zero would otherwise make the loop take every offer and still not
    /// reach `min`.
    #[test]
    fn a_zero_release_value_still_counts_as_one() {
        let mut d = driver();
        d.decide(&Message::SelectTribute {
            player: 0,
            cancelable: true,
            min: 1,
            max: 1,
            cards: vec![(7, 0), (8, 0)],
        });
        let r = &d.field.core.returns;
        assert_eq!(r.at_i32(1), 1);
    }

    /// **An incremental selection finishes as soon as finishing is
    /// legal.** An index into `SelectUnselectCard` *toggles*: a card
    /// already chosen comes back in the `unselect` list, so answering `0`
    /// forever picks and un-picks the same card.
    #[test]
    fn an_incremental_selection_finishes_when_it_may() {
        let mut d = driver();
        d.decide(&Message::SelectUnselectCard {
            player: 0,
            finishable: false,
            cancelable: false,
            min: 1,
            max: 2,
            select: vec![7, 8],
            unselect: vec![],
        });
        assert_eq!(d.field.core.returns.at_i32(0), 1, "the only legal kind");
        assert_eq!(d.field.core.returns.at_i32(1), 0, "the first on offer");
        d.decide(&Message::SelectUnselectCard {
            player: 0,
            finishable: true,
            cancelable: false,
            min: 1,
            max: 2,
            select: vec![8],
            unselect: vec![7],
        });
        assert_eq!(
            d.field.core.returns.at_i32(0),
            -1,
            "done choosing, rather than toggling card 7 back out"
        );
    }
}

#[cfg(test)]
mod clone_tests {
    use super::*;
    use crate::cards::{card_data, POOL};

    /// A pool deck for game `game`, as `examples/bench.rs` deals one.
    fn pool_deck(game: u64) -> Vec<CardData> {
        let mut rng = RandomPolicy::new(1000 + game);
        let mut codes = POOL.to_vec();
        for i in (1..codes.len()).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            codes.swap(i, j);
        }
        codes.truncate(40);
        codes
            .iter()
            .map(|&c| card_data(c).expect("pool data"))
            .collect()
    }

    /// **A clone has the same future as its original.** The property a
    /// solver needs from `Field: Clone`: copy the duel at any step — a
    /// question pending, a chain half-solved, an operation suspended with
    /// its continuation parked — and the copy, given the same answers,
    /// plays out identically to the end. Checked at several depths of
    /// several random pool games, comparing the full rendered trace and
    /// the stop reason.
    #[test]
    fn a_clone_has_the_same_future() {
        for game in 0..4u64 {
            let deck = pool_deck(game);
            for &depth in &[300u32, 2_000, 6_000, 12_000] {
                let mut a = Driver::with_decks_and_seed(
                    RandomPolicy::new(77 + game),
                    [deck.clone(), deck.clone()],
                    [1, 2, 3, 4],
                )
                .tracing();
                if a.run(depth) != Stop::OutOfSteps {
                    continue; // the game ended before this depth
                }
                let mut b = a.clone();
                let sa = a.run(500_000);
                let sb = b.run(500_000);
                assert_eq!(sa, sb, "game {game}, cloned at step {depth}: stop");
                assert_eq!(
                    a.rendered_trace(),
                    b.rendered_trace(),
                    "game {game}, cloned at step {depth}: the copy's future"
                );
                assert!(
                    a.rendered_trace().lines().count() > 100,
                    "and the game went on after the clone"
                );
            }
        }
    }

    /// The copy is independent: driving it does not move the original.
    #[test]
    fn a_clone_is_independent() {
        let deck = pool_deck(9);
        let mut a =
            Driver::with_decks_and_seed(RandomPolicy::new(5), [deck.clone(), deck], [1, 2, 3, 4]);
        assert_eq!(a.run(1_000), Stop::OutOfSteps);
        let before = a.field.messages.len();
        let mut b = a.clone();
        b.run(3_000);
        assert_eq!(a.field.messages.len(), before, "the original did not move");
        assert!(b.field.messages.len() > before, "the copy did");
    }
}

#[cfg(test)]
mod chance_tests {
    use super::*;
    use crate::cards::{card_data, POOL};

    fn pool_deck(game: u64) -> Vec<CardData> {
        let mut rng = RandomPolicy::new(3000 + game);
        let mut codes = POOL.to_vec();
        for i in (1..codes.len()).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            codes.swap(i, j);
        }
        codes.truncate(40);
        codes
            .iter()
            .map(|&c| card_data(c).expect("pool data"))
            .collect()
    }

    /// The trace without the solver mode's chance questions and answers,
    /// which the faithful mode never asks.
    fn without_chance_lines(trace: &str) -> String {
        trace
            .lines()
            .filter(|l| {
                !(l.starts_with("ask coin ")
                    || l.starts_with("ask random ")
                    || l.starts_with("ask decktop ")
                    || l.starts_with("ans coin ")
                    || l.starts_with("ans random ")
                    || l.starts_with("ans decktop "))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// **Solver mode, answered from the generator, is the faithful mode.**
    /// The property the chance questions are built on: asking the host and
    /// being told what the generator would have rolled leaves a duel
    /// exactly where not asking leaves it — the same messages, in the same
    /// order, to the end. Several random pool games, the same seeds and
    /// policy on both sides, compared line for line once the questions
    /// themselves are dropped from the solver side's trace.
    #[test]
    fn answered_from_the_generator_the_solver_mode_replays_the_faithful_one() {
        let mut asked = (0u32, 0u32, 0u32);
        for game in 0..8u64 {
            let deck = pool_deck(game);
            let mut plain = Driver::with_decks_and_seed(
                RandomPolicy::new(91 + game),
                [deck.clone(), deck.clone()],
                [5, 6, 7, game],
            )
            .tracing();
            let mut solver = Driver::with_decks_and_seed(
                RandomPolicy::new(91 + game),
                [deck.clone(), deck],
                [5, 6, 7, game],
            )
            .tracing();
            solver.field.set_chance_mode(true);
            solver.sample_chance = true;
            let sp = plain.run(200_000);
            let ss = solver.run(200_000);
            assert_eq!(sp, ss, "game {game}: stop");
            let solver_trace = solver.rendered_trace();
            for l in solver_trace.lines() {
                if l.starts_with("ask coin ") {
                    asked.0 += 1;
                } else if l.starts_with("ask random ") {
                    asked.1 += 1;
                } else if l.starts_with("ask decktop ") {
                    asked.2 += 1;
                }
            }
            assert_eq!(
                plain.rendered_trace(),
                without_chance_lines(&solver_trace),
                "game {game}: the solver mode's trace"
            );
            assert!(
                plain.rendered_trace().lines().count() > 100,
                "game {game} went somewhere"
            );
            assert_eq!(
                plain.field.rng, solver.field.rng,
                "game {game}: the generators end together"
            );
        }
        assert!(asked.2 > 8, "every draw asked for the deck top: {asked:?}");
    }

    /// A policy's deck order is imposed before the draw reads the deck:
    /// the top card is the one it named.
    #[test]
    fn a_policy_deck_order_is_what_the_draw_takes() {
        struct BottomToTop;
        impl Policy for BottomToTop {
            fn deck_order(&mut self, _p: u8, _n: u32, deck: &[CardId]) -> Option<Vec<CardId>> {
                // The pile is bottom-first: the bottom card goes to the top.
                let mut order = deck.to_vec();
                order.rotate_left(1);
                Some(order)
            }
        }
        let deck = pool_deck(3);
        let mut d = Driver::with_decks_and_seed(BottomToTop, [deck.clone(), deck], [1, 2, 3, 4]);
        d.field.set_chance_mode(true);
        // Up to the first deck-top question, answering everything before it.
        let mut asked = None;
        for _ in 0..10_000 {
            if matches!(d.field.process(), crate::processor::Status::Awaiting) {
                if matches!(d.field.messages.last(), Some(Message::SelectDeckTop { .. })) {
                    asked = d.field.messages.last().cloned();
                    break;
                }
                assert!(d.answer(), "answered");
            }
        }
        let Some(Message::SelectDeckTop { player, .. }) = asked else {
            panic!("no deck-top question: {asked:?}");
        };
        let bottom = d.field.players[usize::from(player)].main[0];
        let code_ = d.field.cards[bottom].data.code;
        assert!(d.answer());
        assert_eq!(
            *d.field.players[usize::from(player)].main.last().unwrap(),
            bottom,
            "the old bottom card is now on top"
        );
        // And the draw takes it.
        let drawn = loop {
            let found = d.field.messages.iter().find_map(|m| match m {
                Message::Draw { player: p, codes } if *p == player => Some(codes.clone()),
                _ => None,
            });
            if let Some(codes) = found {
                break codes;
            }
            assert!(
                !matches!(d.field.process(), crate::processor::Status::Awaiting),
                "nothing else is asked before the draw"
            );
        };
        assert_eq!(drawn[0], code_, "the card the policy put on top was drawn");
    }
}
