# The oracle: ocgcore, built from pinned sources

The reference this engine is verified against. `build.sh` fetches
[edo9300/ygopro-core](https://github.com/edo9300/ygopro-core) (ocgcore) and
the card scripts from [ProjectIgnis/CardScripts](https://github.com/ProjectIgnis/CardScripts)
at the commits pinned in `versions.txt`, with Lua, and builds the core as a
shared library. `core.py` drives it through `ctypes` (`protocol.py` decodes
its messages, `carddata.py` supplies its card database); `tools/differential.py`
and `tools/fuzz.py` use it to play the same duel on both engines and diff
their traces.

## Build

```sh
tools/oracle/build.sh
python3 -c 'from tools.oracle.core import oracle_available; print(oracle_available())'
```

Requires git, curl and a C++17 compiler (`$CXX`, else clang++, else g++;
verified on macOS arm64 with Apple clang and on Debian with clang++), and
Python 3.10 or newer for the harness (standard library only). Everything,
Lua included, is compiled as C++, because the core unwinds C++ exceptions
through Lua frames. Outputs, all under the gitignored `tools/oracle/build/`:

- `libocgcore.dylib` (macOS) / `libocgcore.so` (Linux), the core library
- `cardscripts/`: `constant.lua`, `utility.lua`, the shared script library,
  and the per-card `cN.lua` scripts for the card pool

Override locations with `ORACLE_LIB` / `ORACLE_SCRIPTS`.

## Licensing

ocgcore and the card scripts are AGPL-3.0, as is this engine, which is
derived from both. The build artifacts are fetched and built locally, never
committed.
