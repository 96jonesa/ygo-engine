//! A canonical trace of a duel, for comparing against ocgcore.
//!
//! The port and the reference are two different programs with two different
//! message representations. To compare them, both render a duel into the
//! **same** line-based form, and the comparison is a diff of those lines.
//!
//! ## What goes in, and why so little
//!
//! Only what both engines can be held to:
//!
//! - **questions** — who is asked, what kind, and the *shape* of what is
//!   offered
//! - **decisions** — what the policy answered
//! - **state changes with a public consequence** — draws, moves, damage,
//!   summons, the win
//!
//! Deliberately **not** included: anything either engine is free to do
//! differently without being wrong. Internal ordering of simultaneous
//! events, hint messages, the exact contents of a shuffled pile. Those are
//! places where a difference is not a divergence, and putting them in the
//! trace would drown the signal.
//!
//! Every exemption is a case the harness cannot report on, so each one is
//! written down rather than merely omitted — the handoff's rule, and the
//! reason this module has more comment than code.
//!
//! ## Card identity
//!
//! A trace line names a card by its **printed code and where it is**, never
//! by an index — the two engines' internal ids have no reason to agree, and
//! a trace that compares them would diverge on the first line.

use crate::field::Message;

/// One comparable line.
///
/// `Display` is the wire format: a short tag, then fields separated by
/// spaces. Stable across versions on purpose — a trace is a thing to
/// compare, and a format that drifts makes old traces worthless.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// A question was asked: kind, player, how many options — and, for
    /// a chain window, **which cards** were offered, so a divergence
    /// names the effect one engine offered and the other did not, and an
    /// ordering difference shows as one rather than as a later, baffling
    /// divergence after the random policy picked index `k` of two
    /// different lists.
    Ask {
        kind: &'static str,
        player: u8,
        count: usize,
        detail: String,
    },
    /// The policy answered. The payload is the answer's meaning, not its
    /// encoding — `idle 7` rather than the packed integer, because the two
    /// engines encode differently and mean the same.
    Answer { kind: &'static str, value: String },
    /// Something publicly visible happened.
    Event { kind: &'static str, detail: String },
}

impl std::fmt::Display for Line {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Line::Ask {
                kind,
                player,
                count,
                detail,
            } => {
                write!(f, "ask {kind} p{player} n{count}")?;
                if !detail.is_empty() {
                    write!(f, " [{detail}]")?;
                }
                Ok(())
            }
            Line::Answer { kind, value } => write!(f, "ans {kind} {value}"),
            Line::Event { kind, detail } => {
                // No trailing space for a detail-less event: the oracle
                // side renders `evt chainend`, and so must this.
                if detail.is_empty() {
                    write!(f, "evt {kind}")
                } else {
                    write!(f, "evt {kind} {detail}")
                }
            }
        }
    }
}

/// Render a message as a trace line, or `None` if it is not comparable.
///
/// The `None` cases are the exemptions, and each is justified. Since
/// 2026-09-17 there is one: `Retry`, a protocol artefact of one side's
/// policy giving a bad answer, not a fact about the duel. Everything else
/// the reference writes is rendered — hints included, since the reference
/// emits them at fixed points with fixed values, and a port that hints
/// differently reached a different point. Some kinds are rendered
/// *partially*, where this port's message carries fewer fields than the
/// reference's: `Move` (code and reason, no locations), `Chaining` (the
/// triggering location, not the handler's), `BecomeTarget` (a count),
/// `MissedEffect` (the code), `Swap` (the codes), `ShuffleSetCard` (the
/// counts). Widening those is the next step; the partial forms are still
/// compared on both sides.
/// Codes joined by commas, the list shape every event uses.
fn join(codes: &[u32]) -> String {
    codes
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

pub fn line_for(m: &Message) -> Option<Line> {
    Some(match m {
        Message::SelectIdleCmd {
            player,
            summonable,
            spsummonable,
            repositionable,
            msetable,
            ssetable,
            activatable,
            ..
        } => Line::Ask {
            kind: "idle",
            player: *player,
            count: summonable.len()
                + spsummonable.len()
                + repositionable.len()
                + msetable.len()
                + ssetable.len()
                + activatable.len(),
            // Every list's codes, in order: a candidate list in a different
            // order is a divergence the random policy only sometimes lands
            // on, so the order is compared at the ask.
            detail: format!(
                "s:{} sp:{} r:{} ms:{} ss:{} a:{}",
                join(&summonable.iter().map(|o| o.code).collect::<Vec<_>>()),
                join(&spsummonable.iter().map(|o| o.code).collect::<Vec<_>>()),
                join(&repositionable.iter().map(|o| o.code).collect::<Vec<_>>()),
                join(&msetable.iter().map(|o| o.code).collect::<Vec<_>>()),
                join(&ssetable.iter().map(|o| o.code).collect::<Vec<_>>()),
                join(&activatable.iter().map(|o| o.code).collect::<Vec<_>>()),
            ),
        },
        Message::SelectBattleCmd {
            player,
            activatable,
            attackable,
            ..
        } => Line::Ask {
            kind: "battle",
            player: *player,
            count: activatable.len() + attackable.len(),
            detail: format!(
                "a:{} at:{}",
                join(&activatable.iter().map(|o| o.code).collect::<Vec<_>>()),
                join(&attackable.iter().map(|o| o.code).collect::<Vec<_>>()),
            ),
        },
        Message::SelectCard { player, cards, .. } => Line::Ask {
            kind: "card",
            player: *player,
            count: cards.len(),
            detail: String::new(),
        },
        // Both of these are rendered as `card` because that is what they
        // are to a host: a list to choose from. `SelectUnselectCard`'s
        // count is the two lists together, in the order the answer
        // numbers them — selectable first, then what may be given back.
        Message::SelectUnselectCard {
            player,
            select,
            unselect,
            ..
        } => Line::Ask {
            kind: "card",
            player: *player,
            count: select.len() + unselect.len(),
            detail: String::new(),
        },
        Message::SelectTribute { player, cards, .. } => Line::Ask {
            kind: "card",
            player: *player,
            count: cards.len(),
            detail: String::new(),
        },
        Message::SelectChain {
            player,
            chains,
            spe_count,
            forced,
            hint_timing,
            opponent_hint_timing,
        } => {
            let mut detail = chains
                .iter()
                .map(|c| c.code.to_string())
                .collect::<Vec<_>>()
                .join(",");
            // Diagnostic: the window's kind, in the same words the mirror
            // uses, so the differential compares window kinds too.
            if std::env::var_os("DIFF_CHAIN_DEBUG").is_some() {
                detail = format!(
                    "{detail}] spe={spe_count} forced={} t={hint_timing:#x}/{opponent_hint_timing:#x} [",
                    u8::from(*forced)
                );
            }
            Line::Ask {
                kind: "chain",
                player: *player,
                count: chains.len(),
                detail,
            }
        }
        Message::SelectYesNo { player, .. } => Line::Ask {
            kind: "yesno",
            player: *player,
            count: 2,
            detail: String::new(),
        },
        Message::SelectCoin { player, count } => Line::Ask {
            kind: "coin",
            player: *player,
            count: usize::from(*count),
            detail: String::new(),
        },
        Message::SelectRandom {
            player,
            count,
            cards,
        } => Line::Ask {
            kind: "random",
            player: *player,
            count: usize::from(*count),
            detail: format!(
                "[{}]",
                join(&cards.iter().map(|&c| c as u32).collect::<Vec<_>>())
            ),
        },
        Message::SelectDeckTop { player, count } => Line::Ask {
            kind: "decktop",
            player: *player,
            count: *count as usize,
            detail: String::new(),
        },
        Message::SelectEffectYesNo { player, .. } => Line::Ask {
            kind: "effectyesno",
            player: *player,
            count: 2,
            detail: String::new(),
        },
        Message::SelectOption { player, options } => Line::Ask {
            kind: "option",
            player: *player,
            count: options.len(),
            detail: String::new(),
        },
        Message::SelectPosition {
            player, positions, ..
        } => Line::Ask {
            kind: "position",
            player: *player,
            count: positions.count_ones() as usize,
            detail: String::new(),
        },
        Message::SelectPlace { player, count, .. } => Line::Ask {
            kind: "place",
            player: *player,
            count: *count as usize,
            detail: String::new(),
        },
        // The *offer* is how many races are available, not how many the
        // player must name — that is the shape both engines can be held to.
        Message::AnnounceRace {
            player, available, ..
        } => Line::Ask {
            kind: "race",
            player: *player,
            count: available.count_ones() as usize,
            detail: String::new(),
        },

        Message::Draw { player, codes } => Line::Event {
            kind: "draw",
            detail: format!("p{player} n{}", codes.len()),
        },
        // The hand order after a shuffle, card for card — see the note in
        // `differential.py`'s mirror. `DUEL_PSEUDO_SHUFFLE` does not cover
        // the hand, so this is a real roll, and the order it produces is
        // what every later hand index means. A count alone would compare
        // nothing.
        Message::ShuffleHand { player, codes } => Line::Event {
            kind: "shufflehand",
            detail: format!(
                "p{player} [{}]",
                codes
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        },
        Message::Damage { player, amount } => Line::Event {
            kind: "damage",
            detail: format!("p{player} {amount}"),
        },
        Message::Recover { player, amount } => Line::Event {
            kind: "recover",
            detail: format!("p{player} {amount}"),
        },
        Message::PayLpCost { player, amount } => Line::Event {
            kind: "paylp",
            detail: format!("p{player} {amount}"),
        },
        Message::NewTurn { player } => Line::Event {
            kind: "turn",
            detail: format!("p{player}"),
        },
        Message::NewPhase { phase } => Line::Event {
            kind: "phase",
            detail: format!("{phase:#x}"),
        },
        Message::Win { player, reason } => Line::Event {
            kind: "win",
            detail: format!("p{player} r{reason}"),
        },
        Message::Attack { attacker, target } => Line::Event {
            kind: "attack",
            // Located, not identified: `controller:location:sequence`,
            // which both engines agree on where card ids would not.
            detail: format!(
                "{}:{}:{} -> {}:{}:{}",
                attacker.controller,
                attacker.location,
                attacker.sequence,
                target.controller,
                target.location,
                target.sequence
            ),
        },
        Message::Battle {
            attacker_attack,
            attacker_destroyed,
            target_attack,
            target_destroyed,
            ..
        } => Line::Event {
            kind: "battle",
            detail: format!(
                "{attacker_attack}/{} {target_attack}/{}",
                u8::from(*attacker_destroyed),
                u8::from(*target_destroyed)
            ),
        },
        Message::Summoning { code, .. } => Line::Event {
            kind: "summon",
            detail: format!("{code}"),
        },
        Message::SpSummoning { code, .. } => Line::Event {
            kind: "spsummon",
            detail: format!("{code}"),
        },
        Message::FlipSummoning { code, .. } => Line::Event {
            kind: "flipsummon",
            detail: format!("{code}"),
        },
        Message::Set { code, .. } => Line::Event {
            kind: "set",
            detail: format!("{code}"),
        },
        // A coin toss is one of the few places the two engines draw from
        // their random generators, so the results are compared rather than
        // exempted — which is only meaningful if both are seeded alike.
        Message::TossCoin { player, results } => Line::Event {
            kind: "coin",
            detail: format!(
                "p{player} {}",
                results
                    .iter()
                    .map(|&h| if h { 'h' } else { 't' })
                    .collect::<String>()
            ),
        },
        // `MSG_EQUIP` — an attachment is a piece of board state nothing
        // else in the trace reports, and both sides are rendered as
        // locations because that is what the reference writes: two copies
        // of one equip card are told apart by their seat, not their code.
        Message::Equip { equip, target } => Line::Event {
            kind: "equip",
            detail: format!(
                "p{} l{} s{} -> p{} l{} s{}",
                equip.controller,
                equip.location,
                equip.sequence,
                target.controller,
                target.location,
                target.sequence
            ),
        },
        Message::DamageStepStart => Line::Event {
            kind: "damagestep",
            detail: "start".into(),
        },
        Message::DamageStepEnd => Line::Event {
            kind: "damagestep",
            detail: "end".into(),
        },

        // ---- The widened half (2026-09-17): every informational message
        // ocgcore writes that the pool can produce, rendered field for
        // field. `tools/differential.py::event_line` decodes the same
        // fields from the reference's bytes; a divergence here is a
        // fidelity finding about what a client sees, not about a rule.
        Message::Hint {
            kind,
            player,
            value,
        } => Line::Event {
            kind: "hint",
            detail: format!("k{kind} p{player} {value}"),
        },
        Message::Chaining {
            code,
            controller,
            location,
            sequence,
            description,
            chain_count,
        } => Line::Event {
            kind: "chaining",
            // The reference writes the triggering location as a byte, so
            // `SZONE | STZONE` (0x408) is `8` on the wire.
            detail: format!(
                "{code} t{controller}:{}:{sequence} d{description} n{chain_count}",
                location & 0xff
            ),
        },
        Message::Chained { chain_count } => Line::Event {
            kind: "chained",
            detail: format!("n{chain_count}"),
        },
        Message::ChainSolving { chain_count } => Line::Event {
            kind: "chainsolving",
            detail: format!("n{chain_count}"),
        },
        Message::ChainSolved { chain_count } => Line::Event {
            kind: "chainsolved",
            detail: format!("n{chain_count}"),
        },
        Message::ChainDisabled { chain_count } => Line::Event {
            kind: "chaindisabled",
            detail: format!("n{chain_count}"),
        },
        Message::ChainEnd => Line::Event {
            kind: "chainend",
            detail: String::new(),
        },
        Message::Summoned => Line::Event {
            kind: "summoned",
            detail: String::new(),
        },
        Message::SpSummoned => Line::Event {
            kind: "spsummoned",
            detail: String::new(),
        },
        Message::FlipSummoned => Line::Event {
            kind: "flipsummoned",
            detail: String::new(),
        },
        Message::ConfirmCards { player, codes } => Line::Event {
            kind: "confirm",
            detail: format!("p{player} [{}]", join(codes)),
        },
        Message::ConfirmDeckTop { player, codes } => Line::Event {
            kind: "confirmtop",
            detail: format!("p{player} [{}]", join(codes)),
        },
        Message::ShuffleDeck { player } => Line::Event {
            kind: "shuffledeck",
            detail: format!("p{player}"),
        },
        Message::ShuffleExtra { player, codes } => Line::Event {
            kind: "shuffleextra",
            detail: format!("p{player} [{}]", join(codes)),
        },
        Message::DeckTop {
            player,
            sequence,
            code,
            position,
        } => Line::Event {
            kind: "decktop",
            detail: format!("p{player} s{sequence} {code} pos{position}"),
        },
        Message::ReverseDeck => Line::Event {
            kind: "reversedeck",
            detail: String::new(),
        },
        Message::AttackDisabled => Line::Event {
            kind: "attackdisabled",
            detail: String::new(),
        },
        Message::FieldDisabled { locations } => Line::Event {
            kind: "fielddisabled",
            detail: format!("{locations:#x}"),
        },
        Message::AddCounter {
            counter_type,
            controller,
            location,
            sequence,
            count,
        } => Line::Event {
            kind: "addcounter",
            detail: format!("t{counter_type} p{controller} l{location} s{sequence} n{count}"),
        },
        Message::RemoveCounter {
            counter_type,
            controller,
            location,
            sequence,
            count,
        } => Line::Event {
            kind: "removecounter",
            detail: format!("t{counter_type} p{controller} l{location} s{sequence} n{count}"),
        },
        Message::MatchKill { code } => Line::Event {
            kind: "matchkill",
            detail: format!("{code}"),
        },
        Message::Swap { first, second } => Line::Event {
            kind: "swap",
            detail: format!("{first} {second}"),
        },
        Message::CardHint {
            controller,
            location,
            sequence,
            kind,
            value,
        } => Line::Event {
            kind: "cardhint",
            detail: format!("p{controller} l{location} s{sequence} k{kind} {value}"),
        },
        Message::CardTarget { owner, target } => Line::Event {
            kind: "cardtarget",
            detail: format!(
                "p{} l{} s{} -> p{} l{} s{}",
                owner.controller,
                owner.location,
                owner.sequence,
                target.controller,
                target.location,
                target.sequence
            ),
        },
        Message::CardSelected { cards } => Line::Event {
            kind: "cardselected",
            detail: format!(
                "[{}]",
                cards
                    .iter()
                    .map(|c| format!("p{} l{} s{}", c.controller, c.location, c.sequence))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        },
        Message::MissedEffect { code, .. } => Line::Event {
            kind: "missed",
            detail: format!("{code}"),
        },
        Message::RandomSelected { player, cards } => Line::Event {
            kind: "randomselected",
            detail: format!(
                "p{player} [{}]",
                cards
                    .iter()
                    .map(|c| format!("p{} l{} s{}", c.controller, c.location, c.sequence))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        },
        Message::BecomeTarget { cards } => Line::Event {
            kind: "becometarget",
            detail: format!("n{}", cards.len()),
        },
        Message::Move {
            code,
            previous,
            current,
            reason,
        } => Line::Event {
            kind: "move",
            detail: format!(
                "{code} {}:{}:{}:{} -> {}:{}:{}:{} r{reason:#x}",
                previous.controller,
                previous.location,
                previous.sequence,
                previous.position,
                current.controller,
                current.location,
                current.sequence,
                current.position
            ),
        },
        Message::ShuffleSetCard { location, count } => Line::Event {
            kind: "shuffleset",
            detail: format!("l{location} n{count}"),
        },

        // Everything else is an exemption. See the module notes.
        _ => return None,
    })
}

/// Render a policy's answer as a trace line.
///
/// The counterpart to [`line_for`], and the half that says what was
/// *decided* rather than what was offered. It reads the answer back out of
/// `returns` — the encoding the engine will actually consume — rather than
/// taking the policy's word for it, so an answer written into the wrong
/// slot shows up in the trace instead of hiding behind a retry.
///
/// The rendered **meaning** is what is compared, never the encoding: the
/// reference packs these answers differently and means the same thing.
pub fn answer_line(question: &Message, returns: &crate::field::Returns) -> Option<Line> {
    let (kind, value) = match question {
        Message::SelectYesNo { .. } | Message::SelectEffectYesNo { .. } => (
            "yesno",
            if returns.at_i32(0) != 0 { "y" } else { "n" }.to_string(),
        ),
        Message::SelectOption { .. } => ("option", format!("{}", returns.at_i32(0))),
        Message::SelectCoin { .. } => ("coin", format!("{:#x}", returns.at_i32(0))),
        Message::SelectRandom { .. } => ("random", {
            let n = returns.at_i32(1).max(0) as usize;
            (0..n)
                .map(|i| returns.at_i32(i + 2).to_string())
                .collect::<Vec<_>>()
                .join(",")
        }),
        // The answer is the deck's order, given before the question is
        // answered; the line records only that it was.
        Message::SelectDeckTop { .. } => ("decktop", "-".to_string()),
        Message::SelectCard { .. } => {
            // Slot 0 is the width tag, slot 1 the count, slots 2.. the
            // indices into the offer. Only the indices are comparable.
            let n = returns.at_i32(1).max(0) as usize;
            let picks: Vec<String> = (0..n).map(|i| returns.at_i32(i + 2).to_string()).collect();
            ("card", picks.join(","))
        }
        Message::SelectUnselectCard { .. } => {
            // Slot 0 is the kind, and `-1` there means "done choosing".
            if returns.at_i32(0) < 0 {
                ("unselect", "-".to_string())
            } else {
                ("unselect", format!("{}", returns.at_i32(1)))
            }
        }
        // `SelectTribute` is decoded by `parse_response_cards`, so its
        // answer is rendered the way `SelectCard`'s is.
        Message::SelectTribute { .. } => {
            let n = returns.at_i32(1).max(0) as usize;
            let picks: Vec<String> = (0..n).map(|i| returns.at_i32(i + 2).to_string()).collect();
            ("card", picks.join(","))
        }
        Message::SelectChain { .. } => {
            let v = returns.at_i32(0);
            (
                "chain",
                if v < 0 {
                    "-".to_string()
                } else {
                    v.to_string()
                },
            )
        }
        Message::SelectPlace { .. } => (
            "place",
            format!(
                "p{} l{} s{}",
                returns.at_i8(0),
                returns.at_i8(1),
                returns.at_i8(2)
            ),
        ),
        Message::SelectPosition { .. } => ("position", format!("{:#x}", returns.at_i32(0))),
        // Both menus answer with one packed integer: the low half is the
        // command, the high half the index within that command's list.
        Message::SelectIdleCmd { .. } => {
            let v = returns.at_u32(0);
            ("idle", format!("{} {}", v & 0xffff, v >> 16))
        }
        Message::SelectBattleCmd { .. } => {
            let v = returns.at_u32(0);
            ("battle", format!("{} {}", v & 0xffff, v >> 16))
        }
        Message::AnnounceRace { .. } => ("race", format!("{:#x}", returns.at_u64(0))),
        Message::Sort { .. } => (
            "sort",
            if returns.at_i8(0) < 0 {
                "-".to_string()
            } else {
                "0".to_string()
            },
        ),
        _ => return None,
    };
    Some(Line::Answer { kind, value })
}

/// A trace being built as a duel runs.
#[derive(Clone, Default)]
pub struct Trace {
    pub lines: Vec<Line>,
    /// How much of the message log has been rendered. The log is never
    /// trimmed, so re-rendering from the start would repeat everything.
    seen: usize,
}

impl Trace {
    /// Render whatever is new in the message log.
    pub fn absorb(&mut self, messages: &[Message]) {
        for m in &messages[self.seen.min(messages.len())..] {
            if let Some(line) = line_for(m) {
                self.lines.push(line);
            }
        }
        self.seen = messages.len();
    }

    /// Record what the policy answered. Kept in the same sequence as the
    /// questions, so a trace reads as a conversation.
    ///
    /// Called *after* the messages the question produced have been
    /// absorbed, so the answer lands under its own question rather than
    /// under the next one.
    pub fn answered(&mut self, question: &Message, returns: &crate::field::Returns) {
        if let Some(line) = answer_line(question, returns) {
            self.lines.push(line);
        }
    }

    pub fn render(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::{Driver, PassPolicy, Stop};

    /// **A duel produces a trace**, and it reads as a game: turns,
    /// phases, draws, and a win at the end.
    #[test]
    fn a_duel_produces_a_readable_trace() {
        let mut d = Driver::vanilla(PassPolicy, 40).tracing();
        let stop = d.run(500_000);
        assert!(matches!(stop, Stop::Win { .. }));
        let t = d.rendered_trace();
        assert!(t.contains("evt turn p0"), "turns");
        assert!(t.contains("evt phase"), "phases");
        assert!(t.contains("evt draw"), "draws");
        assert!(t.contains("evt win"), "and a conclusion");
        assert!(t.contains("ask idle"), "and the questions it was asked");
    }

    /// **The same duel traces identically.** A trace is only useful if it
    /// is a function of the inputs — anything that varies run to run would
    /// show up as a false divergence against the reference.
    #[test]
    fn the_trace_is_deterministic() {
        let a = {
            let mut d = Driver::vanilla(PassPolicy, 40).tracing();
            d.run(500_000);
            d.rendered_trace()
        };
        let b = {
            let mut d = Driver::vanilla(PassPolicy, 40).tracing();
            d.run(500_000);
            d.rendered_trace()
        };
        assert_eq!(a, b, "two runs of the same duel");
        assert!(a.lines().count() > 100, "and it is not trivially short");
    }

    /// **Retries are exempt** — an artefact of one side's answering rather
    /// than a fact about the duel. **Hints are not**, since 2026-09-17: the
    /// reference writes them at fixed points with fixed values, and a port
    /// that hints differently is a port that reached a different point.
    #[test]
    fn the_exemptions_are_excluded() {
        use crate::field::Message;
        assert!(line_for(&Message::Retry).is_none());
        assert_eq!(
            line_for(&Message::Hint {
                kind: 1,
                player: 0,
                value: 42
            })
            .map(|l| l.to_string()),
            Some("evt hint k1 p0 42".to_string())
        );
    }

    /// **A card is named by its code, never by an index.** The two
    /// engines' internal ids have no reason to agree.
    #[test]
    fn cards_are_named_by_code() {
        use crate::field::Message;
        let line = line_for(&Message::Summoning {
            code: 18036057,
            controller: 0,
            location: 4,
            sequence: 0,
            position: 1,
        });
        assert_eq!(
            line.map(|l| l.to_string()),
            Some("evt summon 18036057".to_string())
        );
    }

    /// **An attack names locations, not cards** — for the same reason.
    #[test]
    fn an_attack_is_located_not_identified() {
        use crate::field::Message;
        use crate::leave_field::LocInfo;
        let line = line_for(&Message::Attack {
            attacker: LocInfo {
                controller: 0,
                location: 4,
                sequence: 2,
                position: 1,
            },
            target: LocInfo::default(),
        });
        assert_eq!(
            line.map(|l| l.to_string()),
            Some("evt attack 0:4:2 -> 0:0:0".to_string())
        );
    }
}
