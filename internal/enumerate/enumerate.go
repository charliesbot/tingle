// Package enumerate produces the initial list of files with activity tags.
//
// Primary path: `git ls-files -com --exclude-standard` — inherits .gitignore.
// Fallback: filepath.WalkDir with a baked-in ignore list for repos without .git.
// Either path applies .tingleignore patterns on top.
package enumerate

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	gitignore "github.com/sabhiram/go-gitignore"

	"github.com/charliesbot/tingle/internal/model"
)

// Default ignores for the no-git fallback. Kept intentionally small; users add
// repo-specific excludes via .tingleignore.
var defaultIgnores = []string{
	"node_modules/",
	"dist/",
	"build/",
	".venv/",
	"venv/",
	"target/",
	".next/",
	"out/",
	"coverage/",
	".git/",
}

// Repo scans a repo and returns the list of files with activity tags applied.
// Files are not parsed here; FileIndex.Defs and FileIndex.Imports stay nil.
func Repo(repoPath string) ([]*model.FileIndex, error) {
	repoAbs, err := filepath.Abs(repoPath)
	if err != nil {
		return nil, err
	}

	var paths []string
	gitBacked := hasGit(repoAbs)
	if gitBacked {
		paths, err = gitLsFiles(repoAbs)
		if err != nil {
			return nil, err
		}
	} else {
		paths, err = walkDir(repoAbs)
		if err != nil {
			return nil, err
		}
	}

	ignorer := loadTingleignore(repoAbs)
	if ignorer != nil {
		paths = applyIgnore(paths, ignorer)
	}

	modified := statusSet(repoAbs, "-m")
	untracked := statusSet(repoAbs, "-o", "--exclude-standard")

	files := make([]*model.FileIndex, 0, len(paths))
	for _, p := range paths {
		f := &model.FileIndex{
			Path: p,
			Ext:  strings.ToLower(filepath.Ext(p)),
		}
		if isTestPath(p) {
			f.Tags = append(f.Tags, "test")
		}
		if modified[p] {
			f.Tags = append(f.Tags, "M")
		}
		if untracked[p] {
			f.Tags = append(f.Tags, "untracked")
		}
		files = append(files, f)
	}
	return files, nil
}

func hasGit(repo string) bool {
	info, err := os.Stat(filepath.Join(repo, ".git"))
	return err == nil && (info.IsDir() || !info.IsDir())
}

func gitLsFiles(repo string) ([]string, error) {
	cmd := exec.Command("git", "-C", repo, "ls-files", "-com", "--exclude-standard")
	out, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("git ls-files: %w", err)
	}
	lines := strings.Split(strings.TrimSpace(string(out)), "\n")
	result := make([]string, 0, len(lines))
	for _, l := range lines {
		if l != "" {
			result = append(result, l)
		}
	}
	return result, nil
}

func statusSet(repo string, args ...string) map[string]bool {
	cmd := exec.Command("git", append([]string{"-C", repo, "ls-files"}, args...)...)
	out, err := cmd.Output()
	if err != nil {
		return nil
	}
	set := make(map[string]bool)
	for _, p := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if p != "" {
			set[p] = true
		}
	}
	return set
}

func walkDir(repo string) ([]string, error) {
	var result []string
	err := filepath.WalkDir(repo, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return nil // skip unreadable entries silently; noisy UX otherwise
		}
		rel, relErr := filepath.Rel(repo, path)
		if relErr != nil {
			return nil
		}
		if rel == "." {
			return nil
		}
		for _, ig := range defaultIgnores {
			ig = strings.TrimSuffix(ig, "/")
			if rel == ig || strings.HasPrefix(rel, ig+string(filepath.Separator)) {
				if d.IsDir() {
					return filepath.SkipDir
				}
				return nil
			}
		}
		if d.IsDir() {
			return nil
		}
		result = append(result, rel)
		return nil
	})
	return result, err
}

func loadTingleignore(repo string) *gitignore.GitIgnore {
	path := filepath.Join(repo, ".tingleignore")
	if _, err := os.Stat(path); err != nil {
		return nil
	}
	ig, err := gitignore.CompileIgnoreFile(path)
	if err != nil {
		return nil
	}
	return ig
}

func applyIgnore(paths []string, ig *gitignore.GitIgnore) []string {
	out := paths[:0]
	for _, p := range paths {
		if ig.MatchesPath(p) {
			continue
		}
		out = append(out, p)
	}
	return out
}

func isTestPath(p string) bool {
	lp := strings.ToLower(p)
	switch {
	case strings.Contains(lp, ".test."),
		strings.Contains(lp, ".spec."),
		strings.Contains(lp, "__tests__/"),
		strings.HasSuffix(lp, "_test.go"),
		strings.HasPrefix(lp, "tests/"),
		strings.Contains(lp, "/tests/"):
		return true
	}
	return false
}
