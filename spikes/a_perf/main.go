// Spike A — measure tree-sitter-in-Go performance on a real Kotlin repo.
//
// Usage:
//
//	go build -ldflags="-s -w" -o spike ./spikes/a_perf
//	/usr/bin/time -l ./spike <repo-path>
//
// Targets (from design doc):
//   - <2s total parse
//   - <30MB binary (this file, with -ldflags)
//   - <200MB peak RSS (external measurement via /usr/bin/time -l)
//   - cgo overhead not dominant
package main

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	sitter "github.com/smacker/go-tree-sitter"
	"github.com/smacker/go-tree-sitter/kotlin"
)

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: spike <repo-path>")
		os.Exit(2)
	}
	repo := os.Args[1]

	files, err := listKotlinFiles(repo)
	if err != nil {
		fmt.Fprintln(os.Stderr, "enumerate failed:", err)
		os.Exit(1)
	}
	fmt.Printf("files:       %d\n", len(files))

	lang := kotlin.GetLanguage()

	// Bounded worker pool — matches the design's runtime.NumCPU() commitment.
	workers := runtime.NumCPU()
	sem := make(chan struct{}, workers)
	var wg sync.WaitGroup

	var totalBytes int64
	var parseErrors int64
	var parsedOK int64

	// Per-parse timing to measure cgo overhead distribution.
	perParseNanos := make([]int64, 0, len(files))
	var perParseMu sync.Mutex

	enumStart := time.Now()
	enumElapsed := time.Since(enumStart) // just the list step already ran; placeholder

	start := time.Now()
	for _, f := range files {
		wg.Add(1)
		sem <- struct{}{}
		go func(rel string) {
			defer wg.Done()
			defer func() { <-sem }()

			full := filepath.Join(repo, rel)
			data, err := os.ReadFile(full)
			if err != nil {
				atomic.AddInt64(&parseErrors, 1)
				return
			}
			atomic.AddInt64(&totalBytes, int64(len(data)))

			parser := sitter.NewParser()
			parser.SetLanguage(lang)

			parseStart := time.Now()
			tree, err := parser.ParseCtx(context.Background(), nil, data)
			parseDur := time.Since(parseStart).Nanoseconds()
			if err != nil || tree == nil {
				atomic.AddInt64(&parseErrors, 1)
				return
			}
			atomic.AddInt64(&parsedOK, 1)
			perParseMu.Lock()
			perParseNanos = append(perParseNanos, parseDur)
			perParseMu.Unlock()

			tree.Close()
			parser.Close()
		}(f)
	}
	wg.Wait()
	elapsed := time.Since(start)

	var m runtime.MemStats
	runtime.ReadMemStats(&m)

	fmt.Printf("workers:     %d\n", workers)
	fmt.Printf("parse time:  %v (total, wall-clock)\n", elapsed)
	fmt.Printf("enum time:   %v (git ls-files)\n", enumElapsed)
	fmt.Printf("parsed ok:   %d\n", parsedOK)
	fmt.Printf("errors:      %d\n", parseErrors)
	fmt.Printf("bytes read:  %d (%.2f MB)\n", totalBytes, float64(totalBytes)/(1024*1024))

	if len(perParseNanos) > 0 {
		var sum int64
		var max int64
		for _, n := range perParseNanos {
			sum += n
			if n > max {
				max = n
			}
		}
		avg := sum / int64(len(perParseNanos))
		fmt.Printf("per-parse:   avg=%v  max=%v  (sum=%v across %d files)\n",
			time.Duration(avg), time.Duration(max), time.Duration(sum), len(perParseNanos))
	}

	fmt.Printf("heap alloc:  %.2f MB (current)\n", float64(m.HeapAlloc)/(1024*1024))
	fmt.Printf("heap sys:    %.2f MB (from OS into Go heap)\n", float64(m.HeapSys)/(1024*1024))
	fmt.Println("(peak RSS — run under `/usr/bin/time -l` for C-side allocations too)")
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
