# Operations runbook

How to build, run, verify and troubleshoot the project. Measured on a 4 core machine with 15 GB of memory.

## 1. Prerequisites

* Rust: install `rustup`. The pinned version in `rust-toolchain.toml` is installed automatically on the first `cargo` command.
* `cmake` and a C++ compiler with OpenMP. LightGBM is compiled from source by the `lightgbm3` crate.
* About 5 GB of free disk space (a release build is about 2 GB, a debug build about 2 GB, the cleaned data and artifacts about 1 GB).

Ubuntu or Debian:

```bash
sudo apt install build-essential cmake
```

macOS:

```bash
brew install cmake libomp
```

Windows is not tested.

`vwap doctor config` reports what is missing.

## 2. Build and first run

```bash
cargo build --release
./target/release/vwap doctor config
./target/release/vwap pipeline config
```

Timings on the included data (release build):

* `doctor` 0.7 s (it hashes the 57 MB raw file)
* `data clean` 2.7 s
* `events detect` 1.0 s
* `labels build` 0.7 s
* `baselines run` 2.5 s
* `spotcheck` 1.1 s
* `research verify` under 0.1 s
* the whole `pipeline` about 9 s

The first build takes 5 to 10 minutes. `cargo test --workspace` takes about 25 seconds once built.

## 3. What each stage writes

* `data clean`: `data/clean/year=YYYY/part0.parquet` (cleaned bars, true UTC), `artifacts/data/manifest.json` (input hash, counts, `clean_fingerprint`), `quality_report.json`, `roll_table.json`.
* `events detect`: `artifacts/events/bars_{5,15}m.parquet`, `events_{5,15}m.parquet`, `events_report.json`, `manifest.json`. These are for inspection. No stage reads them, because every stage rebuilds events from the cleaned bars in a few seconds, which keeps one source of truth.
* `labels build`: `artifacts/labels/labeled_events_{5,15}m.parquet`, `label_report_{5,15}m.json`, `manifest.json`.
* `baselines run`: `artifacts/baselines/baselines_report.json`, `reports/gate_baselines.md`, `manifest.json`.
* `spotcheck`: `artifacts/spotcheck/*.png`, discovery segment days only.
* `research ...`: `research/ledger.jsonl` and `research/reports/*` (tracked in git).

`data/`, `artifacts/`, `reports/` and `target/` are ignored by git and can be deleted and rebuilt. `research/` is the audit trail and must never be deleted or edited by hand.

## 4. Reproducing the research

```bash
./target/release/vwap research verify config
./target/release/vwap research replay all config
```

`verify` fails if a card changed after registration, if cycles are out of order, if the budget is exceeded, or if a commit named by the ledger is missing. `replay all` fails if any card gives a different delta, p value or verdict from the ledger. A mismatch means the data, the code or a configuration value changed since the result was recorded. Check, in order: `vwap doctor config` (data checksum), `git diff config/`, and the recorded commit of the card in `research/ledger.jsonl`.

## 5. Running a new hypothesis

1. Write `hypotheses/H008.toml` (copy an existing card, write the mechanism in words) and commit it.
2. `vwap research register H008 config`, then `vwap research run H008 config`.
3. Read `research/reports/H008.md`. The verdict is PROMOTE, REFINE or DROP by the rules in `docs/PROTOCOL.md`.
4. Commit `research/ledger.jsonl` and `research/reports/`.
5. Only for PROMOTE: `vwap research confirm H008 config` once, then, if confirmed, create `research/FREEZE.toml` (`card_id` and `note`), commit, and run `vwap holdout run config` once.

The alpha budget is shared: 12 cards and a total of 0.05 for the whole study. `vwap research status config` shows what is left.

## 6. After a configuration or code change

* Changed `data.toml` or the raw file: run `vwap data clean config`, then the later stages.
* Changed `events.toml`, `labels.toml`, `features.toml`: results change. `research replay all` will show which recorded cards are affected. Do not overwrite the ledger. A changed definition is a new study and needs new cards.
* Changed only `baselines.toml` or `model.toml`: the baseline report or the omnibus changes, the recorded card results do not.

## 7. Troubleshooting

* **No space left on device.** Delete `target/debug` (`cargo clean` removes everything). The workspace already sets `debug = 0` for dev and test profiles to keep builds small. Every test binary links polars statically.
* **LightGBM fails to build.** Install `cmake` and a C++ compiler with OpenMP (see section 1), then `cargo clean -p lightgbm3-sys` and build again.
* **"the cleaned data does not match its manifest".** The files in `data/clean` changed after `data clean` wrote its manifest. Run `vwap data clean config` again.
* **"config/data.toml pins ...".** The raw file differs from the pinned SHA256. Restore the file, or, if you changed it on purpose, update `input_sha256` and treat all earlier results as belonging to the old data.
* **"... is ignored by git".** The ledger or report directory is covered by `.gitignore`, so the audit trail would be lost on a fresh clone. Fix `.gitignore`.
* **"no usable font".** Set `font_paths` in `config/events.toml` to a TrueType font on your system. Only `vwap spotcheck` needs it.
* **"... is not committed; commit the card first".** `register` and `run` require the card file to be committed and unchanged, so the run is tied to a commit.
* **`git push` returns 403.** The Claude GitHub App needs write access (contents permission) to the repository. Grant it at https://github.com/apps/claude/installations/select_target. Pushing `.github/workflows/` additionally needs the workflows permission.
