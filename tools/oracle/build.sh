#!/usr/bin/env bash
# Reproducible build of the oracle's ocgcore library + card scripts.
# Verified on macOS arm64 (Apple clang) and Linux (Debian, clang++ or g++;
# the Dockerfile uses it); requires: git, curl, a C++17 compiler (`$CXX`,
# else clang++, else g++). Artifacts land in tools/oracle/build/
# (gitignored): libocgcore.dylib on macOS, libocgcore.so elsewhere.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
BUILD="$HERE/build"
SRC="$BUILD/src"
source "$HERE/versions.txt"

if [ -z "${CXX:-}" ]; then
  if command -v clang++ >/dev/null 2>&1; then CXX=clang++; else CXX=g++; fi
fi
# ORACLE_COVERAGE=1 builds a second, instrumented library beside the normal
# one (clang source-based coverage: -fprofile-instr-generate
# -fcoverage-mapping on the core files only), used by tools/oracle/coverage.py
# for the reference-coverage audit (YUG-99). Its objects and library carry a
# `-cov` suffix so the two builds never mix.
COV="${ORACLE_COVERAGE:-}"
SUFFIX=""
[ -n "$COV" ] && SUFFIX="-cov"
case "$(uname -s)" in
  Darwin) LIB="$BUILD/libocgcore$SUFFIX.dylib" ;;
  *) LIB="$BUILD/libocgcore$SUFFIX.so" ;;
esac

mkdir -p "$BUILD" "$SRC"

# 1. Pinned core sources.
if [ ! -d "$SRC/ygopro-core" ]; then
  git clone https://github.com/edo9300/ygopro-core "$SRC/ygopro-core"
fi
git -C "$SRC/ygopro-core" checkout --quiet "$CORE_COMMIT"

# 2. Lua, compiled as C++ (required: the core relies on C++ exception
#    unwinding through Lua frames; see interpreter.h in the core sources).
if [ ! -d "$SRC/lua-$LUA_VERSION" ]; then
  curl -sfL "https://www.lua.org/ftp/lua-$LUA_VERSION.tar.gz" | tar xz -C "$SRC"
fi

# 3. Compile everything as C++ with $CXX and link the shared library.
OBJ="$BUILD/obj$SUFFIX"
mkdir -p "$OBJ"
CXXFLAGS="-std=c++17 -O2 -fPIC -w"
COVFLAGS=""
[ -n "$COV" ] && COVFLAGS="-fprofile-instr-generate -fcoverage-mapping"
for f in "$SRC/lua-$LUA_VERSION"/src/*.c; do
  base="$(basename "$f" .c)"
  [ "$base" = "lua" ] || [ "$base" = "luac" ] && continue  # skip standalone mains
  out="$OBJ/lua_$base.o"
  [ -f "$out" ] || "$CXX" -x c++ $CXXFLAGS -c "$f" -o "$out"
done
for f in "$SRC/ygopro-core"/*.cpp; do
  base="$(basename "$f" .cpp)"
  out="$OBJ/core_$base.o"
  [ -f "$out" ] || "$CXX" $CXXFLAGS $COVFLAGS -I"$SRC/lua-$LUA_VERSION/src" -c "$f" -o "$out"
done
"$CXX" -shared $COVFLAGS -o "$LIB" "$OBJ"/*.o
echo "built $LIB"

# 4. Card scripts: base scripts + utility deps + one script per pool card.
SCRIPTS="$BUILD/cardscripts"
mkdir -p "$SCRIPTS"
RAW="https://raw.githubusercontent.com/ProjectIgnis/CardScripts/$CARDSCRIPTS_COMMIT"
base_scripts=(constant.lua utility.lua card_counter_constants.lua
  archetype_setcode_constants.lua cards_specific_functions.lua chain.lua
  debug_utility.lua deprecated_functions.lua proc_equip.lua proc_fusion.lua
  proc_fusion_spell.lua proc_gemini.lua proc_link.lua proc_maximum.lua
  proc_normal.lua proc_pendulum.lua proc_persistent.lua proc_ritual.lua
  proc_rush.lua proc_skill.lua proc_spirit.lua proc_synchro.lua proc_union.lua
  proc_workaround.lua proc_xyz.lua)
for name in "${base_scripts[@]}"; do
  [ -f "$SCRIPTS/$name" ] || curl -sf -o "$SCRIPTS/$name" "$RAW/$name" \
    || { echo "error: base script $name not fetched"; exit 1; }
done
# Effect cards in the pool (vanilla monsters need no script).
for code in 14087893 56120475 4206964 54652250 53129443 55144522 \
  5318639 60082869 19613556 44095762 53582587 36361633 \
  87621407 31560081 33508719 79571449 60082869 77754944 18036057 7572887 \
  26202165 2134346 34853266 74713516 44763025 64697231 71044499 70828912 97077563 \
  8131171 45986603 71413901 33184167 83555666 73915051 46411259 63519819 72989439 9596126 80071763 49868263 66235877 13756293 87751584 85684223 98045062 31036355 \
  77585513 32807846 32362575; do
  [ -f "$SCRIPTS/c$code.lua" ] || curl -sf -o "$SCRIPTS/c$code.lua" "$RAW/official/c$code.lua"
done
# Mystic Tomato (83011278): ProjectIgnis scripts it under its alt-art id
# 83011277 (their cdb aliases the two); fetch that script under our id.
[ -f "$SCRIPTS/c83011278.lua" ] \
  || curl -sf -o "$SCRIPTS/c83011278.lua" "$RAW/official/c83011277.lua"
# Guard against silent fetch failures: every script must exist and be lua
# (a saved HTTP error body, e.g. "404: Not Found", would make ocgcore treat
# the card as a vanilla, or break constant.lua for every card).
for f in "$SCRIPTS"/*.lua; do
  head -c4 "$f" | grep -q -E "^(<|404|400|403|500)" && { echo "bad script $f"; exit 1; }
done
echo "scripts in $SCRIPTS"
echo "done; verify with: uv run python -c 'from tools.oracle.core import oracle_available; print(oracle_available())'"
