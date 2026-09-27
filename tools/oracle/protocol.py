"""ocgcore wire protocol: constants, message parsers, response builders.

Layouts transcribed from the pinned edo9300/ygopro-core sources (see
versions.txt): message construction in playerop.cpp, response parsing via
ProgressiveBuffer in the same file, query buffers in ocgapi.cpp/card.cpp.
Everything here is pure and unit-testable without the core library.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass, field

# -- process statuses --------------------------------------------------------
PROCESSOR_END = 0
PROCESSOR_WAITING = 1
PROCESSOR_CONTINUE = 2

# -- locations / positions ---------------------------------------------------
LOCATION_DECK = 0x01
LOCATION_EXTRA = 0x40
LOCATION_HAND = 0x02
LOCATION_MZONE = 0x04
LOCATION_SZONE = 0x08
LOCATION_GRAVE = 0x10

POS_FACEUP_ATTACK = 0x1
POS_FACEDOWN_ATTACK = 0x2
POS_FACEUP_DEFENSE = 0x4
POS_FACEDOWN_DEFENSE = 0x8

# -- card types / attributes / races ----------------------------------------
TYPE_MONSTER = 0x1
TYPE_SPELL = 0x2
TYPE_TRAP = 0x4
TYPE_NORMAL = 0x10
TYPE_EFFECT = 0x20
TYPE_QUICKPLAY = 0x10000
TYPE_FLIP = 0x200000
TYPE_TOKEN = 0x4000
TYPE_FUSION = 0x40
TYPE_SPIRIT = 0x200
TYPE_SPSUMMON = 0x2000000  # cannot be Normal Summoned/Set (a nomi monster)
TYPE_CONTINUOUS = 0x20000
TYPE_EQUIP = 0x40000

ATTRIBUTE_EARTH = 0x01
ATTRIBUTE_WIND = 0x08
ATTRIBUTE_WATER = 0x02
ATTRIBUTE_LIGHT = 0x10
ATTRIBUTE_DARK = 0x20

RACE_SPELLCASTER = 0x2
RACE_FIEND = 0x8
RACE_WARRIOR = 0x1
RACE_FAIRY = 0x4
RACE_MACHINE = 0x20
RACE_ZOMBIE = 0x10
RACE_ROCK = 0x100
RACE_INSECT = 0x800
RACE_REPTILE = 0x80000
RACE_AQUA = 0x40
RACE_PLANT = 0x1000
RACE_DRAGON = 0x2000
RACE_BEAST = 0x4000
RACE_BEASTWARRIOR = 0x8000
RACE_DINOSAUR = 0x10000

# -- duel option flags -------------------------------------------------------
DUEL_PSEUDO_SHUFFLE = 0x10
DUEL_PZONE = 0x800
DUEL_EMZONE = 0x2000
DUEL_FSX_MMZONE = 0x4000
DUEL_TRAP_MONSTERS_NOT_USE_ZONE = 0x8000
DUEL_TRIGGER_ONLY_IN_LOCATION = 0x20000
DUEL_MODE_MR5 = (
    DUEL_PZONE
    | DUEL_EMZONE
    | DUEL_FSX_MMZONE
    | DUEL_TRAP_MONSTERS_NOT_USE_ZONE
    | DUEL_TRIGGER_ONLY_IN_LOCATION
)
# The adapter's standard flags: current Master Rule zones/timing rules, with
# deterministic deck order so insertion order IS draw order (no shuffling).
# MR5 notably excludes DUEL_1ST_TURN_DRAW and DUEL_ATTACK_FIRST_TURN, matching
# our engine's "no draw, no battle on the first player's first turn".
# Master Rule 5 minus the Extra Monster Zone: the Goat-era board has five
# Monster Zones, and a Fusion Monster (Metamorphosis) takes a main one -
# with all five full, ocgcore under MR5 would summon it to the Extra
# Monster Zone instead (fuzz-found), a zone the engine does not model.
ORACLE_DUEL_FLAGS = (DUEL_MODE_MR5 & ~DUEL_EMZONE) | DUEL_PSEUDO_SHUFFLE

# -- messages ----------------------------------------------------------------
MSG_RETRY = 1
MSG_HINT = 2
MSG_WIN = 5
MSG_SELECT_BATTLECMD = 10
MSG_SELECT_IDLECMD = 11
MSG_SELECT_EFFECTYN = 12
MSG_SELECT_YESNO = 13
MSG_SELECT_OPTION = 14
MSG_SELECT_CARD = 15
MSG_SELECT_CHAIN = 16
MSG_SELECT_PLACE = 18
MSG_SELECT_POSITION = 19
MSG_SELECT_TRIBUTE = 20
MSG_SELECT_UNSELECT_CARD = 26
MSG_ANNOUNCE_RACE = 140
MSG_CONFIRM_DECKTOP = 30
MSG_CONFIRM_CARDS = 31
MSG_SHUFFLE_DECK = 32
MSG_SHUFFLE_HAND = 33
MSG_NEW_TURN = 40
MSG_NEW_PHASE = 41
MSG_MOVE = 50
MSG_RANDOM_SELECTED = 81
MSG_TOSS_COIN = 130
MSG_POS_CHANGE = 53
MSG_SET = 54
MSG_SUMMONING = 60
MSG_SUMMONED = 61
MSG_SPSUMMONING = 62
MSG_SPSUMMONED = 63
MSG_FLIPSUMMONING = 64
MSG_FLIPSUMMONED = 65
MSG_CHAINING = 70
MSG_CHAINED = 71
MSG_CHAIN_SOLVING = 72
MSG_CHAIN_SOLVED = 73
MSG_CHAIN_END = 74
MSG_DRAW = 90
MSG_DAMAGE = 91
MSG_ATTACK = 110
MSG_BATTLE = 111
MSG_DAMAGE_STEP_START = 113
MSG_DAMAGE_STEP_END = 114

MSG_NAMES = {
    v: k for k, v in list(globals().items()) if k.startswith("MSG_") and isinstance(v, int)
}

# -- query flags -------------------------------------------------------------
QUERY_CODE = 0x1
QUERY_POSITION = 0x2
QUERY_IS_PUBLIC = 0x100000
QUERY_END = 0x80000000


class _Reader:
    """Little-endian cursor over a message payload."""

    def __init__(self, data: bytes) -> None:
        self.data = data
        self.off = 0

    def u8(self) -> int:
        (v,) = struct.unpack_from("<B", self.data, self.off)
        self.off += 1
        return int(v)

    def u16(self) -> int:
        (v,) = struct.unpack_from("<H", self.data, self.off)
        self.off += 2
        return int(v)

    def u32(self) -> int:
        (v,) = struct.unpack_from("<I", self.data, self.off)
        self.off += 4
        return int(v)

    def i32(self) -> int:
        (v,) = struct.unpack_from("<i", self.data, self.off)
        self.off += 4
        return int(v)

    def u64(self) -> int:
        (v,) = struct.unpack_from("<Q", self.data, self.off)
        self.off += 8
        return int(v)

    def loc_info(self) -> LocInfo:
        return LocInfo(self.u8(), self.u8(), self.u32(), self.u32())


@dataclass(frozen=True)
class LocInfo:
    controller: int
    location: int
    sequence: int
    position: int


@dataclass(frozen=True)
class CmdEntry:
    """One selectable card in an idle/battle/chain/card list."""

    code: int
    controller: int
    location: int
    sequence: int
    extra: int = 0  # description low bits / direct_attackable / release_param


@dataclass(frozen=True)
class IdleCmd:
    player: int
    summonable: tuple[CmdEntry, ...]
    spsummonable: tuple[CmdEntry, ...]
    repositionable: tuple[CmdEntry, ...]
    msetable: tuple[CmdEntry, ...]
    ssetable: tuple[CmdEntry, ...]
    activatable: tuple[CmdEntry, ...]
    to_bp: bool
    to_ep: bool
    can_shuffle: bool


@dataclass(frozen=True)
class BattleCmd:
    player: int
    activatable: tuple[CmdEntry, ...]
    attackable: tuple[CmdEntry, ...]  # extra = direct_attackable
    to_m2: bool
    to_ep: bool


@dataclass(frozen=True)
class SelectCard:
    player: int
    cancelable: bool
    min: int
    max: int
    cards: tuple[CmdEntry, ...]  # sequence/location from loc_info


@dataclass(frozen=True)
class SelectTribute:
    player: int
    cancelable: bool
    min: int
    max: int
    cards: tuple[CmdEntry, ...]  # extra = release_param


@dataclass(frozen=True)
class SelectUnselect:
    player: int
    finishable: bool
    cancelable: bool
    min: int
    max: int
    cards: tuple[CmdEntry, ...]  # selectable, then unselectable


@dataclass(frozen=True)
class SelectChain:
    player: int
    spe_count: int
    forced: bool
    chains: tuple[CmdEntry, ...]
    timing: int = 0  # ocgcore's hint timing bits for the asked player (census.TIMINGS)
    timing_other: int = 0  # ... and for the other player


@dataclass(frozen=True)
class SelectPlace:
    player: int
    count: int
    flag: int  # set bits are FORBIDDEN zones


@dataclass(frozen=True)
class SelectPosition:
    player: int
    code: int
    positions: int


@dataclass(frozen=True)
class AnnounceRace:
    """Declare `count` monster Types among the `available` race bitmask
    (Tribe-Infecting Virus)."""

    player: int
    count: int
    available: int


@dataclass(frozen=True)
class SelectYesNo:
    player: int
    description: int
    code: int = 0  # EFFECTYN carries the prompting card


@dataclass(frozen=True)
class SelectOption:
    player: int
    options: tuple[int, ...]


@dataclass(frozen=True)
class MoveMsg:
    code: int
    prev: LocInfo
    cur: LocInfo
    reason: int


@dataclass(frozen=True)
class DrawMsg:
    player: int
    codes: tuple[int, ...]


@dataclass(frozen=True)
class CoinMsg:
    """ocgcore tossed coins for `player`: results, 1 = heads."""

    player: int
    results: tuple[int, ...]


@dataclass(frozen=True)
class ConfirmDecktopMsg:
    """ocgcore revealed the top `codes` of `player`'s deck, top first
    (an excavation: Magical Merchant)."""

    player: int
    codes: tuple[int, ...]


@dataclass(frozen=True)
class WinMsg:
    player: int  # 0/1 winner, 2 = draw
    reason: int


@dataclass(frozen=True)
class InfoMsg:
    """Any message we track but do not act on."""

    msg_id: int
    raw: bytes = field(repr=False, default=b"")


ParsedMsg = (
    AnnounceRace
    | IdleCmd
    | BattleCmd
    | SelectCard
    | SelectUnselect
    | SelectTribute
    | SelectChain
    | SelectPlace
    | SelectPosition
    | SelectYesNo
    | SelectOption
    | DrawMsg
    | MoveMsg
    | WinMsg
    | InfoMsg
    | CoinMsg
    | ConfirmDecktopMsg
)


def _entries(
    r: _Reader, n: int, seq_width: int, desc: bool = False, trailer_u8: bool = False
) -> tuple[CmdEntry, ...]:
    out = []
    for _ in range(n):
        code = r.u32()
        con = r.u8()
        loc = r.u8()
        seq = r.u32() if seq_width == 4 else r.u8()
        extra = 0
        if desc:
            extra = r.u64()
            r.u8()  # client mode
        if trailer_u8:
            extra = r.u8()
        out.append(CmdEntry(code, con, loc, seq, extra))
    return tuple(out)


def _loc_entries(r: _Reader, n: int) -> tuple[CmdEntry, ...]:
    out = []
    for _ in range(n):
        code = r.u32()
        info = r.loc_info()
        out.append(CmdEntry(code, info.controller, info.location, info.sequence, info.position))
    return tuple(out)


def parse_message(msg: bytes) -> ParsedMsg:
    """Parse one length-stripped message (first byte is the message id)."""
    mid = msg[0]
    r = _Reader(msg[1:])
    if mid == MSG_SELECT_IDLECMD:
        player = r.u8()
        summon = _entries(r, r.u32(), 4)
        sp = _entries(r, r.u32(), 4)
        repos = _entries(r, r.u32(), 1)  # sequence is u8 here (playerop.cpp)
        mset = _entries(r, r.u32(), 4)
        sset = _entries(r, r.u32(), 4)
        act = _entries(r, r.u32(), 4, desc=True)
        return IdleCmd(
            player,
            summon,
            sp,
            repos,
            mset,
            sset,
            act,
            to_bp=bool(r.u8()),
            to_ep=bool(r.u8()),
            can_shuffle=bool(r.u8()),
        )
    if mid == MSG_SELECT_BATTLECMD:
        player = r.u8()
        act = _entries(r, r.u32(), 4, desc=True)
        attack = _entries(r, r.u32(), 1, trailer_u8=True)
        return BattleCmd(player, act, attack, to_m2=bool(r.u8()), to_ep=bool(r.u8()))
    if mid == MSG_SELECT_CARD:
        player = r.u8()
        cancelable = bool(r.u8())
        mn, mx, n = r.u32(), r.u32(), r.u32()
        return SelectCard(player, cancelable, mn, mx, _loc_entries(r, n))
    if mid == MSG_SELECT_TRIBUTE:
        player = r.u8()
        cancelable = bool(r.u8())
        mn, mx, n = r.u32(), r.u32(), r.u32()
        return SelectTribute(player, cancelable, mn, mx, _entries(r, n, 4, trailer_u8=True))
    if mid == MSG_SELECT_UNSELECT_CARD:
        player = r.u8()
        finishable = bool(r.u8())
        cancelable = bool(r.u8())
        mn, mx = r.u32(), r.u32()
        selectable = _loc_entries(r, r.u32())
        unselectable = _loc_entries(r, r.u32())
        return SelectUnselect(player, finishable, cancelable, mn, mx, selectable + unselectable)
    if mid == MSG_SELECT_CHAIN:
        player = r.u8()
        spe = r.u8()
        forced = bool(r.u8())
        timing = r.u32()  # hint timing (self)
        timing_other = r.u32()  # hint timing (other)
        n = r.u32()
        chains = []
        for _ in range(n):
            code = r.u32()
            info = r.loc_info()
            desc = r.u64()
            r.u8()  # client mode
            chains.append(CmdEntry(code, info.controller, info.location, info.sequence, desc))
        return SelectChain(player, spe, forced, tuple(chains), timing, timing_other)
    if mid == MSG_SELECT_PLACE:
        return SelectPlace(r.u8(), r.u8(), r.u32())
    if mid == MSG_SELECT_POSITION:
        return SelectPosition(r.u8(), r.u32(), r.u8())
    if mid == MSG_SELECT_EFFECTYN:
        player = r.u8()
        code = r.u32()
        r.loc_info()
        return SelectYesNo(player, r.u64(), code=code)
    if mid == MSG_SELECT_YESNO:
        player = r.u8()
        return SelectYesNo(player, r.u64())
    if mid == MSG_SELECT_OPTION:
        player = r.u8()
        n = r.u8()
        return SelectOption(player, tuple(r.u64() for _ in range(n)))
    if mid == MSG_ANNOUNCE_RACE:
        player = r.u8()
        count = r.u8()
        return AnnounceRace(player, count, r.u64())
    if mid == MSG_MOVE:
        code = r.u32()
        prev = r.loc_info()
        cur = r.loc_info()
        return MoveMsg(code, prev, cur, r.u32())
    if mid == MSG_DRAW:
        player = r.u8()
        n = r.u32()
        codes = []
        for _ in range(n):
            codes.append(r.u32())
            r.u32()  # position
        return DrawMsg(player, tuple(codes))
    if mid == MSG_WIN:
        return WinMsg(r.u8(), r.u8())
    if mid == MSG_TOSS_COIN:
        player = r.u8()
        n = r.u8()
        return CoinMsg(player, tuple(r.u8() for _ in range(n)))
    if mid == MSG_CONFIRM_DECKTOP:
        player = r.u8()
        n = r.u32()
        codes = []
        for _ in range(n):
            codes.append(r.u32())
            r.u8()  # controller
            r.u8()  # location
            r.u32()  # sequence
        return ConfirmDecktopMsg(player, tuple(codes))
    return InfoMsg(mid, msg)


REQUEST_TYPES = (
    AnnounceRace,
    IdleCmd,
    BattleCmd,
    SelectCard,
    SelectUnselect,
    SelectTribute,
    SelectChain,
    SelectPlace,
    SelectPosition,
    SelectYesNo,
    SelectOption,
)


def split_messages(buf: bytes) -> list[bytes]:
    """Split the OCG_DuelGetMessage buffer into length-stripped messages."""
    out = []
    off = 0
    while off < len(buf):
        (size,) = struct.unpack_from("<I", buf, off)
        off += 4
        out.append(buf[off : off + size])
        off += size
    return out


# -- response builders -------------------------------------------------------


def respond_index_pair(t: int, s: int) -> bytes:
    """IDLECMD / BATTLECMD: packed command type + list index."""
    return struct.pack("<i", (t & 0xFFFF) | (s << 16))


def respond_u64(v: int) -> bytes:
    """ANNOUNCE_RACE: the selected race bits."""
    return struct.pack("<Q", v)


def respond_int(v: int) -> bytes:
    """CHAIN (index or -1) / POSITION (mask) / YESNO / OPTION."""
    return struct.pack("<i", v)


def respond_cards(indices: list[int]) -> bytes:
    """SELECT_CARD / SELECT_TRIBUTE: type-0 index list."""
    return struct.pack(f"<iI{len(indices)}I", 0, len(indices), *indices)


def respond_unselect(index: int) -> bytes:
    """SELECT_UNSELECT_CARD: pick one card by combined-list index."""
    return struct.pack("<ii", 1, index)


def respond_place(player: int, location: int, sequence: int) -> bytes:
    return struct.pack("<BBB", player, location, sequence)


def lowest_allowed_place(flag: int, player: int, location: int) -> int:
    """Lowest allowed sequence in SELECT_PLACE's forbidden-bit flag.

    Bit layout (playerop.cpp): for the requested player, bits 0-6 are the
    main monster zones and bits 8-12 the spell/trap zones; the opponent's
    zones sit 16 bits higher. Set bits mark forbidden zones.
    """
    allowed = ~flag
    shift = 0 if player == 0 else 16
    if location == LOCATION_MZONE:  # noqa: SIM108 - bit layouts read better spelled out
        bits = (allowed >> shift) & 0x7F
    else:
        bits = (allowed >> (shift + 8)) & 0x1F
    for seq in range(8):
        if bits & (1 << seq):
            return seq
    raise ValueError(f"no allowed zone in flag {flag:#x}")
