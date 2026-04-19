// Spike D — validate gotreesitter (pure Go tree-sitter runtime) on our real
// workload.
//
// Goals:
//   1. Prove gotreesitter parses real Kotlin without errors.
//   2. Prove our augmented tags.scm queries compile + execute correctly.
//   3. Prove capture names we rely on (@definition.*, @name.*, @reference.import)
//      round-trip through gotreesitter's query engine.
//   4. Measure parse time vs the cgo baseline (spike A on the same repo).
//   5. Measure binary size with zero cgo.
//
// Build: CGO_ENABLED=0 go build -ldflags="-s -w" -o spikes/d_puregoTS/spike ./spikes/d_puregoTS
// Run:   /usr/bin/time -l ./spikes/d_puregoTS/spike <repo-path>
package main

import (
	_ "embed"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/odvcencio/gotreesitter"
	"github.com/odvcencio/gotreesitter/grammars"
)

//go:embed kotlin-tags.scm
var kotlinQuerySrc string

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: spike <repo-path>")
		os.Exit(2)
	}
	repo := os.Args[1]

	files, err := listKotlinFiles(repo)
	if err != nil {
		fmt.Fprintln(os.Stderr, "enumerate:", err)
		os.Exit(1)
	}
	fmt.Printf("files:       %d\n", len(files))

	// Validate query compiles against gotreesitter's Kotlin language.
	lang := grammars.KotlinLanguage()
	query, err := gotreesitter.NewQuery(kotlinQuerySrc, lang)
	if err != nil {
		fmt.Fprintln(os.Stderr, "query compile failed:", err)
		os.Exit(1)
	}
	fmt.Printf("query:       compiled (captures=%d)\n", query.CaptureCount())

	// Parse all files in parallel, bounded to runtime.NumCPU().
	workers := runtime.NumCPU()
	sem := make(chan struct{}, workers)
	var wg sync.WaitGroup

	var parseErrors, parseOK int64
	var totalBytes int64
	var totalCaptures int64

	// Per-parse timing.
	perParseNanos := make([]int64, 0, len(files))
	var perParseMu sync.Mutex

	// Capture-name histogram so we can verify the captures we rely on all fire.
	captureHist := map[string]int64{}
	var captureHistMu sync.Mutex

	start := time.Now()
	for _, f := range files {
		wg.Add(1)
		sem <- struct{}{}
		go func(rel string) {
			defer wg.Done()
			defer func() { <-sem }()

			data, err := os.ReadFile(filepath.Join(repo, rel))
			if err != nil {
				atomic.AddInt64(&parseErrors, 1)
				return
			}
			atomic.AddInt64(&totalBytes, int64(len(data)))

			// Fresh parser per file to match our spike A pattern. For an apples
			// comparison we could also test parser pool, but keeping it close
			// to the cgo spike for now.
			parser := gotreesitter.NewParser(lang)

			parseStart := time.Now()
			tree, err := parser.Parse(data)
			parseDur := time.Since(parseStart).Nanoseconds()
			if err != nil || tree == nil {
				atomic.AddInt64(&parseErrors, 1)
				return
			}

			perParseMu.Lock()
			perParseNanos = append(perParseNanos, parseDur)
			perParseMu.Unlock()

			// Execute query.
			cursor := query.Exec(tree.RootNode(), lang, data)
			localHist := map[string]int64{}
			localCaptures := int64(0)
			for {
				match, ok := cursor.NextMatch()
				if !ok {
					break
				}
				for _, cap := range match.Captures {
					localHist[cap.Name]++
					localCaptures++
				}
			}
			atomic.AddInt64(&totalCaptures, localCaptures)
			atomic.AddInt64(&parseOK, 1)

			captureHistMu.Lock()
			for k, v := range localHist {
				captureHist[k] += v
			}
			captureHistMu.Unlock()
		}(f)
	}
	wg.Wait()
	elapsed := time.Since(start)

	var m runtime.MemStats
	runtime.ReadMemStats(&m)

	fmt.Printf("workers:     %d\n", workers)
	fmt.Printf("parse time:  %v (total, wall-clock)\n", elapsed)
	fmt.Printf("parsed ok:   %d\n", parseOK)
	fmt.Printf("errors:      %d\n", parseErrors)
	fmt.Printf("bytes read:  %d (%.2f MB)\n", totalBytes, float64(totalBytes)/(1024*1024))
	fmt.Printf("captures:    %d total\n", totalCaptures)

	if len(perParseNanos) > 0 {
		var sum, max int64
		for _, n := range perParseNanos {
			sum += n
			if n > max {
				max = n
			}
		}
		avg := sum / int64(len(perParseNanos))
		fmt.Printf("per-parse:   avg=%v  max=%v  sum=%v\n",
			time.Duration(avg), time.Duration(max), time.Duration(sum))
	}

	fmt.Printf("heap alloc:  %.2f MB (current)\n", float64(m.HeapAlloc)/(1024*1024))
	fmt.Printf("heap sys:    %.2f MB (from OS into Go heap)\n", float64(m.HeapSys)/(1024*1024))

	// Dump capture histogram — confirm the captures we rely on actually fire.
	fmt.Println("\ncapture histogram:")
	keys := make([]string, 0, len(captureHist))
	for k := range captureHist {
		keys = append(keys, k)
	}
	// sort for stable output
	sortStrings(keys)
	for _, k := range keys {
		fmt.Printf("  %-40s %d\n", k, captureHist[k])
	}
}

func listKotlinFiles(repo string) ([]string, error) {
	cmd := exec.Command("git", "-C", repo, "ls-files", "-com", "--exclude-standard", "*.kt", "*.kts")
	out, err := cmd.Output()
	if err != nil {
		return nil, err
	}
	lines := strings.Split(strings.TrimSpace(string(out)), "\n")
	files := make([]string, 0, len(lines))
	for _, l := range lines {
		if l != "" {
			files = append(files, l)
		}
	}
	return files, nil
}

func sortStrings(xs []string) {
	for i := 1; i < len(xs); i++ {
		for j := i; j > 0 && xs[j-1] > xs[j]; j-- {
			xs[j-1], xs[j] = xs[j], xs[j-1]
		}
	}
}
