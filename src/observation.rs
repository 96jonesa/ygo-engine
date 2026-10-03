//! What a player can see: the viewer's projection of a duel, and the
//! information-set key hashed from it.
//!
//! ## The rule
//!
//! An information-set key must be a function of the viewer's observations
//! and of nothing else. Two positions the viewer cannot tell apart must
//! key the same, or a solver learns a strategy that conditions on what the
//! player could not know and is not implementable at the table. Two
//! positions the viewer *can* tell apart may key the same only at the cost
//! of conflating them — a weaker solution, not a wrong one. So the
//! projection here is **curated**: every field of it is something a
//! player at the table sees, and nothing internal that merely correlates
//! with hidden facts is admitted. Field ids and effect ids are the
//! example: the counter advances when a face-down card registers its
//! effects, and how many it registers depends on which card it is.
//!
//! ## Knowledge
//!
//! ocgcore does not track who has seen what — its message stream is the
//! omniscient one, and a client filters it per player — so the knowledge
//! model lives beside the field, in [`Knowledge`], maintained by the
//! [`crate::game::Game`] after every processor step. A card becomes known
//! to both players the moment it is public (face-up on the field, in a
//! graveyard, banished face-up), to its controller when it is in their
//! hand or face-down on their side, and to one viewer when a confirm or
//! reveal is addressed to them (`Core::revealed`). Knowledge persists
//! while the card can be followed: a card flipped face-down stays known,
//! a spirit returned to the hand stays known. It ends when the card
//! enters a deck, because the shuffle destroys its position and the next
//! draw is a chance node — and it ends for a whole hand when a card
//! leaves that hand for a hidden place and the other player cannot tell
//! which card left (see `Knowledge::observe`, "ambiguous departures"):
//! a flag per card cannot say "one of these three", and claiming to know
//! which of a shown hand was set is the error in the other direction,
//! a key finer than what the player can see. A card the viewer chose
//! themselves is identified and never ambiguous.

//!
//! The opponent's hand is therefore a **multiset**, not a row of slots:
//! the known cards' codes plus a count of unknown ones. Nothing strategic
//! depends on hand order, and a hand shuffle changes slots but not
//! identities. A deck needs no contents at all — the deck plus the unknown
//! hand cards together are the decklist minus everything seen, which the
//! rest of the observation already fixes — so a deck is a size.
//!
//! ## What is projected
//!
//! The board (each seat: position, and for a public or known card its
//! code; for a face-up card its effective stats, counters, equip target,
//! summon type, and the public part of its status word), the hands under
//! the knowledge model, the graveyards and banished cards in order, life
//! points, the turn and phase, the chain, the battle in progress, the
//! summon counts, the zone masks, the public effect layer (every granted
//! effect, and every printed effect of a public card), the processor's
//! control-flow skeleton (each queued unit's kind and step, never its
//! payload), and the pending question with the viewer's partial picks.
//!
//! What can go wrong is incompleteness — a state fact left out conflates
//! two positions. The canary is `game::tests::equal_keys_offer_equal_actions`:
//! over random games, nodes with equal keys must offer equal legal
//! actions.

use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};

use crate::board::{location, position};
use crate::card::{status, Card};
use crate::effect::flag;
use crate::event::CardId;
use crate::field::{Field, Message};
use crate::fxhash::FxHasher;

/// Which hidden cards each viewer knows the identity of. See the module
/// note.
///
/// Per card, a pair of bits; and per card the placement last seen, so a
/// processor step that moved nothing costs one comparison per card —
/// this runs after every step, and a scan with set operations per card
/// was a tenth of the engine's whole throughput.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Knowledge {
    /// Bit 0: known to player 0; bit 1: known to player 1.
    known: Vec<u8>,
    /// `(location, position, controller)` at the last look.
    seen: Vec<(u8, u8, u8)>,
}

impl Knowledge {
    pub fn knows(&self, viewer: u8, card: CardId) -> bool {
        self.known.get(card).is_some_and(|k| k & (1 << viewer) != 0)
    }

    /// Known to both players: public knowledge of a hidden card.
    pub fn known_to_both(&self, card: CardId) -> bool {
        self.known.get(card).is_some_and(|k| *k == 0b11)
    }

    /// Bring the knowledge up to date with the field: called after every
    /// processor step, with the confirms and reveals that step made and
    /// the cards a viewer identified by choosing them (`identified`).
    pub fn observe(&mut self, f: &Field, revealed: &[(u8, CardId)], identified: &[(u8, CardId)]) {
        if self.known.len() < f.cards.len() {
            self.known.resize(f.cards.len(), 0);
            self.seen.resize(f.cards.len(), (0xff, 0xff, 0xff));
        }
        // Pass one: what moved, and which hands lost a card to a hidden
        // place.
        let mut moved: Vec<CardId> = Vec::new();
        let mut departures: Vec<(u8, CardId)> = Vec::new();
        for (id, c) in f.cards.iter().enumerate() {
            let now = (c.current.location, c.current.position, c.current.controller);
            if self.seen[id] == now {
                continue;
            }
            moved.push(id);
            let (was_loc, _, was_ctrl) = self.seen[id];
            if was_loc == location::HAND && was_ctrl < 2 && !is_public(c) {
                departures.push((was_ctrl, id));
            }
        }
        // **Ambiguous departures.** A card leaving a hand for a hidden
        // place — set face-down, returned to the deck — is followed by
        // the other player only if they can tell which card left: every
        // card of that hand was known to them and of one code, or they
        // chose the card themselves. Otherwise the hand's cards are all
        // "one of these" from now on, which a flag per card cannot say,
        // and the sound thing is to forget them all — the one that left
        // and the ones that stayed. Trap Dustshoot's return is chosen by
        // the viewer, so it stays exact; a set from a hand they were
        // shown does not.
        for hand_of in 0..2u8 {
            let left: Vec<CardId> = departures
                .iter()
                .filter(|d| d.0 == hand_of)
                .map(|d| d.1)
                .collect();
            if left.is_empty() {
                continue;
            }
            let was_in_hand: Vec<CardId> = (0..f.cards.len())
                .filter(|&c| self.seen[c].0 == location::HAND && self.seen[c].2 == hand_of)
                .collect();
            for viewer in 0..2u8 {
                if viewer == hand_of {
                    continue;
                }
                if left.iter().all(|&c| identified.contains(&(viewer, c))) {
                    continue;
                }
                let mut codes: Vec<u32> = Vec::new();
                let mut all_known = true;
                for &c in &was_in_hand {
                    if self.knows(viewer, c) {
                        codes.push(f.cards[c].data.code);
                    } else {
                        all_known = false;
                    }
                }
                codes.sort_unstable();
                codes.dedup();
                if all_known && codes.len() <= 1 {
                    continue;
                }
                for &c in &was_in_hand {
                    self.known[c] &= !(1 << viewer);
                }
            }
        }
        // Pass two: the placement rules for what moved.
        let own = location::HAND | location::MZONE | location::SZONE | location::EXTRA;
        for id in moved {
            let c = &f.cards[id];
            let now = (c.current.location, c.current.position, c.current.controller);
            self.seen[id] = now;
            let loc = now.0;
            if loc == location::DECK || loc == 0 {
                self.known[id] = 0;
            } else if is_public(c) {
                self.known[id] = 0b11;
            } else if loc & own != 0 && now.2 < 2 {
                self.known[id] |= 1 << now.2;
            }
        }

        // A card shown while it sits in a deck cannot be followed: the
        // deck's order is hidden and the next shuffle or look is a chance
        // node — Magical Merchant shows a whole deck and shuffles it. What
        // is shown and then moves is caught by the move; a card shown and
        // then drawn is confirmed again by the showing site.
        for &(player, card) in revealed {
            if player < 2
                && card < self.known.len()
                && f.cards[card].current.location != location::DECK
            {
                self.known[card] |= 1 << player;
            }
        }
    }
}

/// Public to both players where it is.
fn is_public(c: &Card) -> bool {
    match c.current.location {
        location::GRAVE => true,
        location::MZONE | location::SZONE | location::REMOVED => {
            c.current.position & position::FACEUP != 0
        }
        _ => false,
    }
}

/// Where a card is, as everyone sees it. In a hidden pile — a deck, a
/// hand, an Extra Deck — the position is not part of what anyone sees
/// (a deck's order is the hidden variable, a hand is a multiset), so the
/// sequence is reported as zero there. A duel-level effect that belongs
/// to a card in a deck (Sinister Serpent's End Phase effect) is thereby
/// placed in that deck, not at a depth in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Place {
    pub controller: u8,
    pub location: u8,
    pub sequence: u32,
}

/// The piles whose order is not observable.
const HIDDEN_PILES: u8 = location::DECK | location::HAND | location::EXTRA;

fn place_of(f: &Field, c: CardId) -> Place {
    let cur = &f.cards[c].current;
    Place {
        controller: cur.controller,
        location: cur.location,
        sequence: if cur.location & HIDDEN_PILES != 0 {
            0
        } else {
            cur.sequence
        },
    }
}

/// A card named in a question: where it is, and what it is if the viewer
/// knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CardRef {
    pub code: Option<u32>,
    pub place: Place,
}

/// A face-up card's effective numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Stats {
    pub attack: i32,
    pub defense: i32,
    pub level: u32,
}

/// One seat of the field, or one card of a public pile.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CardView {
    /// `None` for a face-down card that is not public knowledge; a viewer
    /// who knows it alone has it in [`Private::known`].
    pub code: Option<u32>,
    pub position: u8,
    /// Public with the code.
    pub owner: Option<u8>,
    /// Face-up on the field only.
    pub stats: Option<Stats>,
    /// The public part of the status word.
    pub status: u32,
    /// `(counter type, count from player 0, count from player 1)`.
    pub counters: Vec<(u16, u16, u16)>,
    pub equip_target: Option<Place>,
    pub summon_type: u32,
    /// The turn the card arrived where it is.
    pub turn: i16,
    pub attacks_announced: u16,
    pub attacked: u16,
}

/// A hidden pile as both players see it: the publicly known cards' codes,
/// sorted, and how many are not.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HiddenPile {
    pub known: Vec<u32>,
    pub unknown: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlayerView {
    pub lp: i32,
    pub deck: usize,
    pub hand: HiddenPile,
    pub mzone: Vec<Option<CardView>>,
    pub szone: Vec<Option<CardView>>,
    /// In pile order, oldest first.
    pub grave: Vec<CardView>,
    pub removed: Vec<CardView>,
    pub extra: HiddenPile,
    pub used_location: u32,
    pub disabled_location: u32,
}

/// A granted effect, or a printed effect of a public card.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EffectView {
    pub code: u32,
    pub kind: u16,
    pub flags: [u32; 2],
    pub handler: Option<Place>,
    pub owner: Option<Place>,
    pub range: (u16, u16, u16),
    pub reset_flag: u32,
    pub reset_count: u8,
    pub count_limit: u8,
    pub description: u64,
    pub value: i64,
    pub label: Vec<i64>,
}

/// A link on the chain: an activation is public.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChainView {
    pub code: u32,
    pub description: u64,
    pub triggering_player: u8,
    pub triggering_controller: u8,
    pub triggering_location: u16,
    pub triggering_sequence: u32,
    pub triggering_position: u8,
    pub event_code: u32,
    pub event_player: u8,
    pub event_value: u32,
    pub targets: Vec<Place>,
    pub target_player: u8,
    pub target_param: i32,
    pub negated: bool,
    pub flag: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BattleView {
    pub attacker: Option<Place>,
    pub target: Option<Place>,
    pub chain_attack: bool,
    pub damage_calculated: bool,
    pub battle_damage: [u32; 2],
    pub attack_state_count: [u16; 2],
    pub battled_count: [u16; 2],
}

/// Everything both players see.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Public {
    pub turn: i16,
    pub turn_player: u8,
    pub phase: u16,
    pub phase_action: bool,
    pub players: [PlayerView; 2],
    pub chain: Vec<ChainView>,
    pub battle: BattleView,
    pub summon_count: [u32; 2],
    pub normal_summons: [u32; 2],
    pub flip_summons: [u32; 2],
    pub special_summons: [u32; 2],
    pub extra_summon: [bool; 2],
    /// The public effect layer, sorted.
    pub effects: Vec<EffectView>,
    /// The processor's control-flow skeleton: each queued unit's kind and
    /// step, front first, and where its subject card is (the card being
    /// summoned, set, moved, and so on: [`subject`]). The subject's
    /// identity is not here; it is in [`Private::subjects`] for a viewer
    /// who knows it.
    pub queue: Vec<(String, u16, Option<Place>)>,
}

/// What the viewer alone knows: their hand and Extra Deck, and every hidden
/// card elsewhere whose identity they know and the other player does not
/// — their own set cards, a card revealed to them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Private {
    pub viewer: u8,
    /// In hand order.
    pub hand: Vec<u32>,
    /// Sorted.
    pub extra: Vec<u32>,
    /// `(where, code)`, sorted by place.
    pub known: Vec<(Place, u32)>,
    /// `(queue index, code)` for each queued unit whose subject card the
    /// viewer knows: a card being set from their own hand is theirs to know
    /// while the question about it is pending, before it reaches the field.
    pub subjects: Vec<(usize, u32)>,
}

/// The pending question as the viewer sees it, with the viewer's partial
/// picks where the question is being answered piece by piece.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum QuestionView {
    YesNo {
        player: u8,
        description: u64,
    },
    EffectYesNo {
        player: u8,
        code: u32,
        place: Place,
        description: u64,
    },
    Option {
        player: u8,
        options: Vec<u64>,
    },
    Card {
        player: u8,
        cancelable: bool,
        min: u8,
        max: u8,
        cards: Vec<CardRef>,
        picked: Vec<usize>,
    },
    Tribute {
        player: u8,
        cancelable: bool,
        min: u8,
        max: u8,
        cards: Vec<(CardRef, u32)>,
        picked: Vec<usize>,
    },
    Unselect {
        player: u8,
        finishable: bool,
        cancelable: bool,
        min: u8,
        max: u8,
        select: Vec<CardRef>,
        unselect: Vec<CardRef>,
    },
    Chain {
        player: u8,
        spe_count: u8,
        forced: bool,
        hint_timing: u32,
        opponent_hint_timing: u32,
        chains: Vec<(u32, u64, Place)>,
    },
    Place {
        player: u8,
        count: u8,
        flag: u32,
        disable_field: bool,
        taken: Vec<(u8, u8, u8)>,
    },
    Position {
        player: u8,
        code: u32,
        positions: u8,
    },
    Idle {
        player: u8,
        summonable: Vec<CardRef>,
        spsummonable: Vec<CardRef>,
        repositionable: Vec<CardRef>,
        msetable: Vec<CardRef>,
        ssetable: Vec<CardRef>,
        activatable: Vec<(u32, u64, Place)>,
        to_bp: bool,
        to_ep: bool,
    },
    Battle {
        player: u8,
        activatable: Vec<(u32, u64, Place)>,
        attackable: Vec<(CardRef, bool)>,
        to_m2: bool,
        to_ep: bool,
    },
    Race {
        player: u8,
        count: u8,
        available: u64,
        declared: u64,
    },
    Sort {
        player: u8,
        is_chain: bool,
        cards: Vec<CardRef>,
        order: Vec<usize>,
    },
    Coin {
        player: u8,
        count: u8,
    },
    Random {
        player: u8,
        count: u8,
        cards: Vec<CardRef>,
    },
    DeckTop {
        player: u8,
        count: u32,
        settled: usize,
    },
}

/// The viewer's partial answer, from the `Game`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PartialView {
    pub picks: Vec<usize>,
    pub places: Vec<(u8, u8, u8)>,
    pub types: u64,
    pub order: Vec<usize>,
    pub settled: usize,
}

/// A viewer's whole observation. `Hash` and `Eq` are the information-set
/// identity; [`Observation::key`] is the hash.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Observation {
    pub viewer: u8,
    pub public: Public,
    pub private: Private,
    pub question: Option<QuestionView>,
}

impl Observation {
    /// The information-set key: a 64-bit hash of the observation. Use the
    /// observation itself where collisions must be impossible.
    pub fn key(&self) -> u64 {
        let mut h = FxHasher::default();
        self.hash(&mut h);
        h.finish()
    }

    /// The public part's key, the same for both viewers.
    pub fn public_key(&self) -> u64 {
        let mut h = FxHasher::default();
        self.public.hash(&mut h);
        h.finish()
    }
}

/// The status bits everyone can see on any card: the events that set
/// them were watched.
const HIDDEN_STATUS: u32 = status::SET_TURN
    | status::SUMMON_TURN
    | status::FLIP_SUMMON_TURN
    | status::SPSUMMON_TURN
    | status::ATTACK_CANCELED
    | status::JUST_POS
    | status::CONTINUOUS_POS
    | status::SUMMONING
    | status::SPSUMMON_STEP
    | status::PROC_COMPLETE;

/// The bits that are engine bookkeeping rather than state.
const INTERNAL_STATUS: u32 = status::INITIALIZING | status::COPYING_EFFECT;

/// Build the viewer's observation. `&mut Field` because the effective
/// stats are computed through the engine's own getters, which keep their
/// scratch caches on the cards.
pub fn observe(
    f: &mut Field,
    knowledge: &Knowledge,
    question: Option<&Message>,
    partial: &PartialView,
    viewer: u8,
) -> Observation {
    let card_view = |f: &mut Field, c: CardId| -> CardView {
        let public = is_public(&f.cards[c]);
        let known = public || knowledge.known_to_both(c);
        let on_field = f.cards[c].current.location & location::ONFIELD != 0;
        let face_up = f.cards[c].current.position & position::FACEUP != 0;
        let stats = if on_field && face_up {
            Some(Stats {
                attack: f.get_attack(c),
                defense: f.get_defense(c),
                level: f.get_level(c),
            })
        } else {
            None
        };
        let card = &f.cards[c];
        CardView {
            code: known.then_some(card.data.code),
            position: card.current.position,
            owner: known.then_some(card.owner),
            stats,
            status: if public {
                card.status & !INTERNAL_STATUS
            } else {
                card.status & HIDDEN_STATUS
            },
            counters: card
                .counters
                .iter()
                .map(|(&k, v)| (k, v[0], v[1]))
                .collect(),
            equip_target: card.equiping_target.map(|t| place_of(f, t)),
            summon_type: card.summon.type_,
            turn: card.turnid,
            attacks_announced: card.attack_announce_count,
            attacked: card.attacked_count,
        }
    };
    let card_ref = |f: &Field, c: CardId| -> CardRef {
        let known = is_public(&f.cards[c]) || knowledge.knows(viewer, c);
        CardRef {
            code: known.then_some(f.cards[c].data.code),
            place: place_of(f, c),
        }
    };

    let mut players = Vec::with_capacity(2);
    for p in 0..2u8 {
        let pi = usize::from(p);
        let mzone: Vec<Option<CardView>> = (0..f.players[pi].mzone.len())
            .map(|s| f.players[pi].mzone[s].map(|c| card_view(f, c)))
            .collect();
        let szone: Vec<Option<CardView>> = (0..f.players[pi].szone.len())
            .map(|s| f.players[pi].szone[s].map(|c| card_view(f, c)))
            .collect();
        let grave: Vec<CardView> = f.players[pi]
            .grave
            .clone()
            .into_iter()
            .map(|c| card_view(f, c))
            .collect();
        let removed: Vec<CardView> = f.players[pi]
            .removed
            .clone()
            .into_iter()
            .map(|c| card_view(f, c))
            .collect();
        let pile = |f: &Field, cards: &[CardId]| {
            let mut known: Vec<u32> = cards
                .iter()
                .filter(|&&c| knowledge.known_to_both(c))
                .map(|&c| f.cards[c].data.code)
                .collect();
            known.sort_unstable();
            HiddenPile {
                unknown: cards.len() - known.len(),
                known,
            }
        };
        let hand = pile(f, &f.players[pi].hand);
        let extra = pile(f, &f.players[pi].extra);
        let z = &f.players[pi];
        players.push(PlayerView {
            lp: z.lp,
            deck: z.main.len(),
            hand,
            mzone,
            szone,
            grave,
            removed,
            extra,
            used_location: z.used_location,
            disabled_location: z.disabled_location,
        });
    }
    let players: [PlayerView; 2] = match players.try_into() {
        Ok(p) => p,
        Err(_) => unreachable!("two players"),
    };

    let chain: Vec<ChainView> = f
        .core
        .current_chain
        .iter()
        .map(|ch| {
            let e = f.effects.get(ch.triggering_effect);
            let handler = e.and_then(|e| e.get_handler(&f.cards));
            ChainView {
                code: handler.map_or(0, |h| f.cards[h].data.code),
                description: e.map_or(0, |e| e.description),
                triggering_player: ch.triggering_player,
                triggering_controller: ch.triggering_controler,
                triggering_location: ch.triggering_location,
                triggering_sequence: ch.triggering_sequence,
                triggering_position: ch.triggering_position,
                event_code: ch.evt.event_code,
                event_player: ch.evt.event_player,
                event_value: ch.evt.event_value,
                targets: ch.target_cards.iter().map(|&c| place_of(f, c)).collect(),
                target_player: ch.target_player,
                target_param: ch.target_param,
                negated: ch.disable_reason.is_some(),
                flag: ch.flag,
            }
        })
        .collect();

    let battle = BattleView {
        attacker: f.core.attacker.map(|c| place_of(f, c)),
        target: f.core.attack_target.map(|c| place_of(f, c)),
        chain_attack: f.core.chain_attack,
        damage_calculated: f.core.damage_calculated,
        battle_damage: f.core.battle_damage,
        attack_state_count: f.core.attack_state_count,
        battled_count: f.core.battled_count,
    };

    // The public effect layer: every effect a public event granted, and
    // every printed effect of a public card. A hidden card's printed
    // effects are what would give it away.
    let mut seen: BTreeSet<crate::event::EffectId> = BTreeSet::new();
    let mut effects: Vec<EffectView> = Vec::new();
    let mut consider = |f: &Field, id: crate::event::EffectId, effects: &mut Vec<EffectView>| {
        if !seen.insert(id) {
            return;
        }
        let Some(e) = f.effects.get(id) else {
            return;
        };
        let handler = e.get_handler(&f.cards);
        let handler_public = handler.is_none_or(|h| is_public(&f.cards[h]));
        if e.is_flag(flag::INITIAL) && !handler_public {
            return;
        }
        effects.push(EffectView {
            code: e.code,
            kind: e.effect_type,
            flags: e.flag,
            handler: handler.map(|h| place_of(f, h)),
            owner: e.get_owner(&f.cards).map(|o| place_of(f, o)),
            range: (e.range, e.s_range, e.o_range),
            reset_flag: e.reset_flag,
            reset_count: e.reset_count,
            count_limit: e.count_limit,
            description: e.description,
            value: e.value,
            label: e.label.clone(),
        });
    };
    let card_effects: Vec<crate::event::EffectId> = f
        .cards
        .iter()
        .flat_map(|c| c.indexer.iter().copied())
        .collect();
    for id in card_effects {
        consider(f, id, &mut effects);
    }
    let field_effects: Vec<crate::event::EffectId> =
        f.field_effects.indexer.iter().copied().collect();
    for id in field_effects {
        consider(f, id, &mut effects);
    }
    effects.sort();

    let units: Vec<&crate::processor::Unit> = f
        .core
        .units
        .iter()
        .chain(f.core.subunits.iter().rev())
        .collect();
    let queue: Vec<(String, u16, Option<Place>)> = units
        .iter()
        .map(|u| {
            (
                kind_name(&u.kind),
                u.step,
                subject(&u.kind).map(|c| place_of(f, c)),
            )
        })
        .collect();

    let public = Public {
        turn: f.infos.turn_id,
        turn_player: f.infos.turn_player,
        phase: f.infos.phase,
        phase_action: f.core.phase_action,
        players,
        chain,
        battle,
        summon_count: f.core.summon_count,
        normal_summons: f.core.normalsummon_state_count,
        flip_summons: f.core.flipsummon_state_count,
        special_summons: f.core.spsummon_state_count,
        extra_summon: f.core.extra_summon,
        effects,
        queue,
    };

    let vi = usize::from(viewer);
    let hand: Vec<u32> = f.players[vi]
        .hand
        .iter()
        .map(|&c| f.cards[c].data.code)
        .collect();
    let mut extra: Vec<u32> = f.players[vi]
        .extra
        .iter()
        .map(|&c| f.cards[c].data.code)
        .collect();
    extra.sort_unstable();
    // Every hidden card the viewer knows and the other player does not,
    // outside the viewer's own hand and Extra Deck — and never a card in a
    // deck: a deck's identities are the decklist's and its order is
    // hidden, so a deck card has no place a viewer may know.
    let mut known: Vec<(Place, u32)> = (0..f.cards.len())
        .filter(|&c| {
            let cur = &f.cards[c].current;
            let in_own_piles =
                cur.controller == viewer && cur.location & (location::HAND | location::EXTRA) != 0;
            !in_own_piles
                && cur.location != location::DECK
                && !is_public(&f.cards[c])
                && knowledge.knows(viewer, c)
                && !knowledge.known_to_both(c)
        })
        .map(|c| (place_of(f, c), f.cards[c].data.code))
        .collect();
    known.sort_unstable();
    // The identity of each queued unit's subject, where the viewer knows it:
    // a public card, one the knowledge model says they know, or one in their
    // own hand or Extra Deck.
    let subjects: Vec<(usize, u32)> = units
        .iter()
        .enumerate()
        .filter_map(|(i, u)| {
            let c = subject(&u.kind)?;
            let cur = &f.cards[c].current;
            let own_pile =
                cur.controller == viewer && cur.location & (location::HAND | location::EXTRA) != 0;
            (own_pile || is_public(&f.cards[c]) || knowledge.knows(viewer, c))
                .then(|| (i, f.cards[c].data.code))
        })
        .collect();
    let private = Private {
        viewer,
        hand,
        extra,
        known,
        subjects,
    };

    let question = question.map(|q| question_view(f, q, partial, &card_ref));
    Observation {
        viewer,
        public,
        private,
        question,
    }
}

/// The card a queued unit operates on, where its payload names one: the
/// card being summoned, set, moved to the field, equipped, released or
/// destroyed by a replacement, or whose effect is being executed. The
/// observation records where it is for everyone and what it is for a viewer
/// who knows it; without it, two states that differ only in which card a
/// pending operation is working on would look alike to the player who chose
/// that card.
fn subject(kind: &crate::processor::Kind) -> Option<CardId> {
    use crate::processor::Kind as K;
    match kind {
        K::ExecuteCost { subject, .. }
        | K::ExecuteTarget { subject, .. }
        | K::ExecuteOperation { subject, .. } => *subject,
        K::SelectEffectYesNo { card, .. } | K::SelfDestroyUnique { card, .. } => Some(*card),
        K::RemoveCounter { pcard, .. } => *pcard,
        K::OperationReplace { target, .. } => *target,
        K::SelectTribute { target, .. }
        | K::Equip { target, .. }
        | K::DestroyReplace { target, .. }
        | K::ReleaseReplace { target, .. }
        | K::SummonRule { target, .. }
        | K::SpSummonRule { target, .. }
        | K::SpSummonStep { target, .. }
        | K::FlipSummon { target, .. }
        | K::SpellSet { target, .. }
        | K::MonsterSet { target, .. }
        | K::MoveToField { target, .. }
        | K::SendToReplace { target, .. } => Some(*target),
        _ => None,
    }
}

/// A unit's kind by name: the control-flow skeleton, without the payload.
fn kind_name(kind: &crate::processor::Kind) -> String {
    let s = format!("{kind:?}");
    s.split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or("")
        .to_string()
}

fn question_view(
    f: &Field,
    q: &Message,
    partial: &PartialView,
    card_ref: &dyn Fn(&Field, CardId) -> CardRef,
) -> QuestionView {
    let offer = |o: &crate::field::IdleOffer| CardRef {
        code: Some(o.code),
        place: Place {
            controller: o.controller,
            location: o.location,
            sequence: o.sequence,
        },
    };
    let chain_offer = |o: &crate::field::ChainOffer| {
        (
            o.code,
            o.description,
            Place {
                controller: o.info.controller,
                location: o.info.location,
                sequence: o.info.sequence,
            },
        )
    };
    match q {
        Message::SelectYesNo {
            player,
            description,
        } => QuestionView::YesNo {
            player: *player,
            description: *description,
        },
        Message::SelectEffectYesNo {
            player,
            code,
            controller,
            location,
            sequence,
            description,
            ..
        } => QuestionView::EffectYesNo {
            player: *player,
            code: *code,
            place: Place {
                controller: *controller,
                location: *location,
                sequence: *sequence,
            },
            description: *description,
        },
        Message::SelectOption { player, options } => QuestionView::Option {
            player: *player,
            options: options.clone(),
        },
        Message::SelectCard {
            player,
            cancelable,
            min,
            max,
            cards,
        } => QuestionView::Card {
            player: *player,
            cancelable: *cancelable,
            min: *min,
            max: *max,
            cards: cards.iter().map(|&c| card_ref(f, c)).collect(),
            picked: partial.picks.clone(),
        },
        Message::SelectTribute {
            player,
            cancelable,
            min,
            max,
            cards,
        } => QuestionView::Tribute {
            player: *player,
            cancelable: *cancelable,
            min: *min,
            max: *max,
            cards: cards.iter().map(|&(c, r)| (card_ref(f, c), r)).collect(),
            picked: partial.picks.clone(),
        },
        Message::SelectUnselectCard {
            player,
            finishable,
            cancelable,
            min,
            max,
            select,
            unselect,
        } => QuestionView::Unselect {
            player: *player,
            finishable: *finishable,
            cancelable: *cancelable,
            min: *min,
            max: *max,
            select: select.iter().map(|&c| card_ref(f, c)).collect(),
            unselect: unselect.iter().map(|&c| card_ref(f, c)).collect(),
        },
        Message::SelectChain {
            player,
            spe_count,
            forced,
            hint_timing,
            opponent_hint_timing,
            chains,
        } => QuestionView::Chain {
            player: *player,
            spe_count: *spe_count,
            forced: *forced,
            hint_timing: *hint_timing,
            opponent_hint_timing: *opponent_hint_timing,
            chains: chains.iter().map(chain_offer).collect(),
        },
        Message::SelectPlace {
            player,
            count,
            flag,
            disable_field,
        } => QuestionView::Place {
            player: *player,
            count: *count,
            flag: *flag,
            disable_field: *disable_field,
            taken: partial.places.clone(),
        },
        Message::SelectPosition {
            player,
            code,
            positions,
        } => QuestionView::Position {
            player: *player,
            code: *code,
            positions: *positions,
        },
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
            ..
        } => QuestionView::Idle {
            player: *player,
            summonable: summonable.iter().map(offer).collect(),
            spsummonable: spsummonable.iter().map(offer).collect(),
            repositionable: repositionable.iter().map(offer).collect(),
            msetable: msetable.iter().map(offer).collect(),
            ssetable: ssetable.iter().map(offer).collect(),
            activatable: activatable.iter().map(chain_offer).collect(),
            to_bp: *to_bp,
            to_ep: *to_ep,
        },
        Message::SelectBattleCmd {
            player,
            activatable,
            attackable,
            to_m2,
            to_ep,
        } => QuestionView::Battle {
            player: *player,
            activatable: activatable.iter().map(chain_offer).collect(),
            attackable: attackable
                .iter()
                .map(|a| {
                    (
                        CardRef {
                            code: Some(a.code),
                            place: Place {
                                controller: a.controller,
                                location: a.location,
                                sequence: a.sequence,
                            },
                        },
                        a.direct_attackable,
                    )
                })
                .collect(),
            to_m2: *to_m2,
            to_ep: *to_ep,
        },
        Message::AnnounceRace {
            player,
            count,
            available,
        } => QuestionView::Race {
            player: *player,
            count: *count,
            available: *available,
            declared: partial.types,
        },
        Message::Sort {
            player,
            is_chain,
            cards,
        } => QuestionView::Sort {
            player: *player,
            is_chain: *is_chain,
            cards: cards.iter().map(offer).collect(),
            order: partial.order.clone(),
        },
        Message::SelectCoin { player, count } => QuestionView::Coin {
            player: *player,
            count: *count,
        },
        Message::SelectRandom {
            player,
            count,
            cards,
        } => QuestionView::Random {
            player: *player,
            count: *count,
            cards: cards.iter().map(|&c| card_ref(f, c)).collect(),
        },
        Message::SelectDeckTop { player, count } => QuestionView::DeckTop {
            player: *player,
            count: *count,
            settled: partial.settled,
        },
        other => unreachable!("not a question: {other:?}"),
    }
}
