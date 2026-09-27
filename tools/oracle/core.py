"""ctypes bindings for ocgcore and a thin in-process duel driver.

Crash/hang containment happens one level up (tools/oracle/runner.py runs a
whole game diff in a disposable child process); this module assumes a live,
same-process core, exactly like the spike harness it grew from.
"""

from __future__ import annotations

import ctypes
import os
import re
import struct
import sys
from dataclasses import dataclass
from pathlib import Path

from tools.oracle import protocol as p
from tools.oracle.carddata import CARDS

ORACLE_DIR = Path(__file__).parent
# build.sh names the library by platform: a dylib on macOS, a .so elsewhere.
DEFAULT_LIB = (
    ORACLE_DIR / "build" / ("libocgcore.dylib" if sys.platform == "darwin" else "libocgcore.so")
)
DEFAULT_SCRIPTS = ORACLE_DIR / "build" / "cardscripts"


def lib_path() -> Path:
    return Path(os.environ.get("ORACLE_LIB", DEFAULT_LIB))


def scripts_path() -> Path:
    return Path(os.environ.get("ORACLE_SCRIPTS", DEFAULT_SCRIPTS))


_CARD_SCRIPT = re.compile(r"^c\d+\.lua$")

# Appended to every card script when script coverage is on. It wraps each
# function the script defines so the first call reports itself, and reports
# the full set at load time; both go out through `Debug.Message`, which
# ocgcore already routes to our log handler. Nothing in ocgcore changes, and
# the original script text is untouched - this is appended after it.
SCRIPT_COVERAGE_EPILOGUE = b"""
;(function()
    local t, code = self_table, self_code
    if type(t) ~= "table" or type(Debug) ~= "table" then return end
    local seen = {}
    local names = {}
    for k, v in pairs(t) do
        if type(v) == "function" and type(k) == "string" then names[#names + 1] = k end
    end
    for _, k in ipairs(names) do
        local f = t[k]
        Debug.Message("SDEF\t" .. code .. "\t" .. k)
        t[k] = function(...)
            if not seen[k] then
                seen[k] = true
                Debug.Message("SCOV\t" .. code .. "\t" .. k)
            end
            return f(...)
        end
    end
end)()
"""


def _script_coverage_out() -> str:
    """Where per-script function coverage is appended, or empty when off."""
    return os.environ.get("ORACLE_SCRIPT_COVERAGE", "")


def _record_script_coverage(text: str) -> None:
    """One short line per event, appended by every duel process to the same
    file (short appends do not interleave), so a fuzz stage's worth of
    contained children collect into one place like the profile pool."""
    path = _script_coverage_out()
    if not path:
        return
    with open(path, "a", encoding="utf-8") as fh:
        fh.write(text.rstrip("\n") + "\n")


def oracle_available() -> bool:
    return lib_path().exists() and (scripts_path() / "constant.lua").exists()


class _CardData(ctypes.Structure):
    _fields_ = [
        ("code", ctypes.c_uint32),
        ("alias", ctypes.c_uint32),
        ("setcodes", ctypes.POINTER(ctypes.c_uint16)),
        ("type", ctypes.c_uint32),
        ("level", ctypes.c_uint32),
        ("attribute", ctypes.c_uint32),
        ("race", ctypes.c_uint64),
        ("attack", ctypes.c_int32),
        ("defense", ctypes.c_int32),
        ("lscale", ctypes.c_uint32),
        ("rscale", ctypes.c_uint32),
        ("link_marker", ctypes.c_uint32),
    ]


class _Player(ctypes.Structure):
    _fields_ = [
        ("startingLP", ctypes.c_uint32),
        ("startingDrawCount", ctypes.c_uint32),
        ("drawCountPerTurn", ctypes.c_uint32),
    ]


OCG_LOG_TYPE_ERROR = 0  # ocgapi_types.h: ERROR, FROM_SCRIPT, FOR_DEBUG, UNDEFINED

_DataReader = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_uint32, ctypes.POINTER(_CardData))
_DataReaderDone = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.POINTER(_CardData))
_ScriptReader = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_char_p)
_LogHandler = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int)


class _DuelOptions(ctypes.Structure):
    _fields_ = [
        ("seed", ctypes.c_uint64 * 4),
        ("flags", ctypes.c_uint64),
        ("team1", _Player),
        ("team2", _Player),
        ("cardReader", _DataReader),
        ("payload1", ctypes.c_void_p),
        ("scriptReader", _ScriptReader),
        ("payload2", ctypes.c_void_p),
        ("logHandler", _LogHandler),
        ("payload3", ctypes.c_void_p),
        ("cardReaderDone", _DataReaderDone),
        ("payload4", ctypes.c_void_p),
        ("enableUnsafeLibraries", ctypes.c_uint8),
    ]


class _NewCardInfo(ctypes.Structure):
    _fields_ = [
        ("team", ctypes.c_uint8),
        ("duelist", ctypes.c_uint8),
        ("code", ctypes.c_uint32),
        ("con", ctypes.c_uint8),
        ("loc", ctypes.c_uint32),
        ("seq", ctypes.c_uint32),
        ("pos", ctypes.c_uint32),
    ]


class _QueryInfo(ctypes.Structure):
    _fields_ = [
        ("flags", ctypes.c_uint32),
        ("con", ctypes.c_uint8),
        ("loc", ctypes.c_uint32),
        ("seq", ctypes.c_uint32),
        ("overlay_seq", ctypes.c_uint32),
    ]


@dataclass(frozen=True)
class SlotQuery:
    """Per-card query result (None-valued entries mean an empty slot)."""

    code: int
    position: int
    is_public: bool


@dataclass(frozen=True)
class FieldSnapshot:
    lp: tuple[int, int]
    mzone_occupied: tuple[tuple[int | None, ...], tuple[int | None, ...]]  # positions
    szone_occupied: tuple[tuple[int | None, ...], tuple[int | None, ...]]
    deck: tuple[int, int]
    hand: tuple[int, int]
    grave: tuple[int, int]


class OcgError(RuntimeError):
    pass


_lib: ctypes.CDLL | None = None


def _load() -> ctypes.CDLL:
    global _lib
    if _lib is None:
        lib = ctypes.CDLL(str(lib_path()))
        lib.OCG_CreateDuel.argtypes = [
            ctypes.POINTER(ctypes.c_void_p),
            ctypes.POINTER(_DuelOptions),
        ]
        lib.OCG_DestroyDuel.argtypes = [ctypes.c_void_p]
        lib.OCG_DuelNewCard.argtypes = [ctypes.c_void_p, ctypes.POINTER(_NewCardInfo)]
        lib.OCG_StartDuel.argtypes = [ctypes.c_void_p]
        lib.OCG_DuelProcess.argtypes = [ctypes.c_void_p]
        lib.OCG_DuelGetMessage.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint32)]
        lib.OCG_DuelGetMessage.restype = ctypes.c_void_p
        lib.OCG_DuelSetResponse.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32]
        lib.OCG_LoadScript.argtypes = [
            ctypes.c_void_p,
            ctypes.c_char_p,
            ctypes.c_uint32,
            ctypes.c_char_p,
        ]
        lib.OCG_LoadScript.restype = ctypes.c_int
        lib.OCG_DuelQuery.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_uint32),
            ctypes.POINTER(_QueryInfo),
        ]
        lib.OCG_DuelQuery.restype = ctypes.c_void_p
        lib.OCG_DuelQueryLocation.argtypes = lib.OCG_DuelQuery.argtypes
        lib.OCG_DuelQueryLocation.restype = ctypes.c_void_p
        lib.OCG_DuelQueryField.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint32)]
        lib.OCG_DuelQueryField.restype = ctypes.c_void_p
        _lib = lib
    return _lib


class OcgDuel:
    """One ocgcore duel with stacked (pseudo-shuffled) decks."""

    def __init__(
        self,
        decks: tuple[list[int], list[int]],
        start_lp: int = 8000,
        opening_hand: int = 5,
        flags: int = p.ORACLE_DUEL_FLAGS,
        seed: tuple[int, int, int, int] = (1, 2, 3, 4),
    ) -> None:
        lib = _load()
        self._lib = lib
        self._log: list[str] = []
        self._errors: list[str] = []
        scripts = scripts_path()
        cache: dict[str, bytes | None] = {}

        def read_script(name: str) -> bytes | None:
            if name not in cache:
                path = scripts / Path(name).name
                content = path.read_bytes() if path.exists() else None
                if content is not None and _script_coverage_out() and _CARD_SCRIPT.match(path.name):
                    content = content + SCRIPT_COVERAGE_EPILOGUE
                cache[name] = content
            return cache[name]

        @_DataReader  # type: ignore[untyped-decorator, misc, unused-ignore]
        def card_reader(_payload: object, code: int, data: object) -> None:
            d = data.contents  # type: ignore[attr-defined]
            card = CARDS.get(code)
            d.code = code
            d.alias = 0
            d.setcodes = None
            if card is None:  # unknown card: harmless vanilla statline
                d.type, d.level, d.attribute, d.race = 0x11, 4, 1, 1
                d.attack = d.defense = 1000
            else:
                d.type = card.type
                d.level = card.level
                d.attribute = card.attribute
                d.race = card.race
                d.attack = card.attack
                d.defense = card.defense
            d.lscale = d.rscale = d.link_marker = 0

        @_DataReaderDone  # type: ignore[untyped-decorator, misc, unused-ignore]
        def card_reader_done(_payload: object, _data: object) -> None:
            pass

        @_ScriptReader  # type: ignore[untyped-decorator, misc, unused-ignore]
        def script_reader(_payload: object, duel: int, name: bytes) -> int:
            content = read_script(name.decode())
            if content is None:
                return 0
            return int(lib.OCG_LoadScript(ctypes.c_void_p(duel), content, len(content), name))

        @_LogHandler  # type: ignore[untyped-decorator, misc, unused-ignore]
        def log_handler(_payload: object, string: bytes, typ: int) -> None:
            text = string.decode(errors="replace")
            if text.startswith(("SDEF\t", "SCOV\t")):
                _record_script_coverage(text)
                return
            self._log.append(text)
            if typ == OCG_LOG_TYPE_ERROR:
                self._errors.append(text)

        self._callbacks = (card_reader, card_reader_done, script_reader, log_handler)
        opts = _DuelOptions()
        # Not unused: `DUEL_PSEUDO_SHUFFLE` only settles the *piles*, and
        # anything ocgcore rolls itself — a coin, a die, a random discard,
        # an excavation — still draws from this generator. A differential
        # run has to seed both engines alike, so it is a parameter.
        opts.seed = (ctypes.c_uint64 * 4)(*seed)
        opts.flags = flags
        for team in (opts.team1, opts.team2):
            team.startingLP = start_lp
            team.startingDrawCount = opening_hand
            team.drawCountPerTurn = 1
        opts.cardReader = card_reader
        opts.cardReaderDone = card_reader_done
        opts.scriptReader = script_reader
        opts.logHandler = log_handler
        opts.enableUnsafeLibraries = 0
        duel = ctypes.c_void_p()
        rc = lib.OCG_CreateDuel(ctypes.byref(duel), ctypes.byref(opts))
        if rc != 0:
            raise OcgError(f"OCG_CreateDuel failed: {rc}")
        self._duel = duel
        for name in (b"constant.lua", b"utility.lua"):
            content = read_script(name.decode())
            if content is None or not lib.OCG_LoadScript(duel, content, len(content), name):
                raise OcgError(f"failed to load base script {name.decode()}")
        info = _NewCardInfo()
        for team, deck in enumerate(decks):
            for code in deck:
                info.team = team
                info.duelist = 0
                info.code = code
                info.con = team
                # A decklist carries its Extra Deck: fusions go there.
                fusion = code in CARDS and CARDS[code].type & p.TYPE_FUSION
                info.loc = p.LOCATION_EXTRA if fusion else p.LOCATION_DECK
                info.seq = 0
                info.pos = p.POS_FACEDOWN_DEFENSE
                lib.OCG_DuelNewCard(duel, ctypes.byref(info))
        lib.OCG_StartDuel(duel)
        # A card script that fails to load leaves the card a vanilla in
        # ocgcore, silently: surface it as a hard error instead.
        errors = self.take_errors()
        if errors:
            raise OcgError("ocgcore script errors at duel start: " + " | ".join(errors))

    @property
    def log(self) -> list[str]:
        return self._log

    def take_errors(self) -> list[str]:
        """Drain the Lua error messages logged since the last call."""
        errors, self._errors = self._errors, []
        return errors

    def process(self) -> int:
        return int(self._lib.OCG_DuelProcess(self._duel))

    def messages(self) -> list[bytes]:
        length = ctypes.c_uint32()
        ptr = self._lib.OCG_DuelGetMessage(self._duel, ctypes.byref(length))
        if not ptr or length.value == 0:
            return []
        return p.split_messages(ctypes.string_at(ptr, length.value))

    def respond(self, response: bytes) -> None:
        self._lib.OCG_DuelSetResponse(self._duel, response, len(response))

    def query_field(self) -> FieldSnapshot:
        length = ctypes.c_uint32()
        ptr = self._lib.OCG_DuelQueryField(self._duel, ctypes.byref(length))
        buf = ctypes.string_at(ptr, length.value)
        off = 4  # duel_options u32
        lp: list[int] = []
        mzones: list[tuple[int | None, ...]] = []
        szones: list[tuple[int | None, ...]] = []
        deck: list[int] = []
        hand: list[int] = []
        grave: list[int] = []
        for _ in range(2):
            (lp_val,) = struct.unpack_from("<I", buf, off)
            off += 4
            lp.append(lp_val)
            zones: list[list[int | None]] = [[], []]
            for zi, count in ((0, 7), (1, 8)):
                for _slot in range(count):
                    present = buf[off]
                    off += 1
                    if present:
                        zones[zi].append(buf[off])
                        off += 5  # position u8 + materials u32
                    else:
                        zones[zi].append(None)
            mzones.append(tuple(zones[0]))
            szones.append(tuple(zones[1]))
            counts = struct.unpack_from("<6I", buf, off)
            off += 24
            deck.append(counts[0])
            hand.append(counts[1])
            grave.append(counts[2])
        return FieldSnapshot(
            lp=(lp[0], lp[1]),
            mzone_occupied=(mzones[0], mzones[1]),
            szone_occupied=(szones[0], szones[1]),
            deck=(deck[0], deck[1]),
            hand=(hand[0], hand[1]),
            grave=(grave[0], grave[1]),
        )

    def query_location(self, controller: int, location: int) -> list[SlotQuery | None]:
        info = _QueryInfo()
        info.flags = p.QUERY_CODE | p.QUERY_POSITION
        info.con = controller
        info.loc = location
        info.seq = 0
        info.overlay_seq = 0
        length = ctypes.c_uint32()
        ptr = self._lib.OCG_DuelQueryLocation(self._duel, ctypes.byref(length), ctypes.byref(info))
        buf = ctypes.string_at(ptr, length.value) if ptr and length.value else b""
        out: list[SlotQuery | None] = []
        off = 4  # leading u32 total size
        while off < len(buf):
            (size,) = struct.unpack_from("<H", buf, off)
            if size == 0:  # empty slot marker (int16 0)
                out.append(None)
                off += 2
                continue
            code = position = 0
            public = False
            while True:
                (size,) = struct.unpack_from("<H", buf, off)
                off += 2
                (flag,) = struct.unpack_from("<I", buf, off)
                payload = buf[off + 4 : off + size]
                off += size
                if flag == p.QUERY_END:
                    break
                if flag == p.QUERY_CODE:
                    (code,) = struct.unpack_from("<I", payload)
                elif flag == p.QUERY_POSITION:
                    (position,) = struct.unpack_from("<I", payload)
                elif flag == p.QUERY_IS_PUBLIC:
                    public = payload[0] == 1
            out.append(SlotQuery(code=code, position=position, is_public=public))
        return out

    def destroy(self) -> None:
        if self._duel:
            self._lib.OCG_DestroyDuel(self._duel)
            self._duel = ctypes.c_void_p()
