# ygo-engine

**A verified Rust port of ocgcore**, the rules engine behind EDOPro, the
reference simulator for the Yu-Gi-Oh! trading card game.

It is a translation, not a reimplementation from the rules: ocgcore's
processor units, point events, chain solving and step machine are carried
over structurally, and where ocgcore makes an arbitrary choice, this makes
the same one. The card scripts, which ocgcore runs in an embedded Lua VM,
are hand-translated to Rust instead, so that a game state is plain data
that clones in about 30 µs: the engine is built to be searched.

## Verified against the original

A differential harness plays the same duel on both engines and compares
their traces line by line, every question asked and every event announced
(`tools/differential.py`, `tools/fuzz.py`, with the ocgcore build in
[`tools/oracle`](tools/oracle)).

- **6,014,000 randomized games identical** to ocgcore, half on a shuffled
  deck of the whole card pool, half on the Goat Control mirror, with every
  difference found along the way fixed and pinned by a test.
- **A mutation sweep**: 9,684 mutants of the core, each caught by the unit
  tests or by thirty oracle games, or triaged as unreached or equivalent.
- **Every transcribed constant table** diffed against ocgcore's headers and
  the script library (`tools/check_constants.py`).
- About **5× faster** than ocgcore on the same games: 6.5 µs per move
  through the game-tree API, about 430 random games a second on one core.

## Scope

The Goat-format card pool. It is the card list that is scoped, not the
engine: the core rules machinery is complete, faithful even where the pool
does not exercise it, and new cards plug in as translated scripts.

## For search

Beyond the faithful duel, the engine offers what a game-playing search
needs (`src/game.rs`, `src/observation.rs`, `src/determinize.rs`,
`src/rollout.rs`):

- **`Game`**: a duel as a game tree. Legal actions in a canonical order,
  explicit chance nodes with probabilities (coin tosses, random selections,
  the look at a deck's top), forced nodes answered on the way, cycles that
  only return to a menu pruned.
- **Observations and information-set keys**: a per-player view of the
  game with a model of which hidden cards each player has seen.
- **`determinize(viewer, seed)`**: a world the viewer cannot tell from the
  real one, with the hidden cards they do not know reassigned uniformly
  among consistent completions. The belief seam for PIMC and IS-MCTS.
- **`rollout(max_moves, seed)`**: a seeded random playout on a copy, to
  the end or to a cap, handing back the result or the cut-off game.

`Game: Clone + Send`, so a search can run one game per thread.

## Build and verify

```sh
cargo test                                   # unit tests
cargo run --release --example bench -- --game   # speed through the game-tree API

tools/oracle/build.sh                        # fetch and build ocgcore (pinned)
PORT_ORACLE_FUZZ=30 cargo test --test oracle_fuzz   # thirty games against ocgcore
python3 tools/fuzz.py --deck pool --games 1000 --workers 0
python3 tools/differential.py --policy random:42 --context 6   # one duel, diffed
python3 tools/check_constants.py
```

The harness needs Python 3.10 or newer, standard library only; set
`PYTHON` to point the oracle test at an interpreter other than `python3`.

## Documentation

- [`docs/processor-loop.md`](docs/processor-loop.md): the reading each
  piece was written from, every deliberate departure from a literal
  translation, and every finding the harness made.
- [`docs/script-library.md`](docs/script-library.md): the shared Lua
  library ocgcore's card scripts depend on, and how it maps here.

## License

AGPL-3.0-or-later, as a derivative of ocgcore and of the ProjectIgnis card
scripts. See [LICENSE](LICENSE) and [NOTICE](NOTICE) for the chain of
attribution.
