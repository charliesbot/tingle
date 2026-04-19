// Package manifest parses package.json and go.mod at the repo root into
// compact S-record lines ready for the render pipeline.
//
// v1 surfaces only fields useful for agent orientation: package.json scripts,
// bin, main, exports; go.mod module path + go version.
package manifest

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
)

// Parsed holds pre-rendered S-record lines plus file paths declared as entry
// points (used by the ranker's manifest-declared-entry bonus).
type Parsed struct {
	SRecords     []string
	EntryTargets []string // repo-relative paths of bin/main/exports targets
}

// Scan inspects the repo root for package.json and go.mod. Missing files are
// silently skipped. Scan never errors — it returns what it found.
func Scan(repo string) Parsed {
	var out Parsed
	out.scanPackageJSON(repo)
	out.scanGoMod(repo)
	return out
}

type packageJSON struct {
	Name    string          `json:"name"`
	Main    string          `json:"main"`
	Module  string          `json:"module"`
	Bin     json.RawMessage `json:"bin"`
	Exports json.RawMessage `json:"exports"`
	Scripts map[string]string `json:"scripts"`
}

func (p *Parsed) scanPackageJSON(repo string) {
	data, err := os.ReadFile(filepath.Join(repo, "package.json"))
	if err != nil {
		return
	}
	var pkg packageJSON
	if err := json.Unmarshal(data, &pkg); err != nil {
		return
	}

	if len(pkg.Scripts) > 0 {
		keys := make([]string, 0, len(pkg.Scripts))
		for k := range pkg.Scripts {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		parts := make([]string, 0, len(keys))
		for _, k := range keys {
			parts = append(parts, fmt.Sprintf("%s=%s", k, summarizeScript(pkg.Scripts[k])))
		}
		p.SRecords = append(p.SRecords, "S package.json  scripts: "+strings.Join(parts, " "))
	}

	switch {
	case pkg.Main != "" && pkg.Module != "":
		p.SRecords = append(p.SRecords, fmt.Sprintf("S package.json  main: %s  module: %s", pkg.Main, pkg.Module))
		p.EntryTargets = append(p.EntryTargets, pkg.Main, pkg.Module)
	case pkg.Main != "":
		p.SRecords = append(p.SRecords, "S package.json  main: "+pkg.Main)
		p.EntryTargets = append(p.EntryTargets, pkg.Main)
	case pkg.Module != "":
		p.SRecords = append(p.SRecords, "S package.json  module: "+pkg.Module)
		p.EntryTargets = append(p.EntryTargets, pkg.Module)
	}

	if bins := parseBin(pkg.Bin); len(bins) > 0 {
		parts := make([]string, 0, len(bins))
		for _, b := range bins {
			parts = append(parts, fmt.Sprintf("%s->%s", b.name, b.path))
			p.EntryTargets = append(p.EntryTargets, b.path)
		}
		p.SRecords = append(p.SRecords, "S package.json  bin: "+strings.Join(parts, " "))
	}
}

type binEntry struct{ name, path string }

func parseBin(raw json.RawMessage) []binEntry {
	if len(raw) == 0 {
		return nil
	}
	// bin can be a string or an object
	var str string
	if err := json.Unmarshal(raw, &str); err == nil && str != "" {
		return []binEntry{{name: "default", path: str}}
	}
	var obj map[string]string
	if err := json.Unmarshal(raw, &obj); err != nil {
		return nil
	}
	out := make([]binEntry, 0, len(obj))
	for name, path := range obj {
		out = append(out, binEntry{name: name, path: path})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].name < out[j].name })
	return out
}

// summarizeScript trims long script bodies so the S-record line stays short.
// Anything over 40 chars gets truncated with `…`.
func summarizeScript(s string) string {
	s = strings.ReplaceAll(s, "\n", " ")
	s = collapseWS.ReplaceAllString(s, " ")
	if len(s) > 40 {
		return strings.TrimSpace(s[:40]) + "…"
	}
	return s
}

var collapseWS = regexp.MustCompile(`\s+`)

func (p *Parsed) scanGoMod(repo string) {
	data, err := os.ReadFile(filepath.Join(repo, "go.mod"))
	if err != nil {
		return
	}
	lines := strings.Split(string(data), "\n")
	var module, goVer string
	for _, line := range lines {
		line = strings.TrimSpace(line)
		switch {
		case strings.HasPrefix(line, "module "):
			module = strings.TrimSpace(strings.TrimPrefix(line, "module "))
		case strings.HasPrefix(line, "go "):
			goVer = strings.TrimSpace(strings.TrimPrefix(line, "go "))
		}
	}
	switch {
	case module != "" && goVer != "":
		p.SRecords = append(p.SRecords, fmt.Sprintf("S go.mod        module=%s  go=%s", module, goVer))
	case module != "":
		p.SRecords = append(p.SRecords, "S go.mod        module="+module)
	}
}
