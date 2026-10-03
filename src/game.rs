//! The search-facing layer of the port: a duel as a game tree.
//!
//! [`Game`] wraps a [`Field`] in solver mode and presents the duel the way
//! a game-tree search wants it: a node has an [`Actor`] — a player, chance,
//! or nobody because the game is over — a player node offers
//! [`Game::legal_actions`], a chance node enumerates
//! [`Game::chance_outcomes`] with their probabilities, and
//! [`Game::apply`] takes one [`Action`] to the next node. A node is one of
//! the engine's questions; the field between nodes is run to the next
//! question by [`Game::apply`], so a solver never sees the processor.
//!
//! ## Sequential picks, in canonical order
//!
//! "Choose *k* of *n*" is not one action with `C(n, k)` values: it is up
//! to `k` decisions of at most `n` actions each. A [`Action::Pick`] adds
//! the card at that index to the selection, and every later pick must be
//! **after** the last in the offered list — so each subset is reached by
//! exactly one path (its ascending index order, the order the driver has
//! answered with over four million games) — and [`Action::Finish`] closes
//! the selection once the minimum is met. Reaching the maximum closes it
//! without asking. The same shape serves the tribute question (the
//! minimum counts release value, the maximum counts cards), the Type
//! declaration (bits instead of cards), the zone question when it wants
//! several zones, and the sort questions (the next item, until the order
//! is complete). `SelectUnselectCard` already has this shape in the
//! reference and is passed through: one pick or a finish per question.
//!
//! The partial selection lives in the `Game`, never in the field: the
//! engine is answered once, when the selection is complete, exactly as
//! the driver answers it. Cloning a `Game` mid-selection clones the
//! partial picks with it.
//!
//! ## Chance
//!
//! The three chance questions of solver mode (see `docs/processor-loop.md`,
//! "Solver mode: chance as a question") become chance nodes. A coin toss
//! of *n* coins has `2^n` outcomes at `2^-n` each. A random selection of
//! *k* from *n* has every *k*-subset at `1 / C(n, k)` — the reference's
//! roll-until-distinct loop is symmetric in the group, so its subsets are
//! uniform. A look at the deck's top *k* cards is *k* chance nodes in a
//! row, each choosing the next card from the top uniformly among the
//! cards not yet settled — the deck's order is the hidden variable a
//! shuffle leaves, nothing observes it until the top is looked at, and
//! every placement into a deck in this pool shuffles (`check_constants.py`
//! pins that), so uniform is the right belief. The outcomes are listed by
//! ascending card id, never in the deck's actual order: the list must not
//! leak what it hides.
//!
//! ## Cycles
//!
//! The reference lets a player back out of a menu choice: attack, then
//! cancel the target selection; summon, then cancel the tribute
//! selection; activate, then cancel the target. Each returns to the menu
//! with nothing changed — a cycle in the game graph, harmless to random
//! play and fatal to a solver expanding a tree. Such a `Cancel` is a
//! no-op the menu already offered (choose something else there), so it
//! is not offered here: a `Cancel` is listed only when something has
//! been announced since the last menu was answered — a move, life
//! points, a chain link — because then it leads somewhere new (an
//! optional selection inside a resolution, say).
//!
//! A select/unselect question (`SelectUnselectCard`: a cost or target
//! chosen a card at a time, with the chosen ones offered back for
//! unselection) has the same shape one level down: unselecting returns
//! to a node the selects already reach — the selectable set is a
//! function of the selected set, so every final selection is reachable
//! by selecting its members in order — and a deterministic solver that
//! prefers the unselect at one node and the select at the other plays
//! the pair forever, which is how the first Goat game by search on the
//! port stalled (Chaos Sorcerer's two-card banish, select, unselect,
//! select…). So an unselect is listed only when nothing else is: no
//! card selectable and the selection not finishable.
//! `Config::prune_cycles` turns both prunings off, which the replay test
//! needs, since the random policy does cancel and unselect.
//!
//! ## Forced nodes
//!
//! Most of the engine's questions have one legal answer — every "any
//! response?" window with nothing to respond with, every single-option
//! choice. With `Config::skip_forced` (the default) those are answered on
//! the way and a solver sees only nodes with a choice. The trace still
//! records them, and `driver::RandomPolicy` still draws for some of them,
//! which is why the equivalence test below runs without the skip.

use crate::board::location;
use crate::card::CardData;
use crate::driver::{is_question, new_duel_with_lp};
use crate::event::CardId;
use crate::field::{Field, Message};
use crate::host_question::ChanceSample;
use crate::observation::{observe, Knowledge, Observation, PartialView};
use crate::processor::Status;
use crate::trace::Trace;

/// Who acts at a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Actor {
    Player(u8),
    Chance,
    /// The game is over: [`Game::result`] has the outcome.
    Terminal,
}

/// One decision at a node. Which variants are legal depends on the
/// question: [`Game::legal_actions`] and [`Game::chance_outcomes`] say.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    /// `SelectYesNo` / `SelectEffectYesNo`.
    YesNo(bool),
    /// `SelectOption`: the option at this index.
    ChooseOption(usize),
    /// Add the item at this index to the selection (`SelectCard`,
    /// `SelectTribute`, `SelectUnselectCard`, `Sort`).
    Pick(usize),
    /// Close the selection as it stands (`SelectCard` / `SelectTribute`
    /// once the minimum is met; `SelectUnselectCard` when finishable;
    /// `Sort` of a chain: keep the order).
    Finish,
    /// Cancel a cancelable selection.
    Cancel,
    /// `SelectChain`: activate the offer at this index.
    Chain(usize),
    /// `SelectChain`: no response.
    Decline,
    /// `SelectPlace`: a zone. `location` is `MZONE` or `SZONE`.
    Place {
        player: u8,
        location: u8,
        sequence: u8,
    },
    /// `SelectPosition`: one position bit.
    Position(u8),
    /// The Main Phase menu, by list and index.
    Summon(usize),
    SpecialSummon(usize),
    Reposition(usize),
    SetMonster(usize),
    SetSpellTrap(usize),
    /// An activation, from the Main Phase or the Battle Phase menu.
    Activate(usize),
    /// The Battle Phase menu: attack with the card at this index.
    Attack(usize),
    ToBattlePhase,
    ToMainPhase2,
    ToEndPhase,
    /// `AnnounceRace`: declare the Type at this bit.
    DeclareType(u8),
    /// Chance: the coin faces, bit `i` set for heads on coin `i`.
    Coin(u32),
    /// Chance: the random selection's picks, ascending indices into the
    /// offered group.
    RandomPick(Vec<usize>),
    /// Chance: the next card from the top of the deck being looked at.
    DeckTop(CardId),
}

/// How a game ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// `MSG_WIN` for a player, with the reference's reason code.
    Win { player: u8, reason: u8 },
    /// `MSG_WIN` for nobody.
    Draw { reason: u8 },
    /// The processor queue emptied without a win.
    Finished,
    /// One `apply` ran past `Config::max_steps_per_apply` processor steps
    /// without reaching a question: an engine loop, reported as a draw.
    Runaway,
}

/// The terminal utilities, +1 / −1 for a win, 0 / 0 otherwise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outcome {
    pub returns: [f64; 2],
    pub cause: Cause,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameError {
    /// The game is over.
    Terminal,
    /// Not among this node's legal actions or chance outcomes.
    Illegal(Action),
    /// Legal by this layer's account, refused by the engine (`Retry`):
    /// a defect in this layer's encoding, never a caller's fault.
    Rejected(Action),
    /// See [`Cause::Runaway`].
    Runaway,
    /// [`Game::sample_chance`] at a node that is not a chance node.
    NotAChanceNode,
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Answer nodes with exactly one legal action (or one chance outcome)
    /// on the way, so a solver sees only choices.
    pub skip_forced: bool,
    /// Keep a rendered trace (and the message log it reads).
    pub tracing: bool,
    /// The processor-step budget of one `apply`.
    pub max_steps_per_apply: u32,
    /// Each player's starting life points (`Game::new` and `configured`).
    pub lp: i32,
    /// Do not offer a move that only returns to a node already offered:
    /// a `Cancel` back to the last menu with nothing changed, an unselect
    /// in a select/unselect question — see the module note, "Cycles".
    pub prune_cycles: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            skip_forced: true,
            tracing: false,
            max_steps_per_apply: 1_000_000,
            lp: 8000,
            prune_cycles: true,
        }
    }
}

/// The selection built up so far at a sequential-pick node.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Partial {
    #[default]
    None,
    /// `SelectCard` / `SelectTribute`: ascending indices into the offer.
    Picks(Vec<usize>),
    /// `SelectPlace`: `(player, location, sequence)` per zone chosen.
    Places(Vec<(u8, u8, u8)>),
    /// `AnnounceRace`: the bits declared so far.
    Types(u64),
    /// `Sort`: the items placed so far, first first.
    Order(Vec<usize>),
    /// `SelectDeckTop`: the cards settled so far, top first.
    DeckTop(Vec<CardId>),
}

/// A duel as a game tree. See the module note.
#[derive(Clone)]
pub struct Game {
    field: Field,
    question: Option<Message>,
    outcome: Option<Outcome>,
    partial: Partial,
    /// How much of the message log the win scan has read.
    seen: usize,
    trace: Option<Trace>,
    config: Config,
    /// Who knows which hidden card — see [`crate::observation`].
    knowledge: Knowledge,
    /// Has anything been announced since the last menu was answered, other
    /// than questions, hints and shuffles? While not, a `Cancel` leads back
    /// to that menu unchanged.
    changed_since_menu: bool,
    /// The cards the answering player chose at the last card question: a
    /// departure of one of these is not ambiguous to them.
    identified: Vec<(u8, CardId)>,

    /// Processor steps so far.
    pub steps: u64,
}

enum Settled {
    Question,
    Over,
}

enum Unsettled {
    Rejected,
    Runaway,
}

impl Game {
    /// A duel about to start, in solver mode, run to its first node.
    pub fn new(decks: [Vec<CardData>; 2], seed: [u64; 4]) -> Self {
        Self::configured(decks, seed, Config::default())
    }

    pub fn configured(decks: [Vec<CardData>; 2], seed: [u64; 4], config: Config) -> Self {
        Self::from_field(new_duel_with_lp(decks, seed, config.lp), config)
    }

    /// Any field, switched to solver mode and run to its next node. What
    /// a determinized state is built from.
    pub fn from_field(mut field: Field, config: Config) -> Self {
        field.set_chance_mode(true);
        let mut game = Self {
            field,
            question: None,
            outcome: None,
            partial: Partial::None,
            seen: 0,
            trace: config.tracing.then(Trace::default),
            config,
            knowledge: Knowledge::default(),
            changed_since_menu: true,
            identified: Vec::new(),
            steps: 0,
        };

        // The cards as dealt were never announced: look once before the
        // first step. A runaway at the start is recorded in the outcome
        // like any other.
        game.knowledge.observe(&game.field, &[], &[]);

        let _ = game.settle();
        game
    }

    pub fn field(&self) -> &Field {
        &self.field
    }

    /// The field, mutable: for a host that settles hidden state itself —
    /// a determinizer imposing a deck order, a probe reordering one. The
    /// game's own bookkeeping (knowledge, the pending question) is not
    /// refreshed by this; a caller that moves a card is on its own.
    pub fn field_mut(&mut self) -> &mut Field {
        &mut self.field
    }

    /// Bring the knowledge model up to date after moving cards through
    /// [`Game::field_mut`], as the game itself does after a step.
    pub fn refresh_knowledge(&mut self) {
        self.knowledge.observe(&self.field, &[], &[]);
    }

    /// The question this node answers, if the game is not over.
    pub fn question(&self) -> Option<&Message> {
        self.question.as_ref()
    }

    pub fn config(&self) -> Config {
        self.config
    }

    pub fn rendered_trace(&self) -> String {
        self.trace.as_ref().map(Trace::render).unwrap_or_default()
    }

    pub fn knowledge(&self) -> &Knowledge {
        &self.knowledge
    }

    /// What `viewer` sees at this node — see [`crate::observation`].
    pub fn observation(&mut self, viewer: u8) -> Observation {
        let partial = self.partial_view();
        let question = self.question.clone();
        observe(
            &mut self.field,
            &self.knowledge,
            question.as_ref(),
            &partial,
            viewer,
        )
    }

    /// The information-set key of `viewer` at this node: the hash of
    /// [`Game::observation`].
    pub fn infoset_key(&mut self, viewer: u8) -> u64 {
        self.observation(viewer).key()
    }

    /// The key of what both players see.
    pub fn public_key(&mut self) -> u64 {
        self.observation(0).public_key()
    }

    fn partial_view(&self) -> PartialView {
        match &self.partial {
            Partial::None => PartialView::default(),
            Partial::Picks(p) => PartialView {
                picks: p.clone(),
                ..PartialView::default()
            },
            Partial::Places(p) => PartialView {
                places: p.clone(),
                ..PartialView::default()
            },
            Partial::Types(t) => PartialView {
                types: *t,
                ..PartialView::default()
            },
            Partial::Order(o) => PartialView {
                order: o.clone(),
                ..PartialView::default()
            },
            Partial::DeckTop(d) => PartialView {
                settled: d.len(),
                ..PartialView::default()
            },
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.outcome.is_some()
    }

    pub fn result(&self) -> Option<Outcome> {
        self.outcome
    }

    pub fn player_to_act(&self) -> Actor {
        if self.outcome.is_some() {
            return Actor::Terminal;
        }
        match &self.question {
            Some(Message::SelectCoin { .. })
            | Some(Message::SelectRandom { .. })
            | Some(Message::SelectDeckTop { .. }) => Actor::Chance,
            Some(q) => asked_player(q).map_or(Actor::Terminal, Actor::Player),
            None => Actor::Terminal,
        }
    }

    /// The actions a player may take here, in a canonical order. Empty at
    /// a chance node and at the end.
    pub fn legal_actions(&self) -> Vec<Action> {
        let Some(q) = &self.question else {
            return Vec::new();
        };
        if self.outcome.is_some() {
            return Vec::new();
        }
        let mut out = Vec::new();
        match q {
            Message::SelectYesNo { .. } | Message::SelectEffectYesNo { .. } => {
                out.push(Action::YesNo(false));
                out.push(Action::YesNo(true));
            }
            Message::SelectOption { options, .. } => {
                out.extend((0..options.len()).map(Action::ChooseOption));
            }
            Message::SelectCard {
                cancelable,
                min,
                max,
                cards,
                ..
            } => {
                let picks = self.picks();
                if picks.len() < usize::from(*max) {
                    let from = picks.last().map_or(0, |&i| i + 1);
                    // A pick must leave enough cards after it to reach the
                    // minimum: ascending order makes the last card a dead
                    // end for a selection that still needs another.
                    let still_needed = usize::from(*min).saturating_sub(picks.len() + 1);
                    out.extend(
                        (from..cards.len())
                            .filter(|&i| cards.len() - 1 - i >= still_needed)
                            .map(Action::Pick),
                    );
                }
                if picks.len() >= usize::from(*min) {
                    out.push(Action::Finish);
                }
                if *cancelable && picks.is_empty() && self.cancel_leads_somewhere() {
                    out.push(Action::Cancel);
                }
            }
            Message::SelectTribute {
                cancelable,
                min,
                max,
                cards,
                ..
            } => {
                let picks = self.picks();
                let value: u32 = picks.iter().map(|&i| cards[i].1).sum();
                if picks.len() < usize::from(*max) {
                    let from = picks.last().map_or(0, |&i| i + 1);
                    // Feasible picks only: the cards after it, at most as
                    // many as may still be picked, must be able to make up
                    // the minimum release value.
                    let more = usize::from(*max) - picks.len() - 1;
                    out.extend(
                        (from..cards.len())
                            .filter(|&i| {
                                let mut after: Vec<u32> =
                                    cards[i + 1..].iter().map(|(_, r)| *r).collect();
                                after.sort_unstable_by(|a, b| b.cmp(a));
                                let best: u32 = after.iter().take(more).sum();
                                value + cards[i].1 + best >= u32::from(*min)
                            })
                            .map(Action::Pick),
                    );
                }
                if value >= u32::from(*min) {
                    out.push(Action::Finish);
                }
                if *cancelable && picks.is_empty() && self.cancel_leads_somewhere() {
                    out.push(Action::Cancel);
                }
            }
            Message::SelectUnselectCard {
                finishable,
                cancelable,
                select,
                unselect,
                ..
            } => {
                out.extend((0..select.len()).map(Action::Pick));
                // An unselect only returns to a node the selects reach
                // (module note, "Cycles"): listed when nothing else is.
                if !self.config.prune_cycles || (select.is_empty() && !*finishable) {
                    out.extend((select.len()..select.len() + unselect.len()).map(Action::Pick));
                }
                if *finishable {
                    out.push(Action::Finish);
                } else if *cancelable && (self.cancel_leads_somewhere() || out.is_empty()) {
                    out.push(Action::Cancel);
                }
            }
            Message::SelectChain { forced, chains, .. } => {
                out.extend((0..chains.len()).map(Action::Chain));
                if !forced {
                    out.push(Action::Decline);
                }
            }
            Message::SelectPlace {
                player,
                flag,
                count,
                ..
            } => {
                let taken = self.places();
                if taken.len() < usize::from(*count) {
                    let mut mask = *flag;
                    for &(p, loc, seq) in &taken {
                        mask |= place_bit(p == *player, loc, seq);
                    }
                    for (p, own) in [(*player, true), (1 - *player, false)] {
                        for (loc, seats) in [(location::MZONE, 7u8), (location::SZONE, 8u8)] {
                            for seq in 0..seats {
                                if mask & place_bit(own, loc, seq) == 0 {
                                    out.push(Action::Place {
                                        player: p,
                                        location: loc,
                                        sequence: seq,
                                    });
                                }
                            }
                        }
                    }
                }
            }
            Message::SelectPosition { positions, .. } => {
                for bit in [0x1u8, 0x2, 0x4, 0x8] {
                    if positions & bit != 0 {
                        out.push(Action::Position(bit));
                    }
                }
            }
            Message::SelectIdleCmd {
                summonable,
                spsummonable,
                repositionable,
                msetable,
                ssetable,
                activatable,
                to_bp,
                to_ep,
                ..
            } => {
                out.extend((0..summonable.len()).map(Action::Summon));
                out.extend((0..spsummonable.len()).map(Action::SpecialSummon));
                out.extend((0..repositionable.len()).map(Action::Reposition));
                out.extend((0..msetable.len()).map(Action::SetMonster));
                out.extend((0..ssetable.len()).map(Action::SetSpellTrap));
                out.extend((0..activatable.len()).map(Action::Activate));
                if *to_bp {
                    out.push(Action::ToBattlePhase);
                }
                if *to_ep {
                    out.push(Action::ToEndPhase);
                }
                // Shuffling the hand (8) is never offered: it changes
                // nothing a player can see and could be taken forever.
            }
            Message::SelectBattleCmd {
                activatable,
                attackable,
                to_m2,
                to_ep,
                ..
            } => {
                out.extend((0..activatable.len()).map(Action::Activate));
                out.extend((0..attackable.len()).map(Action::Attack));
                if *to_m2 {
                    out.push(Action::ToMainPhase2);
                }
                if *to_ep {
                    out.push(Action::ToEndPhase);
                }
            }
            Message::AnnounceRace {
                count, available, ..
            } => {
                let declared = self.types();
                let still_needed = u32::from(*count).saturating_sub(declared.count_ones() + 1);
                if declared.count_ones() < u32::from(*count) {
                    // Ascending, like picks, and only bits with enough
                    // available bits above them for the rest.
                    let from = 64 - declared.leading_zeros() as u8;
                    for bit in from..64u8 {
                        let b = 1u64 << bit;
                        let above = (available >> bit >> 1).count_ones();
                        if available & b != 0 && above >= still_needed {
                            out.push(Action::DeclareType(bit));
                        }
                    }
                }
            }
            Message::Sort {
                is_chain, cards, ..
            } => {
                let order = self.order();
                if *is_chain && order.is_empty() {
                    out.push(Action::Finish);
                }
                out.extend(
                    (0..cards.len())
                        .filter(|i| !order.contains(i))
                        .map(Action::Pick),
                );
            }
            _ => {}
        }
        out
    }

    /// The outcomes of a chance node with their probabilities, in a
    /// canonical order. Empty at a player node and at the end.
    pub fn chance_outcomes(&self) -> Vec<(Action, f64)> {
        let Some(q) = &self.question else {
            return Vec::new();
        };
        if self.outcome.is_some() {
            return Vec::new();
        }
        match q {
            Message::SelectCoin { count, .. } => {
                let n = 1u32 << count.min(&31);
                let p = 1.0 / f64::from(n);
                (0..n).map(|mask| (Action::Coin(mask), p)).collect()
            }
            Message::SelectRandom { count, cards, .. } => {
                let subsets = combinations(cards.len(), usize::from(*count));
                let p = 1.0 / subsets.len() as f64;
                subsets
                    .into_iter()
                    .map(|s| (Action::RandomPick(s), p))
                    .collect()
            }
            Message::SelectDeckTop { player, .. } => {
                let settled = self.deck_top();
                let mut remaining: Vec<CardId> = self.field.players[usize::from(*player)]
                    .main
                    .iter()
                    .copied()
                    .filter(|c| !settled.contains(c))
                    .collect();
                remaining.sort_unstable();
                let p = 1.0 / remaining.len() as f64;
                remaining
                    .into_iter()
                    .map(|c| (Action::DeckTop(c), p))
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// Take `action` at this node and run to the next one. A pick that
    /// leaves the selection open returns at the same question with the
    /// pick recorded; anything else answers the engine.
    pub fn apply(&mut self, action: &Action) -> Result<(), GameError> {
        if self.outcome.is_some() {
            return Err(GameError::Terminal);
        }
        let Some(q) = self.question.clone() else {
            return Err(GameError::Terminal);
        };
        let legal = match self.player_to_act() {
            Actor::Chance => self.chance_outcomes().iter().any(|(o, _)| o == action),
            Actor::Player(_) => self.legal_actions().contains(action),
            Actor::Terminal => false,
        };
        if !legal {
            return Err(GameError::Illegal(action.clone()));
        }
        if !self.encode(&q, action) {
            // The selection is still open. With the skip on, a pick that
            // leaves exactly one continuation is taken too, until there is
            // a choice again or the engine is answered.
            if !self.config.skip_forced || !self.finish_forced(&q) {
                return Ok(());
            }
        }
        self.answered(&q);
        let settled = self.settle();
        self.identified.clear();
        match settled {
            Ok(_) => Ok(()),
            Err(Unsettled::Rejected) => Err(GameError::Rejected(action.clone())),
            Err(Unsettled::Runaway) => Err(GameError::Runaway),
        }
    }

    /// Resolve the chance node from the duel's own generator — the roll
    /// the faithful mode would have made ([`Field::draw_chance`]) — and
    /// run on. A deck-top node takes the deck as the shuffle left it.
    pub fn sample_chance(&mut self) -> Result<(), GameError> {
        if self.player_to_act() != Actor::Chance {
            return Err(GameError::NotAChanceNode);
        }
        let Some(sample) = self.field.draw_chance() else {
            return Err(GameError::NotAChanceNode);
        };
        match sample {
            ChanceSample::Coin(mask) => self.apply(&Action::Coin(mask)),
            ChanceSample::Random(picks) => self.apply(&Action::RandomPick(picks)),
            ChanceSample::DeckTop => {
                let Some(Message::SelectDeckTop { player, count }) = self.question.clone() else {
                    return Err(GameError::NotAChanceNode);
                };
                let settled = self.deck_top();
                let next: Vec<CardId> = self.field.players[usize::from(player)]
                    .main
                    .iter()
                    .rev()
                    .copied()
                    .filter(|c| !settled.contains(c))
                    .take(count as usize - settled.len())
                    .collect();
                let asked = self.question.clone();
                for c in next {
                    // With the forced-node skip on, the last card of a look
                    // is settled on the way once one remains, and the game
                    // has moved on: nothing more to place here.
                    if self.question != asked {
                        break;
                    }
                    self.apply(&Action::DeckTop(c))?;
                }
                Ok(())
            }
        }
    }

    /// Whether a `Cancel` here goes anywhere: it does unless the question
    /// followed a menu answer with nothing announced since, in which case
    /// it returns to that menu unchanged — a cycle, pruned by default.
    fn cancel_leads_somewhere(&self) -> bool {
        !self.config.prune_cycles || self.changed_since_menu
    }

    // ---- the partial selection, by shape -----------------------------------

    fn picks(&self) -> Vec<usize> {
        match &self.partial {
            Partial::Picks(p) => p.clone(),
            _ => Vec::new(),
        }
    }

    fn places(&self) -> Vec<(u8, u8, u8)> {
        match &self.partial {
            Partial::Places(p) => p.clone(),
            _ => Vec::new(),
        }
    }

    fn types(&self) -> u64 {
        match &self.partial {
            Partial::Types(t) => *t,
            _ => 0,
        }
    }

    fn order(&self) -> Vec<usize> {
        match &self.partial {
            Partial::Order(o) => o.clone(),
            _ => Vec::new(),
        }
    }

    fn deck_top(&self) -> Vec<CardId> {
        match &self.partial {
            Partial::DeckTop(d) => d.clone(),
            _ => Vec::new(),
        }
    }

    // ---- answering ----------------------------------------------------------

    /// Record `action` against `q`. True when the engine has been answered
    /// — the selection is complete or the action was not a pick.
    fn encode(&mut self, q: &Message, action: &Action) -> bool {
        let r = &mut self.field.core.returns;
        match (q, action) {
            (Message::SelectYesNo { .. } | Message::SelectEffectYesNo { .. }, Action::YesNo(b)) => {
                r.set(i32::from(*b));
            }
            (Message::SelectOption { .. }, Action::ChooseOption(i)) => r.set(*i as i32),
            (
                Message::SelectCard { max, .. } | Message::SelectTribute { max, .. },
                Action::Pick(i),
            ) => {
                let mut picks = self.picks();
                picks.push(*i);
                if picks.len() < usize::from(*max) {
                    self.partial = Partial::Picks(picks);
                    return false;
                }
                self.identify(q, &picks);
                self.write_cards(&picks);
            }
            (Message::SelectCard { .. } | Message::SelectTribute { .. }, Action::Finish) => {
                let picks = self.picks();
                self.identify(q, &picks);
                self.write_cards(&picks);
            }

            (Message::SelectCard { .. } | Message::SelectTribute { .. }, Action::Cancel) => {
                self.field.core.return_cards.clear();
                self.field.core.returns.set_i32(0, -1);
            }
            (Message::SelectUnselectCard { .. }, Action::Pick(i)) => {
                r.set_i32(0, 1);
                r.set_i32(1, *i as i32);
                self.identify(q, &[*i]);
            }

            (Message::SelectUnselectCard { .. }, Action::Finish | Action::Cancel) => {
                r.set_i32(0, -1);
            }
            (Message::SelectChain { .. }, Action::Chain(i)) => r.set(*i as i32),
            (Message::SelectChain { .. }, Action::Decline) => r.set(-1),
            (
                Message::SelectPlace { count, .. },
                Action::Place {
                    player,
                    location,
                    sequence,
                },
            ) => {
                let mut places = self.places();
                places.push((*player, *location, *sequence));
                if places.len() < usize::from(*count) {
                    self.partial = Partial::Places(places);
                    return false;
                }
                let r = &mut self.field.core.returns;
                for (k, (p, loc, seq)) in places.iter().enumerate() {
                    r.set_i8(3 * k, *p as i8);
                    r.set_i8(3 * k + 1, *loc as i8);
                    r.set_i8(3 * k + 2, *seq as i8);
                }
            }
            (Message::SelectPosition { .. }, Action::Position(bit)) => r.set(i32::from(*bit)),
            (Message::SelectIdleCmd { .. }, a) => {
                self.changed_since_menu = false;
                let (kind, index) = match a {
                    Action::Summon(i) => (0u32, *i),
                    Action::SpecialSummon(i) => (1, *i),
                    Action::Reposition(i) => (2, *i),
                    Action::SetMonster(i) => (3, *i),
                    Action::SetSpellTrap(i) => (4, *i),
                    Action::Activate(i) => (5, *i),
                    Action::ToBattlePhase => (6, 0),
                    Action::ToEndPhase => (7, 0),
                    _ => unreachable!("checked legal"),
                };
                r.set(((index as u32) << 16 | kind) as i32);
            }
            (Message::SelectBattleCmd { .. }, a) => {
                self.changed_since_menu = false;
                let (kind, index) = match a {
                    Action::Activate(i) => (0u32, *i),
                    Action::Attack(i) => (1, *i),
                    Action::ToMainPhase2 => (2, 0),
                    Action::ToEndPhase => (3, 0),
                    _ => unreachable!("checked legal"),
                };
                r.set(((index as u32) << 16 | kind) as i32);
            }
            (Message::AnnounceRace { count, .. }, Action::DeclareType(bit)) => {
                let declared = self.types() | (1u64 << bit);
                if declared.count_ones() < u32::from(*count) {
                    self.partial = Partial::Types(declared);
                    return false;
                }
                self.field.core.returns.set_u64(0, declared);
            }
            (Message::Sort { cards, .. }, Action::Pick(i)) => {
                let mut order = self.order();
                order.push(*i);
                if order.len() < cards.len() {
                    self.partial = Partial::Order(order);
                    return false;
                }
                // Slot `i` says where item `i` goes.
                let r = &mut self.field.core.returns;
                for (rank, &item) in order.iter().enumerate() {
                    r.set_i8(item, rank as i8);
                }
            }
            (Message::Sort { .. }, Action::Finish) => r.set_i8(0, -1),
            (Message::SelectCoin { .. }, Action::Coin(mask)) => r.set(*mask as i32),
            (Message::SelectRandom { .. }, Action::RandomPick(picks)) => {
                let picks = picks.clone();
                self.write_cards(&picks);
            }
            (Message::SelectDeckTop { player, count }, Action::DeckTop(card)) => {
                let mut settled = self.deck_top();
                settled.push(*card);
                if settled.len() < *count as usize {
                    self.partial = Partial::DeckTop(settled);
                    return false;
                }
                // The pile is bottom-first: the first settled card goes on top.
                let pile = &self.field.players[usize::from(*player)].main;
                let mut order: Vec<CardId> = pile
                    .iter()
                    .copied()
                    .filter(|c| !settled.contains(c))
                    .collect();
                order.extend(settled.iter().rev());
                let imposed = self.field.set_deck_order(*player, &order);
                debug_assert!(imposed, "a permutation of the deck");
                self.field.core.returns.set(0);
            }
            _ => unreachable!("checked legal"),
        }
        true
    }

    /// Remember what the answering player chose: a departure of one of
    /// these cards is not ambiguous to them.
    fn identify(&mut self, q: &Message, picks: &[usize]) {
        let (player, cards): (u8, Vec<CardId>) = match q {
            Message::SelectCard { player, cards, .. } => {
                (*player, picks.iter().map(|&i| cards[i]).collect())
            }
            Message::SelectTribute { player, cards, .. } => {
                (*player, picks.iter().map(|&i| cards[i].0).collect())
            }
            Message::SelectUnselectCard {
                player,
                select,
                unselect,
                ..
            } => (
                *player,
                picks
                    .iter()
                    .map(|&i| {
                        if i < select.len() {
                            select[i]
                        } else {
                            unselect[i - select.len()]
                        }
                    })
                    .collect(),
            ),
            _ => return,
        };
        self.identified = cards.into_iter().map(|c| (player, c)).collect();
    }

    /// The card-selection answer: type 0, the count, then indices into the
    /// offer.
    fn write_cards(&mut self, picks: &[usize]) {
        self.field.core.return_cards.clear();

        let r = &mut self.field.core.returns;
        r.set_i32(0, 0);
        r.set_i32(1, picks.len() as i32);
        for (i, idx) in picks.iter().enumerate() {
            r.set_i32(i + 2, *idx as i32);
        }
    }

    fn answered(&mut self, q: &Message) {
        if let Some(t) = self.trace.as_mut() {
            t.answered(q, &self.field.core.returns);
        }
        self.partial = Partial::None;
    }

    // ---- running the field to the next node ---------------------------------

    /// Run the processor until it asks something with a choice, or the game
    /// ends. Forced nodes are answered on the way when configured.
    fn settle(&mut self) -> Result<Settled, Unsettled> {
        for _ in 0..self.config.max_steps_per_apply {
            self.steps += 1;
            let status = self.field.process();
            // Every move, flip and control change is announced, so a step
            // that announced nothing moved nothing: the knowledge scan is
            // skipped, and it is most steps.
            let announced = self.field.messages.len() > self.seen;
            let revealed = std::mem::take(&mut self.field.core.revealed);
            if announced || !revealed.is_empty() {
                self.knowledge
                    .observe(&self.field, &revealed, &self.identified);
            }

            if !self.changed_since_menu
                && self.field.messages[self.seen.min(self.field.messages.len())..]
                    .iter()
                    .any(announces_change)
            {
                self.changed_since_menu = true;
            }
            if let Some(t) = self.trace.as_mut() {
                t.absorb(&self.field.messages);
            }
            if let Some(outcome) = self.scan_win() {
                self.finish(outcome);
                return Ok(Settled::Over);
            }
            match status {
                Status::Continue => self.drop_messages(),
                Status::End => {
                    self.finish(Outcome {
                        returns: [0.0, 0.0],
                        cause: Cause::Finished,
                    });
                    return Ok(Settled::Over);
                }
                Status::Awaiting => {
                    if matches!(self.field.messages.last(), Some(Message::Retry)) {
                        // The question stands; the answer was refused.
                        self.partial = Partial::None;
                        self.drop_messages();
                        return Err(Unsettled::Rejected);
                    }
                    let Some(q) = self
                        .field
                        .messages
                        .iter()
                        .rev()
                        .find(|m| is_question(m))
                        .cloned()
                    else {
                        return Err(Unsettled::Runaway);
                    };
                    self.question = Some(q.clone());
                    self.partial = Partial::None;
                    self.drop_messages();
                    if !self.config.skip_forced {
                        return Ok(Settled::Question);
                    }
                    // Answer a forced node on the way.
                    if !self.finish_forced(&q) {
                        return Ok(Settled::Question);
                    }
                    self.answered(&q);
                }
            }
        }
        self.finish(Outcome {
            returns: [0.0, 0.0],
            cause: Cause::Runaway,
        });
        Err(Unsettled::Runaway)
    }

    /// Take forced actions at `q` while there is no choice — a forced pick
    /// may lead to another before the engine is answered. True when the
    /// engine has been answered; false at a node with a choice.
    fn finish_forced(&mut self, q: &Message) -> bool {
        loop {
            let Some(sole) = self.sole_action() else {
                return false;
            };
            if self.encode(q, &sole) {
                return true;
            }
        }
    }

    /// The one action or outcome at a forced node.
    fn sole_action(&self) -> Option<Action> {
        match self.player_to_act() {
            Actor::Player(_) => {
                let actions = self.legal_actions();
                (actions.len() == 1).then(|| actions[0].clone())
            }
            Actor::Chance => {
                let outcomes = self.chance_outcomes();
                (outcomes.len() == 1).then(|| outcomes[0].0.clone())
            }
            Actor::Terminal => None,
        }
    }

    fn finish(&mut self, outcome: Outcome) {
        self.outcome = Some(outcome);
        self.question = None;
        self.partial = Partial::None;
        self.drop_messages();
    }

    fn scan_win(&mut self) -> Option<Outcome> {
        let found = self.field.messages[self.seen.min(self.field.messages.len())..]
            .iter()
            .find_map(|m| match m {
                Message::Win { player, reason } => Some(match *player {
                    0 => Outcome {
                        returns: [1.0, -1.0],
                        cause: Cause::Win {
                            player: 0,
                            reason: *reason,
                        },
                    },
                    1 => Outcome {
                        returns: [-1.0, 1.0],
                        cause: Cause::Win {
                            player: 1,
                            reason: *reason,
                        },
                    },
                    _ => Outcome {
                        returns: [0.0, 0.0],
                        cause: Cause::Draw { reason: *reason },
                    },
                }),
                _ => None,
            });
        self.seen = self.field.messages.len();
        found
    }

    /// The message log is read (the question taken, the win scanned, the
    /// trace absorbed) — drop it, unless the trace needs it whole.
    fn drop_messages(&mut self) {
        if self.trace.is_none() {
            self.field.messages.clear();
            self.seen = 0;
        }
    }
}

/// Does this message announce a change of state? Questions, hints, a
/// retry and shuffles do not: a shuffle moves nothing a player can see.
/// Everything else — a move, a flip, life points, a chain link, an attack
/// — does.
fn announces_change(m: &Message) -> bool {
    !(is_question(m)
        || matches!(
            m,
            Message::Hint { .. }
                | Message::Retry
                | Message::ShuffleHand { .. }
                | Message::ShuffleDeck { .. }
                | Message::ShuffleExtra { .. }
        ))
}

/// The player a question is asked of. `None` for the chance questions.
fn asked_player(m: &Message) -> Option<u8> {
    match m {
        Message::SelectYesNo { player, .. }
        | Message::SelectEffectYesNo { player, .. }
        | Message::SelectOption { player, .. }
        | Message::SelectCard { player, .. }
        | Message::SelectUnselectCard { player, .. }
        | Message::SelectChain { player, .. }
        | Message::SelectPlace { player, .. }
        | Message::SelectPosition { player, .. }
        | Message::SelectIdleCmd { player, .. }
        | Message::SelectBattleCmd { player, .. }
        | Message::SelectTribute { player, .. }
        | Message::AnnounceRace { player, .. }
        | Message::Sort { player, .. } => Some(*player),
        _ => None,
    }
}

/// The `SelectPlace` flag bit of a zone: monster seats in the low byte,
/// Spell & Trap seats in the next, the opponent's side sixteen bits up.
fn place_bit(own_side: bool, loc: u8, seq: u8) -> u32 {
    let mut bit = 1u32 << seq;
    if loc == location::SZONE {
        bit <<= 8;
    }
    if !own_side {
        bit <<= 16;
    }
    bit
}

/// Every `k`-subset of `0..n`, each ascending, in lexicographic order.
fn combinations(n: usize, k: usize) -> Vec<Vec<usize>> {
    fn rec(start: usize, n: usize, k: usize, cur: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if cur.len() == k {
            out.push(cur.clone());
            return;
        }
        for i in start..n {
            cur.push(i);
            rec(i + 1, n, k, cur, out);
            cur.pop();
        }
    }
    let mut out = Vec::new();
    rec(0, n, k, &mut Vec::with_capacity(k), &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, status, Card};
    use crate::cards::{card_data, POOL};
    use crate::driver::{BattleMenu, Driver, IdleMenu, Policy, RandomPolicy, Stop};
    use crate::processor::Kind;

    /// A pool deck for game `game`, as `examples/bench.rs` deals one.
    fn pool_deck(game: u64) -> Vec<CardData> {
        let mut rng = RandomPolicy::new(7000 + game);
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

    fn no_skip() -> Config {
        Config {
            skip_forced: false,
            ..Config::default()
        }
    }

    fn hand_card(f: &mut Field, owner: u8, seq: u32) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: 1_000 + seq,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.current.position = position::FACEDOWN_DEFENSE;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, location::HAND, seq, false);
        id
    }

    /// A game at one synthetic question, the message log kept.
    fn at(f: Field) -> Game {
        Game::from_field(
            f,
            Config {
                skip_forced: false,
                tracing: true,
                ..Config::default()
            },
        )
    }

    // ---- the opening ----------------------------------------------------------

    /// **The opening hands are chance nodes.** Solver mode's first node is
    /// the first player's five-card draw: forty outcomes at 1/40, listed by
    /// card id and not by the deck's order, and five of them in a row
    /// before the other player's five.
    #[test]
    fn the_opening_hands_are_chance_nodes() {
        let deck = pool_deck(0);
        let mut g = Game::configured([deck.clone(), deck], [1, 2, 3, 4], no_skip());
        assert_eq!(g.player_to_act(), Actor::Chance);
        assert!(
            g.legal_actions().is_empty(),
            "no player action at a chance node"
        );
        // The Main Deck: a pool deck's Fusion Monsters went to the Extra Deck.
        let n = g.field().players[0].main.len();
        assert!((30..=40).contains(&n), "{n}");
        let outcomes = g.chance_outcomes();
        assert_eq!(outcomes.len(), n);
        assert!(outcomes
            .iter()
            .all(|(_, p)| (p - 1.0 / n as f64).abs() < 1e-12));
        let ids: Vec<CardId> = outcomes
            .iter()
            .map(|(a, _)| match a {
                Action::DeckTop(c) => *c,
                other => panic!("{other:?}"),
            })
            .collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "by card id");
        assert_eq!(
            g.question(),
            Some(&Message::SelectDeckTop {
                player: 0,
                count: 5
            })
        );

        // Settle the five, choosing the highest id each time.
        for k in 0..5 {
            let outcomes = g.chance_outcomes();
            assert_eq!(outcomes.len(), n - k, "one fewer to choose from");
            let (last, _) = outcomes.last().unwrap().clone();
            g.apply(&last).unwrap();
        }
        assert_eq!(
            g.question(),
            Some(&Message::SelectDeckTop {
                player: 1,
                count: 5
            }),
            "then the other player's five"
        );
        // The chosen five are the first player's hand, highest id drawn
        // first.
        let hand = &g.field().players[0].hand;
        assert_eq!(hand.len(), 5);
        let mut expect: Vec<CardId> = g.field().players[0].main.to_vec();
        expect.extend(hand.iter().copied());
        expect.sort_unstable();
        assert_eq!(hand[0], expect[n - 1], "the highest id was drawn first");
    }

    // ---- sequential picks ---------------------------------------------------

    /// **Picks are sequential and ascending.** Two of four: a pick, then
    /// only later cards or a finish; the second pick closes the selection
    /// without a finish; the engine reads the same answer the driver
    /// would have written.
    #[test]
    fn picks_are_sequential_ascending_and_close_at_the_maximum() {
        let mut f = Field::new(8000);
        let cards: Vec<CardId> = (0..4).map(|i| hand_card(&mut f, 0, i)).collect();
        f.core.select_cards.clone_from(&cards);
        f.emplace(Kind::SelectCard {
            player: 0,
            cancelable: false,
            min: 1,
            max: 2,
        });
        let mut g = at(f);
        let Some(Message::SelectCard { cards: offered, .. }) = g.question().cloned() else {
            panic!("{:?}", g.question());
        };
        assert_eq!(g.player_to_act(), Actor::Player(0));
        assert_eq!(
            g.legal_actions(),
            vec![
                Action::Pick(0),
                Action::Pick(1),
                Action::Pick(2),
                Action::Pick(3)
            ],
            "nothing picked: no finish below the minimum, no cancel"
        );
        g.apply(&Action::Pick(2)).unwrap();
        assert_eq!(
            g.legal_actions(),
            vec![Action::Pick(3), Action::Finish],
            "after the third card: only the fourth, or finish"
        );
        assert_eq!(
            g.apply(&Action::Pick(1)),
            Err(GameError::Illegal(Action::Pick(1))),
            "an earlier card is not a legal later pick"
        );
        let mut two = g.clone();
        g.apply(&Action::Finish).unwrap();
        assert!(g.is_terminal(), "the queue emptied: {:?}", g.question());
        assert_eq!(g.field().core.return_cards.list, vec![offered[2]]);

        two.apply(&Action::Pick(3)).unwrap();
        assert!(two.is_terminal(), "the maximum closes the selection");
        assert_eq!(
            two.field().core.return_cards.list,
            vec![offered[2], offered[3]]
        );
    }

    /// A cancelable selection offers a cancel until something is picked;
    /// a minimum of zero offers a finish at once.
    #[test]
    fn cancel_and_finish_follow_the_question() {
        let mut f = Field::new(8000);
        let cards: Vec<CardId> = (0..3).map(|i| hand_card(&mut f, 0, i)).collect();
        f.core.select_cards.clone_from(&cards);
        f.emplace(Kind::SelectCard {
            player: 0,
            cancelable: true,
            min: 0,
            max: 3,
        });
        let mut g = at(f);
        assert_eq!(
            g.legal_actions(),
            vec![
                Action::Pick(0),
                Action::Pick(1),
                Action::Pick(2),
                Action::Finish,
                Action::Cancel
            ]
        );
        g.apply(&Action::Pick(0)).unwrap();
        assert!(
            !g.legal_actions().contains(&Action::Cancel),
            "no cancel once something is picked"
        );
        let mut c = at({
            let mut f = Field::new(8000);
            let cards: Vec<CardId> = (0..3).map(|i| hand_card(&mut f, 0, i)).collect();
            f.core.select_cards.clone_from(&cards);
            f.emplace(Kind::SelectCard {
                player: 0,
                cancelable: true,
                min: 1,
                max: 3,
            });
            f
        });
        c.apply(&Action::Cancel).unwrap();
        assert!(c.is_terminal());
        assert!(c.field().core.return_cards.canceled, "cancelled");
    }

    /// **A tribute's minimum counts release value.** Two offers worth two
    /// each against a minimum of two: one pick already allows a finish.
    #[test]
    fn a_tribute_finish_needs_the_release_value() {
        let mut f = Field::new(8000);
        let cards: Vec<CardId> = (0..2)
            .map(|i| {
                let c = hand_card(&mut f, 0, i);
                f.cards[c].release_param = 2;
                c
            })
            .collect();
        f.core.select_cards.clone_from(&cards);
        f.emplace(Kind::SelectTributeP {
            player: 0,
            cancelable: false,
            min: 2,
            max: 2,
        });
        let mut g = at(f);
        assert!(matches!(g.question(), Some(Message::SelectTribute { .. })));
        assert_eq!(g.legal_actions(), vec![Action::Pick(0), Action::Pick(1)]);
        g.apply(&Action::Pick(1)).unwrap();
        assert_eq!(g.legal_actions(), vec![Action::Finish], "worth two: enough");
        g.apply(&Action::Finish).unwrap();
        assert!(g.is_terminal());
        assert_eq!(g.field().core.return_cards.list.len(), 1);
    }

    /// A zone question with two free seats offers both; with two to
    /// choose, the second is forced once the first is taken.
    #[test]
    fn zones_come_from_the_flag() {
        let mut f = Field::new(8000);
        // Everything unavailable except own monster seat 2 and own S/T seat 4.
        let flag = !(place_bit(true, location::MZONE, 2) | place_bit(true, location::SZONE, 4));
        f.emplace(Kind::SelectPlace {
            player: 0,
            flag,
            count: 2,
            disable_field: false,
        });
        let mut g = at(f);
        let m = Action::Place {
            player: 0,
            location: location::MZONE,
            sequence: 2,
        };
        let s = Action::Place {
            player: 0,
            location: location::SZONE,
            sequence: 4,
        };
        assert_eq!(g.legal_actions(), vec![m.clone(), s.clone()]);
        g.apply(&s).unwrap();
        assert_eq!(g.legal_actions(), vec![m.clone()], "the taken seat is gone");
        g.apply(&m).unwrap();
        assert!(g.is_terminal(), "answered: {:?}", g.question());
        let r = &g.field().core.returns;
        assert_eq!(
            [
                r.at_i8(0),
                r.at_i8(1),
                r.at_i8(2),
                r.at_i8(3),
                r.at_i8(4),
                r.at_i8(5)
            ],
            [0, location::SZONE as i8, 4, 0, location::MZONE as i8, 2]
        );
    }

    /// Types are declared one bit at a time, ascending, until the count.
    #[test]
    fn types_are_declared_one_bit_at_a_time() {
        let mut f = Field::new(8000);
        f.emplace(Kind::AnnounceRace {
            playerid: 0,
            count: 2,
            available: 0b1101,
        });
        let mut g = at(f);
        assert_eq!(
            g.legal_actions(),
            vec![Action::DeclareType(0), Action::DeclareType(2)],
            "ascending: the top bit first would leave nothing for the second"
        );
        g.apply(&Action::DeclareType(2)).unwrap();
        assert_eq!(
            g.legal_actions(),
            vec![Action::DeclareType(3)],
            "only bits above the last declared"
        );
        g.apply(&Action::DeclareType(3)).unwrap();
        assert!(g.is_terminal());
        assert_eq!(g.field().core.returns.at_u64(0), 0b1100);
    }

    /// A sort is built next-item-first; the answer is each item's rank.
    #[test]
    fn a_sort_is_built_an_item_at_a_time() {
        let mut f = Field::new(8000);
        let cards: Vec<CardId> = (0..3).map(|i| hand_card(&mut f, 0, i)).collect();
        f.core.select_cards.clone_from(&cards);
        f.emplace(Kind::SortCard {
            player: 0,
            is_chain: false,
        });
        let mut g = at(f);
        assert!(matches!(g.question(), Some(Message::Sort { .. })));
        assert_eq!(
            g.legal_actions(),
            vec![Action::Pick(0), Action::Pick(1), Action::Pick(2)],
            "no keep-order finish for a card sort"
        );
        g.apply(&Action::Pick(2)).unwrap();
        g.apply(&Action::Pick(0)).unwrap();
        assert_eq!(g.legal_actions(), vec![Action::Pick(1)]);
        g.apply(&Action::Pick(1)).unwrap();
        assert!(g.is_terminal());
        let r = &g.field().core.returns;
        assert_eq!([r.at_i8(0), r.at_i8(1), r.at_i8(2)], [1, 2, 0], "ranks");
    }

    // ---- chance -------------------------------------------------------------

    #[test]
    fn coins_and_random_picks_enumerate_uniformly() {
        let mut f = Field::new(8000);
        f.set_chance_mode(true);
        f.toss_coin(None, 0, 0, 3);
        let g = at(f);
        assert_eq!(g.player_to_act(), Actor::Chance);
        let outcomes = g.chance_outcomes();
        assert_eq!(outcomes.len(), 8);
        assert!(outcomes.iter().all(|(_, p)| (p - 0.125).abs() < 1e-12));
        assert_eq!(outcomes[5].0, Action::Coin(5));

        let mut f = Field::new(8000);
        let cards: Vec<CardId> = (0..4).map(|i| hand_card(&mut f, 0, i)).collect();
        f.set_chance_mode(true);
        crate::script_api::random_select_request(&mut f, &cards, 0, 2);
        let g = at(f);
        let outcomes = g.chance_outcomes();
        assert_eq!(outcomes.len(), 6, "C(4, 2)");
        assert_eq!(outcomes[0].0, Action::RandomPick(vec![0, 1]));
        assert_eq!(outcomes[5].0, Action::RandomPick(vec![2, 3]));
        assert!(outcomes.iter().all(|(_, p)| (p - 1.0 / 6.0).abs() < 1e-12));
    }

    /// **Sampling a whole-deck look settles it once.** Magical Merchant
    /// with nothing to find asks for the entire deck; with the forced-node
    /// skip on, the last card is settled on the way once one remains, and
    /// the sampler must not place it a second time.
    #[test]
    fn sampling_a_whole_deck_look_settles_it_once() {
        let mut f = Field::new(8000);
        let deck: Vec<CardId> = (0..3)
            .map(|i| {
                let c = f.new_card_nowhere(Card::with_data(
                    CardData {
                        code: 90 + i,
                        type_: card_type::MONSTER | card_type::NORMAL,
                        ..Default::default()
                    },
                    1,
                ));
                f.add_card(1, c, location::DECK, 0, false);
                c
            })
            .collect();
        f.set_chance_mode(true);
        crate::script_api::reveal_deck_request(&mut f, 1, 3);
        let mut g = Game::from_field(f, Config::default());
        assert_eq!(g.player_to_act(), Actor::Chance);
        assert_eq!(g.chance_outcomes().len(), 3);
        let before = g.field().players[1].main.clone();
        g.sample_chance().unwrap();
        assert!(
            g.is_terminal() || g.player_to_act() != Actor::Chance,
            "{:?}",
            g.question()
        );
        assert_eq!(g.field().players[1].main, before, "as the shuffle left it");
        let _ = deck;
    }

    /// **Sampling chance leaves the deck as it was.** The generator's
    /// answer to a deck-top node is the order the shuffle rolled, so the
    /// settled order is the current one and nothing moves.
    #[test]
    fn sampling_a_deck_top_node_keeps_the_order() {
        let deck = pool_deck(2);
        let mut g = Game::configured([deck.clone(), deck], [1, 2, 3, 4], no_skip());
        let before = g.field().players[0].main.clone();
        g.sample_chance().unwrap();
        let mut after = g.field().players[0].main.clone();
        after.extend(g.field().players[0].hand.iter().rev());
        assert_eq!(after, before, "the five came off the top in order");
        assert_eq!(
            g.sample_chance_at_a_player_node(),
            Err(GameError::NotAChanceNode)
        );
    }

    impl Game {
        fn sample_chance_at_a_player_node(&mut self) -> Result<(), GameError> {
            while self.player_to_act() == Actor::Chance {
                self.sample_chance()?;
            }
            self.sample_chance()
        }
    }

    // ---- the whole tree -----------------------------------------------------

    /// **Every legal action is accepted.** Random pool games played by
    /// choosing uniformly among what this layer offers: the engine must
    /// never refuse (`Rejected`), no player node may be empty, and with the
    /// skip on none may be forced.
    #[test]
    fn every_legal_action_is_accepted() {
        let mut decisions = 0usize;
        let mut ended = 0usize;
        for game in 0..6u64 {
            let deck = pool_deck(game);
            let mut g = Game::new([deck.clone(), deck], [9, 8, 7, game]);
            let mut rng = RandomPolicy::new(500 + game);
            for _ in 0..4_000 {
                match g.player_to_act() {
                    Actor::Terminal => {
                        ended += 1;
                        break;
                    }
                    Actor::Chance => {
                        let outcomes = g.chance_outcomes();
                        assert!(outcomes.len() > 1, "a forced chance node was offered");
                        let total: f64 = outcomes.iter().map(|(_, p)| p).sum();
                        assert!(
                            (total - 1.0).abs() < 1e-9,
                            "probabilities sum to one: {total}"
                        );
                        let pick = outcomes[rng.below(outcomes.len())].0.clone();
                        g.apply(&pick)
                            .unwrap_or_else(|e| panic!("game {game}: {pick:?}: {e:?}"));
                    }
                    Actor::Player(_) => {
                        let actions = g.legal_actions();
                        assert!(
                            actions.len() > 1,
                            "a forced node was offered: {:?}",
                            g.question()
                        );
                        let a = actions[rng.below(actions.len())].clone();
                        decisions += 1;
                        g.apply(&a).unwrap_or_else(|e| {
                            panic!("game {game}: {a:?} at {:?}: {e:?}", g.question())
                        });
                    }
                }
            }
        }
        assert!(decisions > 1_500, "{decisions} decisions");
        assert!(ended >= 1, "some game ended: {ended}");
    }

    /// An illegal action is refused and changes nothing.
    #[test]
    fn an_illegal_action_changes_nothing() {
        let deck = pool_deck(1);
        let mut g = Game::new([deck.clone(), deck], [1, 2, 3, 4]);
        let before = g.question().cloned();
        let steps = g.steps;
        assert_eq!(
            g.apply(&Action::YesNo(true)),
            Err(GameError::Illegal(Action::YesNo(true)))
        );
        assert_eq!(
            g.apply(&Action::DeckTop(99_999)),
            Err(GameError::Illegal(Action::DeckTop(99_999)))
        );
        assert_eq!(g.question().cloned(), before);
        assert_eq!(g.steps, steps);
        while g.player_to_act() == Actor::Chance {
            g.sample_chance().unwrap();
        }
        assert!(matches!(g.player_to_act(), Actor::Player(_)));
        assert_eq!(
            g.apply(&Action::Coin(0)),
            Err(GameError::Illegal(Action::Coin(0)))
        );
        assert_eq!(
            g.apply(&Action::Summon(99)),
            Err(GameError::Illegal(Action::Summon(99)))
        );
    }

    /// A game over refuses everything.
    #[test]
    fn a_finished_game_refuses_everything() {
        let mut f = Field::new(8000);
        let cards: Vec<CardId> = (0..2).map(|i| hand_card(&mut f, 0, i)).collect();
        f.core.select_cards.clone_from(&cards);
        f.emplace(Kind::SelectCard {
            player: 0,
            cancelable: false,
            min: 1,
            max: 1,
        });
        let mut g = at(f);
        g.apply(&Action::Pick(0)).unwrap();
        assert!(g.is_terminal());
        assert_eq!(g.player_to_act(), Actor::Terminal);
        assert_eq!(g.result().map(|o| o.cause), Some(Cause::Finished));
        assert!(g.legal_actions().is_empty());
        assert!(g.chance_outcomes().is_empty());
        assert_eq!(g.apply(&Action::Pick(0)), Err(GameError::Terminal));
    }

    // ---- equivalence with the driver ------------------------------------------

    /// What `RandomPolicy` would answer, as this layer's actions. One policy
    /// call per question, the driver's own, so the generator is consumed
    /// exactly as the driver consumes it.
    fn actions_for(q: &Message, policy: &mut RandomPolicy, turn_id: i16) -> Vec<Action> {
        match q {
            Message::SelectYesNo {
                player,
                description,
            } => vec![Action::YesNo(policy.yes_no(*player, *description))],
            Message::SelectEffectYesNo {
                player,
                description,
                ..
            } => vec![Action::YesNo(policy.effect_yes_no(*player, *description))],
            Message::SelectOption { player, options } => vec![Action::ChooseOption(
                policy
                    .option(*player, options.len(), turn_id % 2 == 1)
                    .min(options.len().saturating_sub(1)),
            )],
            Message::SelectCard {
                player,
                cards,
                min,
                max,
                ..
            } => {
                let chosen = policy.cards(*player, cards, *min, *max);
                let mut out: Vec<Action> = chosen
                    .iter()
                    .filter_map(|id| cards.iter().position(|c| c == id))
                    .map(Action::Pick)
                    .collect();
                if out.len() < usize::from(*max) {
                    out.push(Action::Finish);
                }
                out
            }
            Message::SelectUnselectCard {
                player,
                finishable,
                cancelable,
                select,
                unselect,
                ..
            } => match policy.unselect(
                *player,
                select.len() + unselect.len(),
                *finishable,
                *cancelable,
            ) {
                Some(i) => vec![Action::Pick(i)],
                None if *finishable => vec![Action::Finish],
                None => vec![Action::Cancel],
            },
            Message::SelectChain {
                player,
                chains,
                forced,
                ..
            } => match policy.chain(*player, chains.len(), *forced) {
                Some(i) => vec![Action::Chain(i)],
                None => vec![Action::Decline],
            },
            Message::SelectPlace { player, flag, .. } => {
                let (p, loc, seq) = policy.place(*player, *flag);
                let own = p == *player;
                let loc = if Driver::<RandomPolicy>::first_free_seat(*flag, loc, own).is_some() {
                    loc
                } else if loc == location::MZONE {
                    location::SZONE
                } else {
                    location::MZONE
                };
                let seq = Driver::<RandomPolicy>::first_free_seat(*flag, loc, own).unwrap_or(seq);
                vec![Action::Place {
                    player: p,
                    location: loc,
                    sequence: seq as u8,
                }]
            }
            Message::SelectPosition {
                player, positions, ..
            } => vec![Action::Position(policy.position(*player, *positions))],
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
                    set_monster_this_turn: turn_id % 2 == 1,
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
                let (kind, i) = policy.idle(&menu);
                vec![match kind {
                    0 => Action::Summon(i),
                    1 => Action::SpecialSummon(i),
                    2 => Action::Reposition(i),
                    3 => Action::SetMonster(i),
                    4 => Action::SetSpellTrap(i),
                    5 => Action::Activate(i),
                    6 => Action::ToBattlePhase,
                    _ => Action::ToEndPhase,
                }]
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
                let (kind, i) = policy.battle(&menu);
                vec![match kind {
                    0 => Action::Activate(i),
                    1 => Action::Attack(i),
                    2 => Action::ToMainPhase2,
                    _ => Action::ToEndPhase,
                }]
            }
            Message::SelectTribute {
                cards, min, max, ..
            } => {
                // The driver's greedy answer: offers in order until the
                // release values reach the minimum.
                let (min, max) = (u32::from(*min), usize::from(*max));
                let mut out = Vec::new();
                let mut value = 0u32;
                for (i, (_, release)) in cards.iter().enumerate() {
                    if value >= min || out.len() >= max {
                        break;
                    }
                    out.push(Action::Pick(i));
                    value += (*release).max(1);
                }
                if out.len() < max {
                    out.push(Action::Finish);
                }
                out
            }
            Message::AnnounceRace {
                player,
                count,
                available,
            } => {
                let picked = policy.announce_race(*player, *count, *available);
                (0..64u8)
                    .filter(|b| picked & (1u64 << b) != 0)
                    .map(Action::DeclareType)
                    .collect()
            }
            Message::Sort { player, cards, .. } => match policy.sort(*player, cards.len()) {
                None => vec![Action::Finish],
                Some(order) => {
                    // The driver's answer is each item's rank; ours is the
                    // items in order.
                    let mut by_rank: Vec<(i8, usize)> =
                        order.iter().enumerate().map(|(i, &r)| (r, i)).collect();
                    by_rank.sort();
                    by_rank.into_iter().map(|(_, i)| Action::Pick(i)).collect()
                }
            },
            other => panic!("not a player question: {other:?}"),
        }
    }

    /// **This layer replays the driver.** The same random pool games, once
    /// through `Driver<RandomPolicy>` in solver mode with chance sampled,
    /// once through `Game` answering every question with what the same
    /// policy would have answered (and sampling chance the same way): the
    /// rendered traces are identical to the line, and the games end the
    /// same way. Run without the forced-node skip, because the policy
    /// draws at some forced nodes.
    #[test]
    fn the_game_replays_the_random_driver_to_the_line() {
        let mut compared = 0;
        for game in 0..8u64 {
            let deck = pool_deck(10 + game);
            let mut driver = Driver::with_decks_and_seed(
                RandomPolicy::new(300 + game),
                [deck.clone(), deck.clone()],
                [4, 3, 2, game],
            )
            .tracing();
            driver.field.set_chance_mode(true);
            driver.sample_chance = true;
            let stop = driver.run(60_000);
            if stop == Stop::OutOfSteps {
                continue;
            }
            let mut policy = RandomPolicy::new(300 + game);
            let mut g = Game::configured(
                [deck.clone(), deck],
                [4, 3, 2, game],
                Config {
                    skip_forced: false,
                    tracing: true,
                    prune_cycles: false,
                    ..Config::default()
                },
            );
            for _ in 0..200_000 {
                match g.player_to_act() {
                    Actor::Terminal => break,
                    Actor::Chance => g.sample_chance().unwrap(),
                    Actor::Player(_) => {
                        let q = g.question().cloned().unwrap();
                        let turn = g.field().infos.turn_id;
                        for a in actions_for(&q, &mut policy, turn) {
                            g.apply(&a)
                                .unwrap_or_else(|e| panic!("game {game}: {a:?} for {q:?}: {e:?}"));
                        }
                    }
                }
            }
            assert!(g.is_terminal(), "game {game} ended in the driver: {stop:?}");
            let expected = match stop {
                Stop::Win { player, reason } if player > 1 => Cause::Draw { reason },
                Stop::Win { player, reason } => Cause::Win { player, reason },
                Stop::Finished => Cause::Finished,
                other => panic!("{other:?}"),
            };
            let cause = g.result().unwrap().cause;
            let same_end = match (expected, cause) {
                (Cause::Win { player: a, .. }, Cause::Win { player: b, .. }) => a == b,
                (a, b) => a == b,
            };
            assert!(same_end, "game {game}: {expected:?} vs {cause:?}");
            assert_eq!(
                driver.rendered_trace(),
                g.rendered_trace(),
                "game {game}: the traces"
            );
            assert!(driver.rendered_trace().lines().count() > 500);
            compared += 1;
        }
        assert!(compared >= 4, "{compared} games compared");
    }

    /// **The forced-node skip changes nothing but the number of nodes.**
    /// The same games, answered at every node with a choice by the same
    /// draws, once seeing the forced nodes and once not: identical traces,
    /// fewer nodes.
    #[test]
    fn skipping_forced_nodes_changes_only_the_node_count() {
        for game in 0..4u64 {
            let deck = pool_deck(20 + game);
            let mut traces = Vec::new();
            let mut nodes = Vec::new();
            for skip in [false, true] {
                let mut g = Game::configured(
                    [deck.clone(), deck.clone()],
                    [6, 6, 6, game],
                    Config {
                        skip_forced: skip,
                        tracing: true,
                        ..Config::default()
                    },
                );
                let mut rng = RandomPolicy::new(900 + game);
                let mut seen = 0u32;
                for _ in 0..3_000 {
                    match g.player_to_act() {
                        Actor::Terminal => break,
                        Actor::Chance => {
                            let outcomes = g.chance_outcomes();
                            assert!(!outcomes.is_empty(), "{:?}", g.question());
                            let pick = if outcomes.len() == 1 {
                                outcomes[0].0.clone()
                            } else {
                                outcomes[rng.below(outcomes.len())].0.clone()
                            };
                            g.apply(&pick).unwrap();
                        }
                        Actor::Player(_) => {
                            let actions = g.legal_actions();
                            assert!(!actions.is_empty(), "no action at {:?}", g.question());
                            let a = if actions.len() == 1 {
                                actions[0].clone()
                            } else {
                                seen += 1;
                                actions[rng.below(actions.len())].clone()
                            };
                            g.apply(&a).unwrap();
                        }
                    }
                    if seen >= 400 {
                        break;
                    }
                }
                traces.push(g.rendered_trace());
                nodes.push(g.steps);
            }
            assert_eq!(traces[0], traces[1], "game {game}");
            assert!(traces[0].lines().count() > 300);
        }
    }

    /// **A cancel back to the menu is not offered.** Random pool games
    /// with the pruning on: at every cancelable selection reached straight
    /// from a menu, `Cancel` is absent; the same games replayed action for
    /// action with the pruning off offer it there — so what is pruned is
    /// exactly the no-op, and the rest of the tree is untouched.
    #[test]
    fn a_cancel_back_to_the_menu_is_not_offered() {
        let mut pruned_nodes = 0usize;
        for game in 0..6u64 {
            let deck = pool_deck(30 + game);
            // No forced-node skipping on either side, so the two games
            // visit the same nodes: the pruning alone changes the lists.
            let mut on = Game::configured(
                [deck.clone(), deck.clone()],
                [8, 1, 8, game],
                Config {
                    skip_forced: false,
                    ..Config::default()
                },
            );
            let mut rng = RandomPolicy::new(1200 + game);
            // (node index, whether the node was a fresh cancelable selection)
            let mut taken: Vec<Action> = Vec::new();
            let mut fresh_cancelable: Vec<usize> = Vec::new();
            for _ in 0..2_500 {
                match on.player_to_act() {
                    Actor::Terminal => break,
                    Actor::Chance => {
                        let outcomes = on.chance_outcomes();
                        let pick = outcomes[rng.below(outcomes.len())].0.clone();
                        on.apply(&pick).unwrap();
                        taken.push(pick);
                    }
                    Actor::Player(_) => {
                        let actions = on.legal_actions();
                        let cancelable = match on.question() {
                            Some(Message::SelectCard { cancelable, .. })
                            | Some(Message::SelectTribute { cancelable, .. })
                            | Some(Message::SelectUnselectCard { cancelable, .. }) => *cancelable,
                            _ => false,
                        };
                        if cancelable && !on.changed_since_menu && actions.len() > 1 {
                            assert!(
                                !actions.contains(&Action::Cancel),
                                "pruned: {:?}",
                                on.question()
                            );
                            fresh_cancelable.push(taken.len());
                        }
                        let a = actions[rng.below(actions.len())].clone();
                        on.apply(&a).unwrap();
                        taken.push(a);
                    }
                }
            }
            if fresh_cancelable.is_empty() {
                continue;
            }
            // The same path with the pruning off offers the cancel there.
            let mut off = Game::configured(
                [deck.clone(), deck],
                [8, 1, 8, game],
                Config {
                    skip_forced: false,
                    prune_cycles: false,
                    ..Config::default()
                },
            );
            for (i, a) in taken.iter().enumerate() {
                if fresh_cancelable.contains(&i) {
                    assert!(
                        off.legal_actions().contains(&Action::Cancel),
                        "game {game} node {i}: unpruned, the cancel is there: {:?}",
                        off.question()
                    );
                    pruned_nodes += 1;
                }
                off.apply(a)
                    .unwrap_or_else(|e| panic!("game {game} node {i}: {a:?}: {e:?}"));
            }
        }
        assert!(pruned_nodes > 0, "some cancel was pruned");
    }

    /// **A game can be handed to another thread.** `Field` is `Send` —
    /// the parked continuations capture only values that cross threads,
    /// and a thread owns its field outright — so independent playouts run
    /// in parallel. Pinned by playing a game on each of four threads and
    /// by the compile-time bound below.
    #[test]
    fn a_game_can_be_played_on_another_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<Game>();
        assert_send::<Field>();
        let deck = pool_deck(40);
        let games: Vec<Game> = (0..4u64)
            .map(|i| Game::new([deck.clone(), deck.clone()], [7, 7, 7, i]))
            .collect();
        let results: Vec<u64> = std::thread::scope(|scope| {
            let handles: Vec<_> = games
                .into_iter()
                .enumerate()
                .map(|(i, mut g)| {
                    scope.spawn(move || {
                        let mut rng = RandomPolicy::new(1_500 + i as u64);
                        for _ in 0..2_000 {
                            match g.player_to_act() {
                                Actor::Terminal => break,
                                Actor::Chance => {
                                    let o = g.chance_outcomes();
                                    let a = o[rng.below(o.len())].0.clone();
                                    g.apply(&a).unwrap();
                                }
                                Actor::Player(_) => {
                                    let l = g.legal_actions();
                                    let a = l[rng.below(l.len())].clone();
                                    g.apply(&a).unwrap();
                                }
                            }
                        }
                        g.steps
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(results.iter().all(|&s| s > 100), "{results:?}");
    }

    /// A clone mid-selection carries the partial picks and the same future.
    #[test]
    fn a_clone_mid_selection_has_the_same_future() {
        let mut f = Field::new(8000);
        let cards: Vec<CardId> = (0..4).map(|i| hand_card(&mut f, 0, i)).collect();
        f.core.select_cards.clone_from(&cards);
        f.emplace(Kind::SelectCard {
            player: 0,
            cancelable: false,
            min: 2,
            max: 3,
        });
        let mut g = at(f);
        g.apply(&Action::Pick(1)).unwrap();
        let mut c = g.clone();
        assert_eq!(c.legal_actions(), g.legal_actions());
        g.apply(&Action::Pick(3)).unwrap();
        c.apply(&Action::Pick(3)).unwrap();
        g.apply(&Action::Finish).unwrap();
        c.apply(&Action::Finish).unwrap();
        assert_eq!(
            g.field().core.return_cards.list,
            c.field().core.return_cards.list
        );
        assert_eq!(g.rendered_trace(), c.rendered_trace());
    }

    #[test]
    fn combinations_are_ascending_and_complete() {
        assert_eq!(combinations(3, 2), vec![vec![0, 1], vec![0, 2], vec![1, 2]]);
        assert_eq!(combinations(4, 0), vec![Vec::<usize>::new()]);
        assert_eq!(combinations(5, 3).len(), 10);
    }

    /// A duel where the first player opens with Chaos Sorcerer in hand and
    /// a LIGHT and a DARK monster already in the graveyard, at the first
    /// idle command.
    fn sorcerer_duel(config: Config) -> Game {
        use crate::card::{attribute, card_type, status, Card, CardData};
        use crate::driver::{new_duel_with_lp, Driver, PassPolicy};
        let vanilla = Driver::<PassPolicy>::vanilla_data();
        let mut deck: Vec<CardData> = (0..40).map(|_| vanilla.clone()).collect();
        // The last card of the list is the first drawn (pseudo-shuffle).
        deck.push(
            crate::cards::card_data(crate::cards::chaos_sorcerer::CODE).expect("Chaos Sorcerer"),
        );
        let mut f = new_duel_with_lp([deck.clone(), deck], [1, 2, 3, 4], config.lp);
        for (seq, attr) in [(0, attribute::LIGHT), (1, attribute::DARK)] {
            let mut c = Card::with_data(
                CardData {
                    code: 6_000 + seq,
                    type_: card_type::MONSTER | card_type::NORMAL,
                    level: 4,
                    attack: 1200,
                    defense: 1000,
                    attribute: attr,
                    ..Default::default()
                },
                0,
            );
            c.current.controller = 0;
            c.set_status(status::EFFECT_ENABLED, true);
            let id = f.new_card(c);
            f.add_card(0, id, location::GRAVE, seq, false);
            f.cards[id].current.position = position::FACEUP;
        }
        let mut g = Game::from_field(f, config);
        while g.player_to_act() == Actor::Chance {
            g.sample_chance().unwrap();
        }
        assert!(
            matches!(g.question(), Some(Message::SelectIdleCmd { .. })),
            "{:?}",
            g.question()
        );
        g
    }

    /// **An unselect back to a selected node is not offered.** Chaos
    /// Sorcerer's cost selects a LIGHT and a DARK one at a time; after the
    /// first pick the reference offers the second card and the first back
    /// for unselection. Pruned, only the second card is offered (and the
    /// selection completes on the way under the forced-node skip);
    /// unpruned, the unselect is offered and returns to the node before
    /// the pick, key for key — the cycle the first Goat game by search
    /// stalled in.
    #[test]
    fn an_unselect_back_to_a_selected_node_is_not_offered() {
        for prune in [true, false] {
            let mut g = sorcerer_duel(Config {
                skip_forced: false,
                prune_cycles: prune,
                ..Config::default()
            });
            let sp = g
                .legal_actions()
                .into_iter()
                .find(|a| matches!(a, Action::SpecialSummon(_)))
                .expect("the Sorcerer is summonable");
            g.apply(&sp).unwrap();
            let Some(Message::SelectUnselectCard {
                select, unselect, ..
            }) = g.question()
            else {
                panic!("{:?}", g.question());
            };
            assert_eq!((select.len(), unselect.len()), (2, 0));
            let offered: Vec<u32> = select
                .iter()
                .map(|c| g.field().cards[*c].data.code)
                .collect();
            let first_legal = g.legal_actions();
            g.apply(&Action::Pick(0)).unwrap();
            let Some(Message::SelectUnselectCard {
                select, unselect, ..
            }) = g.question()
            else {
                panic!("{:?}", g.question());
            };
            assert_eq!((select.len(), unselect.len()), (1, 1));
            let legal = g.legal_actions();
            if prune {
                assert_eq!(legal, vec![Action::Pick(0)], "the unselect is not listed");
            } else {
                assert_eq!(legal, vec![Action::Pick(0), Action::Pick(1)]);
                g.apply(&Action::Pick(1)).unwrap();
                // Back where it started: the same two cards offered, none
                // selected, cancelable again, the same legal actions. (The
                // key is not compared: the observation carries the
                // processor's skeleton, and the select/unselect unit's
                // round counter has moved on — a transposition miss, not
                // a difference in the game.)
                let Some(Message::SelectUnselectCard {
                    select,
                    unselect,
                    cancelable,
                    ..
                }) = g.question()
                else {
                    panic!("{:?}", g.question());
                };
                let mut again: Vec<u32> = select
                    .iter()
                    .map(|c| g.field().cards[*c].data.code)
                    .collect();
                let mut first = offered.clone();
                again.sort_unstable();
                first.sort_unstable();
                assert_eq!((again, unselect.len(), *cancelable), (first, 0, true));
                assert_eq!(g.legal_actions(), first_legal, "the same choices again");
            }
        }
        // Under the forced-node skip the pruned second pick is answered on
        // the way: one pick completes the cost.
        let mut g = sorcerer_duel(Config::default());
        let sp = g
            .legal_actions()
            .into_iter()
            .find(|a| matches!(a, Action::SpecialSummon(_)))
            .unwrap();
        g.apply(&sp).unwrap();
        g.apply(&Action::Pick(0)).unwrap();
        assert!(
            !matches!(g.question(), Some(Message::SelectUnselectCard { .. })),
            "{:?}",
            g.question()
        );
    }
}

#[cfg(test)]
mod observation_tests {
    use super::*;
    use crate::board::position;
    use crate::card::{card_type, status, Card};
    use crate::cards::{card_data, POOL};
    use crate::driver::RandomPolicy;
    use crate::observation::{HiddenPile, Place};
    use crate::processor::Kind;
    use std::collections::HashMap;

    fn pool_deck(game: u64) -> Vec<CardData> {
        let mut rng = RandomPolicy::new(8000 + game);
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

    fn card(f: &mut Field, owner: u8, code_: u32, loc: u8, seq: u32, pos: u8) -> CardId {
        let mut c = Card::with_data(
            CardData {
                code: code_,
                type_: card_type::MONSTER | card_type::NORMAL,
                level: 4,
                attack: 1000,
                defense: 1000,
                ..Default::default()
            },
            owner,
        );
        c.current.controller = owner;
        c.current.position = pos;
        c.set_status(status::EFFECT_ENABLED, true);
        let id = f.new_card(c);
        f.add_card(owner, id, loc, seq, false);
        f.cards[id].current.position = pos;
        id
    }

    /// A field with a yes/no pending for player 0, hands as given.
    fn asked(hands: [&[u32]; 2]) -> (Field, Vec<CardId>, Vec<CardId>) {
        let mut f = Field::new(8000);
        let h0: Vec<CardId> = hands[0]
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                card(
                    &mut f,
                    0,
                    c,
                    location::HAND,
                    i as u32,
                    position::FACEDOWN_DEFENSE,
                )
            })
            .collect();
        let h1: Vec<CardId> = hands[1]
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                card(
                    &mut f,
                    1,
                    c,
                    location::HAND,
                    i as u32,
                    position::FACEDOWN_DEFENSE,
                )
            })
            .collect();
        f.emplace(Kind::SelectYesNo {
            player: 0,
            description: 0,
        });
        (f, h0, h1)
    }

    fn game(f: Field) -> Game {
        Game::from_field(
            f,
            Config {
                skip_forced: false,
                ..Config::default()
            },
        )
    }

    /// **The other hand is hidden; the own hand is not.** Two positions
    /// that differ only in the opponent's hand key the same for the
    /// viewer and differently for the opponent; the reverse for the
    /// viewer's own hand.
    #[test]
    fn the_other_hand_is_hidden_and_the_own_hand_is_not() {
        let (a, _, _) = asked([&[11, 12], &[21, 22]]);
        let (b, _, _) = asked([&[11, 12], &[21, 23]]);
        let (c, _, _) = asked([&[11, 13], &[21, 22]]);
        let (mut a, mut b, mut c) = (game(a), game(b), game(c));
        assert_eq!(a.player_to_act(), Actor::Player(0));
        assert_eq!(
            a.infoset_key(0),
            b.infoset_key(0),
            "the opponent's hand is not seen"
        );
        assert_ne!(a.infoset_key(1), b.infoset_key(1), "the owner sees it");
        assert_ne!(a.infoset_key(0), c.infoset_key(0), "the own hand is seen");
        assert_eq!(a.infoset_key(1), c.infoset_key(1));
        assert_eq!(a.public_key(), b.public_key(), "neither hand is public");
        assert_eq!(a.public_key(), c.public_key());
        let obs = a.observation(0);
        assert_eq!(obs.private.hand, vec![11, 12]);
        for p in 0..2 {
            assert_eq!(
                obs.public.players[p].hand,
                HiddenPile {
                    known: vec![],
                    unknown: 2
                },
                "no hand is public"
            );
        }
    }

    /// **A reveal becomes common knowledge.** Shown one of the opponent's
    /// hand cards, the viewer knows it — and the opponent knows the viewer
    /// knows, so the card's code is public knowledge from then on: both
    /// keys change, the public key with them, and the card leaves the
    /// viewer's private knowledge for the public pile.
    #[test]
    fn a_reveal_enters_the_viewers_knowledge() {
        let (plain, _, _) = asked([&[11], &[21, 22]]);
        let (mut shown, _, h1) = asked([&[11], &[21, 22]]);
        assert!(crate::script_api::confirm_cards(&mut shown, 0, vec![h1[1]]));
        let (mut plain, mut shown) = (game(plain), game(shown));
        assert_ne!(
            plain.infoset_key(0),
            shown.infoset_key(0),
            "the viewer learned"
        );
        assert_ne!(
            plain.infoset_key(1),
            shown.infoset_key(1),
            "and the owner saw them learn"
        );
        assert_ne!(plain.public_key(), shown.public_key());
        let obs = shown.observation(0);
        assert_eq!(
            obs.public.players[1].hand,
            HiddenPile {
                known: vec![22],
                unknown: 1
            },
            "common knowledge"
        );
        assert!(
            obs.private.known.is_empty(),
            "nothing the viewer knows alone"
        );
        assert!(shown.knowledge().knows(0, h1[1]));
        assert!(!shown.knowledge().knows(0, h1[0]));
    }

    /// **A card that was public stays known face-down.** Flipped face-down
    /// after being seen, its code is still in the viewer's observation; a
    /// card set face-down from the start is not.
    #[test]
    fn a_public_card_stays_known_when_it_goes_face_down() {
        let mut f = Field::new(8000);
        let seen = card(&mut f, 1, 31, location::MZONE, 0, position::FACEUP_ATTACK);
        f.emplace(Kind::SelectYesNo {
            player: 0,
            description: 0,
        });
        let mut g = game(f);
        assert!(g.knowledge().knows(0, seen));
        g.field.cards[seen].current.position = position::FACEDOWN_DEFENSE;
        g.knowledge.observe(&g.field, &[], &[]);
        let view = g.observation(0).public.players[1].mzone[0].clone().unwrap();
        assert_eq!(view.code, Some(31), "still known");
        assert_eq!(view.position, position::FACEDOWN_DEFENSE);
        assert!(view.stats.is_none(), "but no stats face-down");

        let mut f = Field::new(8000);
        card(
            &mut f,
            1,
            31,
            location::MZONE,
            0,
            position::FACEDOWN_DEFENSE,
        );
        f.emplace(Kind::SelectYesNo {
            player: 0,
            description: 0,
        });
        let mut h = game(f);
        let view = h.observation(0).public.players[1].mzone[0].clone().unwrap();
        assert_eq!(view.code, None, "never seen");
        let own = h.observation(1);
        assert_eq!(
            own.public.players[1].mzone[0].clone().unwrap().code,
            None,
            "not public"
        );
        assert_eq!(
            own.private.known,
            vec![(
                Place {
                    controller: 1,
                    location: location::MZONE,
                    sequence: 0
                },
                31
            )],
            "the controller knows their own set card"
        );
    }

    /// A card returned to the deck is forgotten: the next look at it is a
    /// chance node.
    #[test]
    fn a_card_entering_the_deck_is_forgotten() {
        let mut f = Field::new(8000);
        let c = card(&mut f, 1, 41, location::GRAVE, 0, position::FACEUP_ATTACK);
        f.emplace(Kind::SelectYesNo {
            player: 0,
            description: 0,
        });
        let mut g = game(f);
        assert!(g.knowledge().knows(0, c) && g.knowledge().knows(1, c));
        g.field.remove_card(c);
        g.field.add_card(1, c, location::DECK, 0, false);
        g.knowledge.observe(&g.field, &[], &[]);
        assert!(!g.knowledge().knows(0, c));
        assert!(!g.knowledge().knows(1, c));
    }

    /// **A card shown in a deck stays unknown.** Magical Merchant with
    /// nothing to find shows the whole deck to both players and shuffles
    /// it: the identities were the decklist's already and the order is
    /// hidden again, so nothing may enter either viewer's knowledge or
    /// private list — reversing the deck must not move a key.
    #[test]
    fn a_card_shown_in_a_deck_stays_unknown() {
        let mut f = Field::new(8000);
        let deck: Vec<CardId> = (0..4)
            .map(|i| {
                card(
                    &mut f,
                    1,
                    51 + i,
                    location::DECK,
                    i,
                    position::FACEDOWN_DEFENSE,
                )
            })
            .collect();
        card(&mut f, 0, 11, location::HAND, 0, position::FACEDOWN_DEFENSE);
        f.emplace(Kind::SelectYesNo {
            player: 0,
            description: 0,
        });
        let mut g = game(f);
        // Shown to both, as `confirm_deck_top` records it.
        let shown: Vec<(u8, CardId)> = deck.iter().flat_map(|&c| [(0, c), (1, c)]).collect();
        g.knowledge.observe(&g.field, &shown, &[]);
        assert!(deck
            .iter()
            .all(|&c| !g.knowledge().knows(0, c) && !g.knowledge().knows(1, c)));
        let before = [g.infoset_key(0), g.infoset_key(1)];
        assert!(g.observation(0).private.known.is_empty());
        let mut order = g.field.players[1].main.clone();
        order.reverse();
        assert!(g.field_mut().set_deck_order(1, &order));
        assert_eq!([g.infoset_key(0), g.infoset_key(1)], before);
    }

    /// **An effect belonging to a card in a deck is placed in the deck,
    /// not at a depth in it.** Sinister Serpent's End Phase effect is a
    /// duel-level effect whose handler is the Serpent, wherever it is; when
    /// it is in the deck, the effect's place in the public layer must not
    /// carry the deck position — reversing the deck must not move a key.
    #[test]
    fn an_effect_on_a_deck_card_carries_no_deck_position() {
        use crate::effect::{effect_type, flag, Effect};
        let mut f = Field::new(8000);
        let deck: Vec<CardId> = (0..4)
            .map(|i| {
                card(
                    &mut f,
                    1,
                    51 + i,
                    location::DECK,
                    i,
                    position::FACEDOWN_DEFENSE,
                )
            })
            .collect();
        card(&mut f, 0, 11, location::HAND, 0, position::FACEDOWN_DEFENSE);
        // A granted duel-level effect on the second deck card, as a script
        // registers one with `Duel.RegisterEffect`.
        let mut e = Effect::new(effect_type::FIELD | effect_type::CONTINUOUS, 0x1200);
        e.owner = Some(deck[1]);
        e.handler = Some(deck[1]);
        e.flag[0] |= flag::FIELD_ONLY;
        let e = f.new_effect(e);
        f.field_effects.indexer.insert(e);
        f.emplace(Kind::SelectYesNo {
            player: 0,
            description: 0,
        });
        let mut g = game(f);
        let obs = g.observation(0);
        let placed: Vec<_> = obs
            .public
            .effects
            .iter()
            .filter(|v| v.code == 0x1200)
            .map(|v| v.handler)
            .collect();
        assert_eq!(
            placed,
            vec![Some(crate::observation::Place {
                controller: 1,
                location: location::DECK,
                sequence: 0
            })],
            "in the deck, at no depth"
        );
        let before = [g.infoset_key(0), g.infoset_key(1)];
        let mut order = g.field.players[1].main.clone();
        order.reverse();
        assert!(g.field_mut().set_deck_order(1, &order));
        assert_eq!([g.infoset_key(0), g.infoset_key(1)], before);
    }

    /// A hand of three shown to the viewer, one of which then leaves for
    /// a hidden place.
    fn shown_hand_then_a_set(which: usize, to: u8) -> (Game, Vec<CardId>) {
        let mut f = Field::new(8000);
        let hand: Vec<CardId> = (0..3)
            .map(|i| {
                card(
                    &mut f,
                    1,
                    61 + i,
                    location::HAND,
                    i,
                    position::FACEDOWN_DEFENSE,
                )
            })
            .collect();
        card(&mut f, 0, 11, location::HAND, 0, position::FACEDOWN_DEFENSE);
        assert!(crate::script_api::confirm_cards(&mut f, 0, hand.clone()));
        f.emplace(Kind::SelectYesNo {
            player: 0,
            description: 0,
        });
        let mut g = game(f);
        assert!(
            hand.iter().all(|&c| g.knowledge().known_to_both(c)),
            "shown"
        );
        let c = hand[which];
        g.field_mut().remove_card(c);
        g.field_mut().add_card(1, c, to, 2, false);
        g.field_mut().cards[c].current.position = position::FACEDOWN_DEFENSE;
        g.refresh_knowledge();
        (g, hand)
    }

    /// **A set from a shown hand is ambiguous.** Shown three cards, the
    /// viewer cannot tell which one was set: the set card and the two
    /// still in hand are all forgotten, and two histories that set a
    /// different one of the three key the same for the viewer — while
    /// the owner, who can tell, keys them apart.
    #[test]
    fn a_set_from_a_shown_hand_is_ambiguous() {
        let (mut a, hand) = shown_hand_then_a_set(1, location::SZONE);
        let (mut b, _) = shown_hand_then_a_set(2, location::SZONE);
        for &c in &hand {
            assert!(!a.knowledge().knows(0, c), "the viewer lost {c}");
            assert!(a.knowledge().knows(1, c), "the owner did not");
        }
        let obs = a.observation(0);
        assert_eq!(
            obs.public.players[1].hand,
            HiddenPile {
                known: vec![],
                unknown: 2
            }
        );
        assert_eq!(obs.public.players[1].szone[2].as_ref().unwrap().code, None);
        assert!(obs.private.known.is_empty());
        assert_eq!(
            a.infoset_key(0),
            b.infoset_key(0),
            "one of three, either way"
        );
        assert_ne!(a.infoset_key(1), b.infoset_key(1), "the owner knows which");
    }

    /// **A pending operation's card is the chooser's to know.** Player 0
    /// is setting one of two different hand cards and is asked where: at the
    /// zone question the card is still in hand, and the queued set and move
    /// carry it. The two histories
    /// differ only in which card the queued set carries: the setter, who
    /// chose it, keys them apart; the opponent and the public skeleton,
    /// which see a card leave a hand of two either way, key them the same.
    #[test]
    fn a_pending_set_keys_by_its_card_for_the_setter_only() {
        let setting = |which: usize| {
            let mut f = Field::new(8000);
            let hand: Vec<CardId> = [11, 12]
                .iter()
                .enumerate()
                .map(|(i, &c)| {
                    card(
                        &mut f,
                        0,
                        c,
                        location::HAND,
                        i as u32,
                        position::FACEDOWN_DEFENSE,
                    )
                })
                .collect();
            card(&mut f, 1, 21, location::HAND, 0, position::FACEDOWN_DEFENSE);
            f.emplace_at(
                Kind::MonsterSet {
                    setplayer: 0,
                    target: hand[which],
                    ignore_count: false,
                    min_tribute: 0,
                    zone: 0x1f,
                    state: Box::default(),
                },
                1,
            );
            f.emplace(Kind::SelectYesNo {
                player: 0,
                description: 0,
            });
            game(f)
        };
        let (mut a, mut b) = (setting(0), setting(1));
        assert_eq!(a.player_to_act(), Actor::Player(0));
        assert_ne!(
            a.infoset_key(0),
            b.infoset_key(0),
            "the setter knows which card it is setting"
        );
        assert_eq!(a.infoset_key(1), b.infoset_key(1), "the opponent does not");
        assert_eq!(a.public_key(), b.public_key(), "nor does the public");
        let (oa, ob) = (a.observation(0), b.observation(0));
        assert_eq!(oa.public.queue, ob.public.queue);
        assert!(
            oa.public.queue.iter().any(|(_, _, at)| at.is_some()),
            "the skeleton says where the subject is"
        );
        assert!(matches!(
            a.question(),
            Some(Message::SelectPlace { player: 0, .. })
        ));
        let codes = |o: &crate::observation::Observation| -> Vec<u32> {
            o.private.subjects.iter().map(|&(_, code)| code).collect()
        };
        assert_eq!(codes(&oa), vec![11, 11], "the queued set and move");
        assert_eq!(codes(&ob), vec![12, 12]);
        assert!(a.observation(1).private.subjects.is_empty());
    }

    /// **A set from a hand of one code stays known.** Two copies of the
    /// same card shown: whichever was set, the viewer can name it.
    #[test]
    fn a_set_from_a_hand_of_one_code_stays_known() {
        let mut f = Field::new(8000);
        let hand: Vec<CardId> = (0..2)
            .map(|i| card(&mut f, 1, 70, location::HAND, i, position::FACEDOWN_DEFENSE))
            .collect();
        card(&mut f, 0, 11, location::HAND, 0, position::FACEDOWN_DEFENSE);
        assert!(crate::script_api::confirm_cards(&mut f, 0, hand.clone()));
        f.emplace(Kind::SelectYesNo {
            player: 0,
            description: 0,
        });
        let mut g = game(f);
        g.field_mut().remove_card(hand[0]);
        g.field_mut()
            .add_card(1, hand[0], location::MZONE, 1, false);
        g.field_mut().cards[hand[0]].current.position = position::FACEDOWN_DEFENSE;
        g.refresh_knowledge();
        assert!(hand.iter().all(|&c| g.knowledge().knows(0, c)));
        let obs = g.observation(0);
        assert_eq!(
            obs.public.players[1].mzone[1].as_ref().unwrap().code,
            Some(70)
        );
        assert_eq!(
            obs.public.players[1].hand,
            HiddenPile {
                known: vec![70],
                unknown: 0
            }
        );
    }

    /// **A departure the viewer chose is not ambiguous.** Trap Dustshoot:
    /// the viewer picks which shown card returns to the deck, so the two
    /// that stay are still known to them.
    #[test]
    fn an_identified_departure_keeps_the_rest_known() {
        let mut f = Field::new(8000);
        let hand: Vec<CardId> = (0..3)
            .map(|i| {
                card(
                    &mut f,
                    1,
                    61 + i,
                    location::HAND,
                    i,
                    position::FACEDOWN_DEFENSE,
                )
            })
            .collect();
        card(&mut f, 0, 11, location::HAND, 0, position::FACEDOWN_DEFENSE);
        assert!(crate::script_api::confirm_cards(&mut f, 0, hand.clone()));
        f.emplace(Kind::SelectYesNo {
            player: 0,
            description: 0,
        });
        let mut g = game(f);
        g.field_mut().remove_card(hand[1]);
        g.field_mut().add_card(1, hand[1], location::DECK, 0, false);
        g.knowledge.observe(&g.field, &[], &[(0, hand[1])]);
        assert!(!g.knowledge().knows(0, hand[1]), "in the deck");
        assert!(
            g.knowledge().knows(0, hand[0]) && g.knowledge().knows(0, hand[2]),
            "the rest"
        );
        assert_eq!(
            g.observation(0).public.players[1].hand,
            HiddenPile {
                known: vec![61, 63],
                unknown: 0
            }
        );
    }

    /// **The deck's order and the generator are hidden.** Reordering a
    /// deck, or advancing the generator, changes no key.
    #[test]
    fn the_deck_order_and_the_generator_are_hidden() {
        let deck = pool_deck(0);
        let mut g = Game::new([deck.clone(), deck], [3, 1, 4, 1]);
        while g.player_to_act() == Actor::Chance {
            g.sample_chance().unwrap();
        }
        let before = [g.infoset_key(0), g.infoset_key(1), g.public_key()];
        let mut order = g.field.players[1].main.clone();
        order.reverse();
        assert!(g.field.set_deck_order(1, &order));
        let _ = g.field.rng.next_integer(0, 100);
        assert_eq!([g.infoset_key(0), g.infoset_key(1), g.public_key()], before);
        // And a clone keys the same.
        let mut c = g.clone();
        assert_eq!(c.infoset_key(0), before[0]);
    }

    /// **Equal keys offer equal actions** — the canary for a projection
    /// that left something out. Over random pool games, every player node
    /// is keyed for the actor and the legal actions recorded; a repeat of
    /// a key must repeat the actions. Each game is played twice with the
    /// same chance draws and different choices, so the shared opening
    /// recurs and the comparison is known to run.
    #[test]
    fn equal_keys_offer_equal_actions() {
        let mut seen: HashMap<u64, Vec<Action>> = HashMap::new();
        let mut nodes = 0usize;
        let mut repeats = 0usize;
        for game in 0..6u64 {
            let deck = pool_deck(game);
            for pass in 0..2u64 {
                let mut g = Game::new([deck.clone(), deck.clone()], [2, 7, 1, game]);
                let mut chance = RandomPolicy::new(600 + game);
                let mut choice = RandomPolicy::new(700 + 10 * game + pass);
                for _ in 0..3_000 {
                    match g.player_to_act() {
                        Actor::Terminal => break,
                        Actor::Chance => {
                            let outcomes = g.chance_outcomes();
                            let pick = outcomes[chance.below(outcomes.len())].0.clone();
                            g.apply(&pick).unwrap();
                        }
                        Actor::Player(p) => {
                            let actions = g.legal_actions();
                            let key = g.infoset_key(p);
                            nodes += 1;
                            match seen.get(&key) {
                                Some(prev) => {
                                    repeats += 1;
                                    assert_eq!(prev, &actions, "game {game}: {:?}", g.question());
                                }
                                None => {
                                    seen.insert(key, actions.clone());
                                }
                            }
                            let a = actions[choice.below(actions.len())].clone();
                            g.apply(&a).unwrap();
                        }
                    }
                }
            }
        }
        assert!(nodes > 1_000, "{nodes}");
        assert!(repeats > 0, "some infoset recurred");
    }

    /// The question is part of the key: the same board with a different
    /// question keys differently, and a partial pick changes the key.
    #[test]
    fn the_question_and_the_partial_picks_are_in_the_key() {
        let (a, _, _) = asked([&[11, 12], &[21]]);
        let mut a = game(a);
        let mut f = Field::new(8000);
        let cards: Vec<CardId> = (0..2)
            .map(|i| {
                card(
                    &mut f,
                    0,
                    11 + i,
                    location::HAND,
                    i,
                    position::FACEDOWN_DEFENSE,
                )
            })
            .collect();
        card(&mut f, 1, 21, location::HAND, 0, position::FACEDOWN_DEFENSE);
        f.core.select_cards.clone_from(&cards);
        f.emplace(Kind::SelectCard {
            player: 0,
            cancelable: false,
            min: 1,
            max: 2,
        });
        let mut b = game(f);
        assert_ne!(a.infoset_key(0), b.infoset_key(0), "a different question");
        let before = b.infoset_key(0);
        b.apply(&Action::Pick(0)).unwrap();
        assert_ne!(b.infoset_key(0), before, "a pick made");
    }
}
