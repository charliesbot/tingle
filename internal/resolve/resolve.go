// Package resolve maps relative imports to repo-relative file paths.
// Heuristic only: path math + common-extension guessing. External imports
// and aliased imports stay raw.
package resolve

import (
	"path/filepath"
	"strings"

	"github.com/charliesbot/tingle/internal/model"
)

// Default extensions to try, in rough order of likelihood.
var candidateExts = []string{".ts", ".tsx", ".js", ".jsx", ".mjs", ".py", ".go", ".kt"}

// Aliases is a user-supplied prefix → path mapping (e.g. "@" → "src").
// Empty map means no aliases applied.
type Aliases map[string]string

// All rewrites f.Imports in place so resolvable ones become repo-relative
// paths, and unresolvable ones stay raw. Idempotent on already-resolved paths.
func All(files []*model.FileIndex, aliases Aliases) {
	have := make(map[string]bool, len(files))
	for _, f := range files {
		have[f.Path] = true
	}

	for _, f := range files {
		for i, imp := range f.Imports {
			resolved := resolveOne(f.Path, imp, have, aliases)
			if resolved != "" {
				f.Imports[i] = resolved
			}
		}
	}
}

func resolveOne(from, imp string, have map[string]bool, aliases Aliases) string {
	// alias substitution first
	for prefix, target := range aliases {
		if imp == prefix || strings.HasPrefix(imp, prefix+"/") {
			imp = target + strings.TrimPrefix(imp, prefix)
			break
		}
	}

	if !strings.HasPrefix(imp, ".") {
		return ""
	}

	target := filepath.Clean(filepath.Join(filepath.Dir(from), imp))
	if have[target] {
		return target
	}
	for _, e := range candidateExts {
		if have[target+e] {
			return target + e
		}
	}
	for _, e := range candidateExts {
		cand := filepath.Join(target, "index"+e)
		if have[cand] {
			return cand
		}
	}
	// Python package
	if have[filepath.Join(target, "__init__.py")] {
		return filepath.Join(target, "__init__.py")
	}
	return ""
}
