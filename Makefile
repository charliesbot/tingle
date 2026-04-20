# Build + bench for tingle.
#
# `make build` compiles the release binary at rust/target/release/tingle.
# `make bench` measures wall-clock + peak RSS on a set of real repos and
# writes the results into docs/bench-results.md.

RUST_BIN := rust/target/release/tingle

# REPOS is the list of real projects to bench against. Override on the
# command line if your paths differ:
#   make bench REPOS="/path/to/repo-a /path/to/repo-b"
REPOS ?= \
  $(HOME)/projects/advent-of-code \
  $(HOME)/projects/charliesbot.dev \
  $(HOME)/projects/one

.PHONY: build test bench install

build:
	cd rust && cargo build --release

test:
	cd rust && cargo test --release

bench: build
	@bash scripts/bench.sh $(RUST_BIN) $(REPOS)

install:
	cargo install --path rust --force
