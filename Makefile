# Parity + bench harness for the Go→Rust migration.
#
# `make parity` runs the Go binary (./tingle) and the Rust binary
# (rust/target/release/tingle) against three real repos and diffs the
# structural body of the output (header lines 1-2 stripped — they vary
# by version/date/commit and carry no structural signal).
#
# `make bench` measures wall-clock + peak RSS per binary per repo and
# writes the results into docs/bench-results.md.

GO_BIN    := ./tingle
RUST_BIN  := rust/target/release/tingle

# REPOS is the list of real projects to compare against. Override on the
# command line if your paths differ:
#   make parity REPOS="/path/to/repo-a /path/to/repo-b"
REPOS ?= \
  $(HOME)/projects/advent-of-code \
  $(HOME)/projects/charliesbot.dev \
  $(HOME)/projects/one

.PHONY: build-go build-rust parity bench clean-artifacts

build-go:
	go build -o $(GO_BIN) ./cmd/tingle

build-rust:
	cd rust && cargo build --release

parity: build-go build-rust
	@mkdir -p /tmp/tingle-parity
	@fail=0; \
	for repo in $(REPOS); do \
	  name=$$(basename $$repo); \
	  $(GO_BIN) $$repo 2>/dev/null | sed '1,2d' > /tmp/tingle-parity/$$name.go.txt; \
	  $(RUST_BIN) $$repo 2>/dev/null | sed '1,2d' > /tmp/tingle-parity/$$name.rust.txt; \
	  diff -u /tmp/tingle-parity/$$name.go.txt /tmp/tingle-parity/$$name.rust.txt > /tmp/tingle-parity/$$name.diff || true; \
	  adds=$$(grep -c '^+' /tmp/tingle-parity/$$name.diff | head -1); \
	  removes=$$(grep -c '^-' /tmp/tingle-parity/$$name.diff | head -1); \
	  adds=$$((adds - 1)); \
	  removes=$$((removes - 1)); \
	  if [ ! -s /tmp/tingle-parity/$$name.diff ]; then \
	    printf "%-25s  IDENTICAL\n" "$$name"; \
	  elif [ $$removes -eq 0 ]; then \
	    printf "%-25s  RUST-ONLY ADDITIONS (%s lines) — grammar-fix wins, OK\n" "$$name" "$$adds"; \
	  else \
	    printf "%-25s  REGRESSION (%s removed, %s added)\n" "$$name" "$$removes" "$$adds"; \
	    fail=1; \
	  fi; \
	done; \
	if [ $$fail -ne 0 ]; then echo "parity: diffs saved to /tmp/tingle-parity/"; fi; \
	exit $$fail

bench: build-go build-rust
	@bash scripts/bench.sh $(REPOS)

clean-artifacts:
	rm -rf /tmp/tingle-parity
