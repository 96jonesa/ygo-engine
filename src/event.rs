//! Events: what just happened, and who caused it.
//!
//! A translation of ocgcore's `tevent`. Almost every rules question in the
//! engine is asked *of an event* — "may this effect activate in response to
//! this?" — so the struct is small but reaches everywhere.
//!
//! The one systematic change from the reference is that pointers become
//! indices. ocgcore holds `card*` and `effect*`; a port that did the same
//! would need lifetimes threaded through every rules query, or unsafe. The
//! engine also has to be cheaply *cloneable* — the solver clones state
//! millions of times — and a graph of raw pointers is not. Indices into the
//! duel's own arenas clone by memcpy and borrow nothing.

/// Index into the duel's card arena. `card*` in the reference.
pub type CardId = usize;
/// Index into the duel's effect arena. `effect*` in the reference.
pub type EffectId = usize;

/// The player value meaning "nobody", as the reference spells it.
pub const PLAYER_NONE: u8 = 2;
/// `PLAYER_ALL` — "both", where a player field may say so.
pub const PLAYER_ALL: u8 = 3;

/// The "player" a card destroying *itself* names as the reason player.
///
/// Not a player at all — 5, where the real ones are 0 and 1. `Destroy` reads
/// it as "nobody chose this", and skips both the saving of the reason effect
/// and the charging of an indestructible effect's count: a card that destroys
/// itself is not something anyone did to it.
pub const PLAYER_SELFDES: u8 = 5;

/// Something that happened, which effects may respond to.
///
/// `event_code` is the *kind* — a summon, a destruction, a phase beginning.
/// The remaining fields are the circumstances that a condition may test.
/// `Ord` is derived because the reference keeps events in a `std::set`
/// keyed by the whole value — `delayed_quick_tmp` erases by value later, so
/// two events are the same only if every field matches.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Event {
    /// The card the event is *about*, when a single card is.
    pub trigger_card: Option<CardId>,
    /// The cards involved, for events that concern a set of them (a board
    /// wipe raises one event over many cards, not many events).
    pub event_cards: Vec<CardId>,
    /// The effect that caused this, if an effect did.
    pub reason_effect: Option<EffectId>,
    /// What kind of event this is: one of the `code` constants.
    pub event_code: u32,
    /// Kind-specific payload — for a battle, damage; for a phase, the phase.
    pub event_value: u32,
    /// Why it happened: a `REASON_*` mask in the reference.
    pub reason: u32,
    /// The player the event is *about*.
    pub event_player: u8,
    /// The player who caused it.
    pub reason_player: u8,
    /// Ordering across the duel. Two effects triggering on the same event
    /// share this, which is how simultaneous triggers are recognised as
    /// simultaneous.
    pub global_id: u32,
}

impl Event {
    /// An event of a kind, with nothing else set. Most callers fill in a
    /// field or two; the reference builds these the same way.
    pub fn new(event_code: u32) -> Self {
        Self {
            event_player: PLAYER_NONE,
            reason_player: PLAYER_NONE,
            ..Default::default()
        }
        .with_code(event_code)
    }

    fn with_code(mut self, code: u32) -> Self {
        self.event_code = code;
        self
    }
}

/// Event kinds.
///
/// Transcribed from the reference, never written from memory. Most of these
/// are small decimal numbers with no internal structure, so a wrong one
/// looks exactly as plausible as a right one and no test that does not pin
/// the literal will catch it.
///
/// Only the codes the machinery itself raises are here. Card-specific codes
/// arrive with the cards that raise them.
pub mod code {
    pub const CANNOT_SELECT_EFFECT_TARGET: u32 = 333;
    pub const FLIP: u32 = 1001;
    /// The marker for "any time a chain may be built", used by effects with
    /// no event of their own — a set trap, a quick-play.
    pub const FREE_CHAIN: u32 = 1002;

    /// The events a card's movement raises, transcribed as the contiguous
    /// block the reference declares. `LEAVE_FIELD_P` is the *pre*-leave
    /// warning raised before anything moves; `LEAVE_FIELD` is the real one.
    pub const DESTROY: u32 = 1010;
    pub const REMOVE: u32 = 1011;
    pub const TO_HAND: u32 = 1012;
    pub const TO_GRAVE: u32 = 1014;
    pub const LEAVE_FIELD: u32 = 1015;
    pub const RELEASE: u32 = 1017;
    pub const DISCARD: u32 = 1018;
    pub const LEAVE_FIELD_P: u32 = 1019;
    pub const DESTROYED: u32 = 1029;
    pub const LEAVE_GRAVE: u32 = 1031;
    pub const DETACH_MATERIAL: u32 = 1202;
    pub const TO_DECK: u32 = 1013;
    pub const CHANGE_POS: u32 = 1016;
    pub const SSET: u32 = 1107;
    pub const MSET: u32 = 1106;
    pub const MOVE: u32 = 1030;
    pub const DRAW: u32 = 1110;
    pub const SUMMON_SUCCESS: u32 = 1100;
    pub const SUMMON: u32 = 1103;
    pub const FLIP_SUMMON: u32 = 1104;
    pub const SPSUMMON: u32 = 1105;
    pub const CONTROL_CHANGED: u32 = 1120;
    pub const EQUIP: u32 = 1121;
    pub const BE_MATERIAL: u32 = 1108;
    pub const BE_PRE_MATERIAL: u32 = 1109;
    pub const FLIP_SUMMON_SUCCESS: u32 = 1101;
    pub const SPSUMMON_SUCCESS: u32 = 1102;
    pub const CHAIN_SOLVING: u32 = 1020;
    pub const CHAIN_ACTIVATING: u32 = 1021;
    pub const CHAIN_SOLVED: u32 = 1022;
    pub const CHAIN_NEGATED: u32 = 1024;
    pub const CHAIN_DISABLED: u32 = 1025;
    /// The whole chain has finished. Not one link — the stack.
    pub const CHAIN_END: u32 = 1026;
    pub const CHAINING: u32 = 1027;
    pub const BECOME_TARGET: u32 = 1028;

    /// A range, not a single code: the counter type is added to it.
    pub const ADD_COUNTER: u32 = 0x10000;
    /// Also a base: the raised event is `REMOVE_COUNTER + countertype`.
    pub const REMOVE_COUNTER: u32 = 0x20000;
    pub const ADJUST: u32 = 1040;
    pub const BREAK_EFFECT: u32 = 1050;

    /// `EFFECT_*` codes, as distinct from events. An effect's `code` is
    /// either the event it responds to (for an activated effect) or what it
    /// *is* (for a continuous one), and they share the field, so they share
    /// this module.
    pub const IMMUNE_EFFECT: u32 = 1;
    pub const DISABLE: u32 = 2;
    pub const CANNOT_DISABLE: u32 = 3;
    pub const UNIQUE_CHECK: u32 = 297;
    /// Destruction and release, a contiguous run in the reference. Note
    /// that `DESTROY_REPLACE` (50) and `RELEASE_REPLACE` (51) sit inside it
    /// and are declared above with the other replacements.
    pub const INDESTRUCTABLE: u32 = 40;
    pub const INDESTRUCTABLE_EFFECT: u32 = 41;
    pub const INDESTRUCTABLE_BATTLE: u32 = 42;
    pub const UNRELEASABLE_SUM: u32 = 43;
    pub const UNRELEASABLE_NONSUM: u32 = 44;
    pub const DESTROY_SUBSTITUTE: u32 = 45;
    pub const CANNOT_RELEASE: u32 = 46;
    pub const INDESTRUCTABLE_COUNT: u32 = 47;
    pub const UNRELEASABLE_EFFECT: u32 = 48;
    pub const TRIBUTE_LIMIT: u32 = 154;
    pub const DESTROY_REPLACE: u32 = 50;
    pub const RELEASE_REPLACE: u32 = 51;
    pub const SEND_REPLACE: u32 = 52;
    /// The two discard prohibitions, transcribed as a pair. `DiscardDeck`
    /// reads only `CANNOT_DISCARD_DECK`; taking that one alone would leave
    /// its neighbour to be written from memory later.
    pub const CANNOT_DISCARD_HAND: u32 = 55;
    pub const CANNOT_DISCARD_DECK: u32 = 56;
    pub const CANNOT_PLACE_COUNTER: u32 = 58;
    pub const DISABLE_EFFECT: u32 = 8;
    pub const DISABLE_CHAIN: u32 = 9;
    pub const CANNOT_INACTIVATE: u32 = 12;
    pub const CANNOT_DISEFFECT: u32 = 13;
    pub const CANNOT_CHANGE_POS_E: u32 = 87;
    pub const DEFENSE_ATTACK: u32 = 190;
    pub const REMAIN_FIELD: u32 = 17;
    pub const CANNOT_ACTIVATE: u32 = 6;
    pub const CANNOT_TRIGGER: u32 = 7;
    pub const TRAP_ACT_IN_HAND: u32 = 15;
    pub const TRAP_ACT_IN_SET_TURN: u32 = 16;
    pub const QP_ACT_IN_SET_TURN: u32 = 19;
    pub const CANNOT_DRAW: u32 = 25;
    pub const CANNOT_DISABLE_SUMMON: u32 = 26;
    pub const CANNOT_DISABLE_SPSUMMON: u32 = 27;
    /// The summon and set procedures, and the counts that limit them.
    pub const CANNOT_SUMMON: u32 = 20;
    pub const CANNOT_MSET: u32 = 23;
    pub const SET_SUMMON_COUNT_LIMIT: u32 = 28;
    pub const EXTRA_SUMMON_COUNT: u32 = 29;
    pub const SUMMON_PROC: u32 = 32;
    pub const LIMIT_SUMMON_PROC: u32 = 33;
    pub const EXTRA_SET_COUNT: u32 = 35;
    pub const SET_PROC: u32 = 36;
    pub const LIMIT_SET_PROC: u32 = 37;
    pub const DEVINE_LIGHT: u32 = 38;
    pub const CANNOT_DISABLE_FLIP_SUMMON: u32 = 39;
    pub const DISABLE_TRAPMONSTER: u32 = 10;
    pub const CANNOT_CHANGE_CONTROL: u32 = 5;
    pub const EQUIP_LIMIT: u32 = 76;
    pub const CANNOT_CHANGE_POSITION: u32 = 14;
    pub const MONSTER_SSET: u32 = 18;
    pub const CANNOT_SSET: u32 = 24;
    pub const SSET_COST: u32 = 95;
    /// The two ways an attack is refused, which are **not** the same thing:
    /// `CANNOT_ATTACK` is a standing prohibition, `ATTACK_DISABLED` is the
    /// per-attack one set when an attack is negated. Both are waived by
    /// `UNSTOPPABLE_ATTACK`.
    ///
    /// `ATTACK_DISABLED` is the **only** name the reference defines under
    /// both prefixes: `EFFECT_ATTACK_DISABLED` is 197 and
    /// `EVENT_ATTACK_DISABLED` is 0x476. This module flattens the two
    /// prefixes into one namespace, so the bare name is the effect and the
    /// event carries the suffix. `check_constants.py` knows about the pair
    /// by name; it cannot infer which prefix a flattened name meant.
    /// The battle event run, 1130-1143, transcribed whole. Two entries in
    /// it are **commented out in the reference** and are noted rather than
    /// defined: `EVENT_DAMAGE_CALCULATING` (1135) and `EVENT_BATTLE_END`
    /// (1137). The gaps in the numbering are those two, not omissions here.
    pub const ATTACK_ANNOUNCE: u32 = 1130;
    pub const BE_BATTLE_TARGET: u32 = 1131;
    pub const BATTLE_START: u32 = 1132;
    pub const BATTLE_CONFIRM: u32 = 1133;
    pub const PRE_DAMAGE_CALCULATE: u32 = 1134;
    pub const PRE_BATTLE_DAMAGE: u32 = 1136;
    pub const BATTLED: u32 = 1138;
    pub const BATTLE_DESTROYING: u32 = 1139;
    pub const BATTLE_DESTROYED: u32 = 1140;
    pub const DAMAGE_STEP_END: u32 = 1141;
    /// The battle-damage block, 198-208, transcribed whole. 199 does not
    /// exist; `BATTLE_DESTROY_REDIRECT` (204) is in the run and is not
    /// used by the damage calculation.
    pub const CHANGE_BATTLE_STAT: u32 = 198;
    pub const NO_BATTLE_DAMAGE: u32 = 200;
    pub const AVOID_BATTLE_DAMAGE: u32 = 201;
    pub const REFLECT_BATTLE_DAMAGE: u32 = 202;
    pub const PIERCE: u32 = 203;
    pub const BATTLE_DESTROY_REDIRECT: u32 = 204;
    pub const BATTLE_DAMAGE_TO_EFFECT: u32 = 205;
    pub const BOTH_BATTLE_DAMAGE: u32 = 206;
    pub const ALSO_BATTLE_DAMAGE: u32 = 207;
    pub const CHANGE_BATTLE_DAMAGE: u32 = 208;
    /// The toss block, 220-223, transcribed whole. `TossCoin` reads the
    /// two coin entries; the dice pair arrives with `TossDice` and is
    /// written down now so it is not written from memory then.
    pub const TOSS_COIN_REPLACE: u32 = 220;
    pub const TOSS_DICE_REPLACE: u32 = 221;
    pub const TOSS_COIN_CHOOSE: u32 = 222;
    pub const TOSS_DICE_CHOOSE: u32 = 223;
    /// The life-point block, 80-83, transcribed whole. Two of them
    /// **swap damage and recovery outright** and the other two change the
    /// amount or who takes it, which is why `Damage` and `Recover` call
    /// each other.
    pub const REVERSE_DAMAGE: u32 = 80;
    pub const REVERSE_RECOVER: u32 = 81;
    pub const CHANGE_DAMAGE: u32 = 82;
    pub const REFLECT_DAMAGE: u32 = 83;
    pub const LPCOST_CHANGE: u32 = 170;
    pub const LPCOST_REPLACE: u32 = 171;
    /// 294-297 transcribed whole; 298 and 299 do not exist.
    pub const BP_TWICE: u32 = 296;
    pub const MATCH_KILL: u32 = 300;
    /// The life-point events.
    pub const DAMAGE: u32 = 1111;
    pub const RECOVER: u32 = 1112;
    pub const BATTLE_DAMAGE: u32 = 1143;
    /// The toss event run, 1150-1153, transcribed whole — the dice half
    /// for the same reason as the effect block above.
    pub const TOSS_DICE: u32 = 1150;
    pub const TOSS_COIN: u32 = 1151;
    pub const TOSS_COIN_NEGATE: u32 = 1152;
    pub const TOSS_DICE_NEGATE: u32 = 1153;
    pub const LEVEL_UP: u32 = 1200;
    pub const PAY_LPCOST: u32 = 1201;
    /// The battle-target block, 70-74, transcribed whole. Four of the five
    /// are about *this* card as a target and one — `DIRECT_ATTACK` — is
    /// about it as an attacker, which is why they read as a jumble.
    pub const CANNOT_BE_BATTLE_TARGET: u32 = 70;
    pub const CANNOT_BE_EFFECT_TARGET: u32 = 71;
    pub const IGNORE_BATTLE_TARGET: u32 = 72;
    pub const CANNOT_DIRECT_ATTACK: u32 = 73;
    pub const DIRECT_ATTACK: u32 = 74;
    /// `CANNOT_ATTACK` and `CANNOT_ATTACK_ANNOUNCE` are **different
    /// prohibitions**: the first stops the card attacking at all, the
    /// second stops it *declaring* one. `is_capable_attack` reads the
    /// first; `is_capable_attack_announce` reads both.
    pub const CANNOT_ATTACK: u32 = 85;
    pub const CANNOT_ATTACK_ANNOUNCE: u32 = 86;
    pub const ATTACK_COST: u32 = 96;
    pub const CANNOT_SELECT_BATTLE_TARGET: u32 = 332;
    /// 343-346, taken with the two entries between them so the run is
    /// whole. `PATRICIAN_OF_DARKNESS` and `UNION_STATUS` are not used here.
    pub const ONLY_ATTACK_MONSTER: u32 = 343;
    pub const MUST_ATTACK_MONSTER: u32 = 344;
    pub const PATRICIAN_OF_DARKNESS: u32 = 345;
    pub const EXTRA_ATTACK_MONSTER: u32 = 346;
    pub const SELF_ATTACK: u32 = 406;
    pub const ATTACK_DISABLED: u32 = 197;
    pub const ATTACK_DISABLED_EVENT: u32 = 0x476;
    /// The phase block, 180-196, transcribed whole.
    ///
    /// These were first taken four at a time, as the skips alone, and the
    /// comment here used to say they were "not contiguous" — which was a
    /// statement about the partial table, not about the reference. They
    /// are contiguous. Taking the run whole is the rule, and this is what
    /// the rule is for: the gap looks like a fact about ocgcore until you
    /// transcribe the entries between.
    ///
    /// `CANNOT_M2` (186) is read by `BattleCommand` and is here ahead of
    /// it; `MUST_ATTACK_MONSTER` (344) and `ATTACK_DISABLED` (197) belong
    /// to the same subject but sit elsewhere in the reference's numbering
    /// and are left where they are.
    pub const SKIP_DP: u32 = 180;
    pub const SKIP_SP: u32 = 181;
    pub const SKIP_M1: u32 = 182;
    pub const SKIP_BP: u32 = 183;
    pub const SKIP_M2: u32 = 184;
    pub const CANNOT_BP: u32 = 185;
    pub const CANNOT_M2: u32 = 186;
    pub const CANNOT_EP: u32 = 187;
    pub const SKIP_TURN: u32 = 188;
    pub const SKIP_EP: u32 = 189;
    pub const MUST_ATTACK: u32 = 191;
    pub const FIRST_ATTACK: u32 = 192;
    pub const ATTACK_ALL: u32 = 193;
    pub const EXTRA_ATTACK: u32 = 194;
    pub const ONLY_BE_ATTACKED: u32 = 196;
    /// Out of the block but part of the same question: whether the opening
    /// turn has a Battle Phase.
    pub const BP_FIRST_TURN: u32 = 403;
    pub const HAND_LIMIT: u32 = 270;
    pub const SET_POSITION: u32 = 140;
    pub const CANNOT_LOSE_DECK: u32 = 400;
    pub const CANNOT_LOSE_LP: u32 = 401;
    pub const UNSTOPPABLE_ATTACK: u32 = 404;
    /// The special-summon permissions and limits.
    pub const CANNOT_FLIP_SUMMON: u32 = 21;
    pub const CANNOT_SPECIAL_SUMMON: u32 = 22;
    pub const SPSUMMON_CONDITION: u32 = 30;
    pub const SPSUMMON_PROC: u32 = 34;
    pub const SPSUMMON_COST: u32 = 92;
    pub const SPSUMMON_PROC_G: u32 = 320;
    /// A spirit that must not go back to the hand, and one that may
    /// choose. `proc_spirit.lua` reads both: the first stops the return
    /// outright, the second turns the mandatory trigger into an optional
    /// one.
    pub const SPIRIT_DONOT_RETURN: u32 = 280;
    pub const SPIRIT_MAYNOT_RETURN: u32 = 281;
    pub const SPSUMMON_COUNT_LIMIT: u32 = 330;
    pub const LEFT_SPSUMMON_COUNT: u32 = 331;
    pub const FORCE_SPSUMMON_POSITION: u32 = 427;
    pub const REVERSE_DECK: u32 = 294;
    pub const DECREASE_TRIBUTE: u32 = 151;
    pub const DECREASE_TRIBUTE_SET: u32 = 152;
    pub const UNSUMMONABLE_CARD: u32 = 336;
    pub const FORCE_NORMAL_SUMMON_POSITION: u32 = 426;
    pub const NORMAL_SUMMON_FACEUP_DEFENSE: u32 = 429;
    /// `EFFECT_FUSION_MATERIAL` — the procedure a Fusion Monster
    /// registers on itself so a Fusion Summon can ask what it is made of.
    pub const FUSION_MATERIAL: u32 = 230;

    pub const REVIVE_LIMIT: u32 = 31;
    pub const ADD_CODE: u32 = 113;
    pub const CHANGE_CODE: u32 = 114;
    pub const SELF_DESTROY: u32 = 141;
    pub const SELF_TOGRAVE: u32 = 142;
    pub const CANNOT_USE_AS_COST: u32 = 57;
    pub const CANNOT_TO_GRAVE_AS_COST: u32 = 59;
    /// The redirects, a contiguous run in the reference. `LEAVE_FIELD_REDIRECT`
    /// answers "instead of the graveyard, on leaving the field"; the other
    /// four answer "instead of *that* destination".
    pub const LEAVE_FIELD_REDIRECT: u32 = 60;
    pub const TO_HAND_REDIRECT: u32 = 61;
    pub const TO_DECK_REDIRECT: u32 = 62;
    pub const TO_GRAVE_REDIRECT: u32 = 63;
    pub const REMOVE_REDIRECT: u32 = 64;
    /// A *callback* redirect: rather than naming a destination, it runs an
    /// operation when the card would go to the graveyard. `SendTo` carries
    /// the fact that one applies as a flag packed into `sendto_param`.
    pub const TO_GRAVE_REDIRECT_CB: u32 = 313;
    pub const CANNOT_TURN_SET: u32 = 69;
    pub const CANNOT_TO_HAND: u32 = 65;
    pub const CANNOT_TO_DECK: u32 = 66;
    pub const CANNOT_REMOVE: u32 = 67;
    pub const CANNOT_TO_GRAVE: u32 = 68;
    pub const UPDATE_ATTACK: u32 = 100;
    pub const SET_ATTACK: u32 = 101;
    pub const SET_ATTACK_FINAL: u32 = 102;
    pub const SET_BASE_ATTACK: u32 = 103;
    pub const UPDATE_DEFENSE: u32 = 104;
    pub const SET_DEFENSE: u32 = 105;
    pub const SET_DEFENSE_FINAL: u32 = 106;
    pub const SET_BASE_DEFENSE: u32 = 107;
    pub const REVERSE_UPDATE: u32 = 108;
    pub const SWAP_AD: u32 = 109;
    pub const SWAP_BASE_AD: u32 = 110;
    pub const SWAP_ATTACK_FINAL: u32 = 111;
    pub const SWAP_DEFENSE_FINAL: u32 = 112;
    pub const ADD_RACE: u32 = 120;
    pub const REMOVE_RACE: u32 = 121;
    pub const CHANGE_RACE: u32 = 122;
    pub const ADD_ATTRIBUTE: u32 = 125;
    pub const REMOVE_ATTRIBUTE: u32 = 126;
    pub const CHANGE_ATTRIBUTE: u32 = 127;
    pub const UPDATE_LEVEL: u32 = 130;
    pub const CHANGE_LEVEL: u32 = 131;
    pub const UPDATE_RANK: u32 = 132;
    pub const CHANGE_RANK: u32 = 133;
    pub const PRE_MONSTER: u32 = 250;
    pub const CHANGE_LEVEL_FINAL: u32 = 314;
    pub const CHANGE_RANK_FINAL: u32 = 315;
    pub const ALLOW_NEGATIVE: u32 = 405;
    pub const LEVEL_RANK: u32 = 408;
    pub const RANK_LEVEL: u32 = 409;
    pub const LEVEL_RANK_S: u32 = 410;
    pub const RANK_LEVEL_S: u32 = 411;
    pub const ADD_TYPE: u32 = 115;
    pub const REMOVE_TYPE: u32 = 116;
    pub const CHANGE_TYPE: u32 = 117;
    pub const GEMINI_STATUS: u32 = 75;
    pub const GEMINI_SUMMONABLE: u32 = 77;
    pub const ACTIVATE_COST: u32 = 90;
    pub const SUMMON_COST: u32 = 91;
    pub const FLIPSUMMON_COST: u32 = 93;
    /// The tribute modifiers, a contiguous run in the reference apart from
    /// `EXTRA_RELEASE` (153) sitting between them.
    pub const DOUBLE_TRIBUTE: u32 = 150;
    pub const EXTRA_RELEASE: u32 = 153;
    pub const EXTRA_RELEASE_SUM: u32 = 155;
    pub const TRIPLE_TRIBUTE: u32 = 156;
    pub const ADD_EXTRA_TRIBUTE: u32 = 157;
    /// 158, closing the run. `SelectRelease` reads it to know which
    /// "release one of these instead" permission a chosen card came from.
    pub const EXTRA_RELEASE_NONSUM: u32 = 158;
    pub const SET_CONTROL: u32 = 4;
    pub const REMOVE_BRAINWASHING: u32 = 295;
    pub const MATERIAL_CHECK: u32 = 251;
    pub const MSET_COST: u32 = 94;
    pub const PUBLIC: u32 = 160;
    pub const FORBIDDEN: u32 = 292;
    /// A trap monster granted the use of the spell/trap seat it came from.
    pub const DISABLE_FIELD: u32 = 260;
    pub const USE_EXTRA_MZONE: u32 = 261;
    pub const USE_EXTRA_SZONE: u32 = 262;
    pub const MAX_MZONE: u32 = 263;
    pub const MAX_SZONE: u32 = 264;
    pub const MUST_USE_MZONE: u32 = 265;
    pub const QP_ACT_IN_NTPHAND: u32 = 311;
    pub const ADD_SETCODE: u32 = 334;
    pub const BECOME_QUICK: u32 = 407;
    /// Two ranges rather than single codes: a counter's code is packed into
    /// the low bits, which is why `is_can_be_forbidden` masks with
    /// `0xf0000` before comparing.
    pub const COUNTER_PERMIT: u32 = 0x10000;
    pub const COUNTER_LIMIT: u32 = 0x20000;
    /// The third of the counter-code block. Like its neighbours it is a
    /// **base**: the counter type is added to it, so the registered code
    /// is `RCOUNTER_REPLACE + countertype`, not a lookup key on its own.
    pub const RCOUNTER_REPLACE: u32 = 0x30000;

    /// Raised on entering a phase, with the phase in the low bits. These two
    /// are ranges rather than single codes, which is why the gather masks
    /// with `0xfffff000` before comparing.
    pub const PHASE: u32 = 0x1000;
    /// `EVENT_PHASE_START` — distinct from `EVENT_PHASE` above, and one bit
    /// higher. `EVENT_PHASE + p` is "this phase is happening" (what
    /// `PhaseEvent` gathers on); `EVENT_PHASE_START + p` is "this phase has
    /// just begun" (what `Turn` raises). Two different events per phase,
    /// and the names differ by one word.
    pub const PHASE_START: u32 = 0x2000;
    pub const STARTUP: u32 = 1000;
    pub const PREDRAW: u32 = 1113;
    pub const TURN_END: u32 = 1210;
    /// `EVENT_CONFIRM` (1211) and `EVENT_TOHAND_CONFIRM` (1212) — raised
    /// by `Duel.ConfirmCards` when a card is revealed, and again when the
    /// revealed card is in the hand *during* a `TO_HAND` event.
    ///
    /// Nothing in the Goat pool listens for either; they are here because
    /// `ConfirmCards` raises them and the port builds the machinery even
    /// where the pool does not reach it. In the reference they exist for
    /// Vanquish Soul Jiaolong and the Puppet King/Queen pair.
    pub const CONFIRM: u32 = 1211;
    pub const TOHAND_CONFIRM: u32 = 1212;
    pub const DRAW_COUNT: u32 = 271;
    /// The mask that recovers which family a phase code belongs to.
    pub const PHASE_MASK: u32 = 0xfffff000;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Nobody" is a real value the engine passes around, not a null.
    #[test]
    fn a_fresh_event_has_no_players() {
        let e = Event::new(code::CHAIN_END);
        assert_eq!(e.event_code, code::CHAIN_END);
        assert_eq!(e.event_player, PLAYER_NONE);
        assert_eq!(e.reason_player, PLAYER_NONE);
        assert!(e.trigger_card.is_none());
    }

    /// One event over many cards, which is how simultaneous triggers are
    /// recognised as simultaneous.
    #[test]
    fn an_event_can_concern_many_cards() {
        let mut e = Event::new(code::PHASE);
        e.event_cards = vec![1, 2, 3];
        assert_eq!(e.event_cards.len(), 3);
        assert!(e.trigger_card.is_none(), "a set, not a single subject");
    }
}

/// `CATEGORY_*` — what an effect declares it will do, as `SetOperationInfo`
/// keys it. Script-side constants (`constant.lua:714-746`), transcribed
/// whole: the core reads only `SPECIAL_SUMMON` (`0x200`), but a card names
/// its category on every activation and a missing name would be written
/// from memory.
pub mod category {
    pub const DESTROY: u64 = 0x1;
    pub const RELEASE: u64 = 0x2;
    pub const REMOVE: u64 = 0x4;
    pub const TOHAND: u64 = 0x8;
    pub const TODECK: u64 = 0x10;
    pub const TOGRAVE: u64 = 0x20;
    pub const DECKDES: u64 = 0x40;
    pub const HANDES: u64 = 0x80;
    pub const SUMMON: u64 = 0x100;
    pub const SPECIAL_SUMMON: u64 = 0x200;
    pub const TOKEN: u64 = 0x400;
    pub const FLIP: u64 = 0x800;
    pub const POSITION: u64 = 0x1000;
    pub const CONTROL: u64 = 0x2000;
    pub const DISABLE: u64 = 0x4000;
    pub const DISABLE_SUMMON: u64 = 0x8000;
    pub const DRAW: u64 = 0x10000;
    pub const SEARCH: u64 = 0x20000;
    pub const EQUIP: u64 = 0x40000;
    pub const DAMAGE: u64 = 0x80000;
    pub const RECOVER: u64 = 0x100000;
    pub const ATKCHANGE: u64 = 0x200000;
    pub const DEFCHANGE: u64 = 0x400000;
    pub const COUNTER: u64 = 0x800000;
    pub const COIN: u64 = 0x1000000;
    pub const DICE: u64 = 0x2000000;
    pub const LEAVE_GRAVE: u64 = 0x4000000;
    pub const LVCHANGE: u64 = 0x8000000;
    pub const NEGATE: u64 = 0x10000000;
    pub const ANNOUNCE: u64 = 0x20000000;
    pub const FUSION_SUMMON: u64 = 0x40000000;
    pub const TOEXTRA: u64 = 0x80000000;
    pub const SET: u64 = 0x100000000;
}

/// `CHAININFO` — the fields of a chain link a script may ask for
/// (`field.h:726`, an `enum class` counting from 1). Transcribed whole.
pub mod chain_info {
    pub const TRIGGERING_EFFECT: u8 = 1;
    pub const TRIGGERING_PLAYER: u8 = 2;
    pub const TRIGGERING_CONTROLER: u8 = 3;
    pub const TRIGGERING_LOCATION: u8 = 4;
    pub const TRIGGERING_LOCATION_SYMBOLIC: u8 = 5;
    pub const TRIGGERING_SEQUENCE: u8 = 6;
    pub const TRIGGERING_SEQUENCE_SYMBOLIC: u8 = 7;
    pub const TARGET_CARDS: u8 = 8;
    pub const TARGET_PLAYER: u8 = 9;
    pub const TARGET_PARAM: u8 = 10;
    pub const DISABLE_REASON: u8 = 11;
    pub const DISABLE_PLAYER: u8 = 12;
    pub const CHAIN_ID: u8 = 13;
    pub const TYPE: u8 = 14;
    pub const EXTTYPE: u8 = 15;
    pub const TRIGGERING_POSITION: u8 = 16;
    pub const TRIGGERING_CODE: u8 = 17;
    pub const TRIGGERING_CODE2: u8 = 18;
    pub const TRIGGERING_TYPE: u8 = 19;
    pub const TRIGGERING_LEVEL: u8 = 20;
    pub const TRIGGERING_RANK: u8 = 21;
    pub const TRIGGERING_ATTRIBUTE: u8 = 22;
    pub const TRIGGERING_RACE: u8 = 23;
    pub const TRIGGERING_ATTACK: u8 = 24;
    pub const TRIGGERING_DEFENSE: u8 = 25;
    pub const TRIGGERING_STATUS: u8 = 26;
    pub const TRIGGERING_SUMMON_LOCATION: u8 = 27;
    pub const TRIGGERING_SUMMON_TYPE: u8 = 28;
    pub const TRIGGERING_SUMMON_PROC_COMPLETE: u8 = 29;
    pub const TRIGGERING_SETCODES: u8 = 30;
}
