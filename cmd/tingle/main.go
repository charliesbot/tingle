// tingle — fast, stateless orientation map for AI agents.
//
// Usage:
//
//	tingle <repo-path>
//	tingle --alias PREFIX:PATH <repo-path>
//	tingle --no-legend <repo-path>
package main

import (
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime/debug"
	"strings"

	"github.com/charliesbot/tingle/internal/enumerate"
	"github.com/charliesbot/tingle/internal/manifest"
	"github.com/charliesbot/tingle/internal/model"
	"github.com/charliesbot/tingle/internal/parse"
	"github.com/charliesbot/tingle/internal/rank"
	"github.com/charliesbot/tingle/internal/render"
	"github.com/charliesbot/tingle/internal/resolve"
)

func init() {
	// Cap Go heap at 512 MiB. On gotreesitter, per-parser arenas can balloon
	// when parsing many files concurrently — this keeps peak RSS bounded
	// without requiring users to set GOMEMLIMIT in the env.
	// Users can override with GOMEMLIMIT=... at invocation time.
	if os.Getenv("GOMEMLIMIT") == "" {
		debug.SetMemoryLimit(512 * 1024 * 1024)
	}
}

var version = "v0-dev" // overridden via -ldflags="-X main.version=..."

type aliasFlag map[string]string

func (a aliasFlag) String() string {
	parts := make([]string, 0, len(a))
	for k, v := range a {
		parts = append(parts, k+":"+v)
	}
	return strings.Join(parts, ",")
}

func (a aliasFlag) Set(v string) error {
	idx := strings.Index(v, ":")
	if idx < 0 {
		return fmt.Errorf("alias must be PREFIX:PATH, got %q", v)
	}
	a[v[:idx]] = v[idx+1:]
	return nil
}

func main() {
	aliases := aliasFlag{}
	var (
		noLegend bool
		showVer  bool
	)
	flag.Var(aliases, "alias", "map an import prefix to a repo path; repeatable (e.g. '--alias @:src')")
	flag.BoolVar(&noLegend, "no-legend", false, "omit the legend header line")
	flag.BoolVar(&showVer, "version", false, "print version and exit")
	flag.Parse()

	if showVer {
		fmt.Println("tingle", version)
		return
	}

	args := flag.Args()
	repo := "."
	if len(args) > 0 {
		repo = args[0]
	}
	repoAbs, err := filepath.Abs(repo)
	if err != nil {
		fail(err)
	}
	if info, err := os.Stat(repoAbs); err != nil || !info.IsDir() {
		fail(fmt.Errorf("not a directory: %s", repoAbs))
	}

	// 1. enumerate
	files, err := enumerate.Repo(repoAbs)
	if err != nil {
		fail(err)
	}

	// 2. parse — reset per-run counters so parse-error reporting is clean.
	parse.NewRun()
	parse.All(repoAbs, files)

	// 3. resolve
	resolve.All(files, resolve.Aliases(aliases))

	// 4. rank (includes graph building)
	dirEdges, callers := rank.Graph(files)

	// 4a. manifest scan for S records + manifest-declared entry point bonus
	m := manifest.Scan(repoAbs)

	entries := rank.EntryPoints(files, rank.EntryPointsOpts{
		Repo:       repoAbs,
		ManifestEP: m.EntryTargets,
		MaxEPs:     15,
	})
	utilities := rank.Utilities(files)

	// 5. render
	out := render.Render(files, entries, utilities, dirEdges, callers, m.SRecords, render.Options{
		Version:     version,
		Commit:      shortGitCommit(repoAbs),
		TokenizerID: "cl100k_base",
		NoLegend:    noLegend,
	})
	_, _ = os.Stdout.WriteString(out)

	// Emit non-zero parse-error counts to stderr. Gate 4: zero parse errors.
	if n := parse.PackageStats.ParseErrors.Load(); n > 0 {
		fmt.Fprintf(os.Stderr, "tingle: %d parse errors (files emitted without defs/imports)\n", n)
	}
	if n := parse.PackageStats.ReadErrors.Load(); n > 0 {
		fmt.Fprintf(os.Stderr, "tingle: %d files unreadable\n", n)
	}
}

func fail(err error) {
	fmt.Fprintln(os.Stderr, "tingle:", err)
	os.Exit(1)
}

func shortGitCommit(repo string) string {
	out, err := exec.Command("git", "-C", repo, "rev-parse", "--short", "HEAD").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

// Ensure the model package stays referenced even if all other packages stop
// depending on it (keeps go vet happy during refactors).
var _ = (*model.FileIndex)(nil)
