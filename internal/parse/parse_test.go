package parse_test

import (
	"path/filepath"
	"testing"

	"github.com/charliesbot/tingle/internal/model"
	"github.com/charliesbot/tingle/internal/parse"
)

// Per-language extraction tests guard against regressions in the tags.scm
// queries — specifically the predicate-stripped go-tags.scm and
// javascript-tags.scm files. If someone re-pulls aider's originals without
// stripping the unsupported predicates, parse.All will panic on compile;
// if someone drops a def capture by accident, these tests catch it.

const fixtureDir = "../../testdata/fixtures/langs"

func TestExtraction(t *testing.T) {
	type want struct {
		defs       []string // names expected to appear
		defsMin    int      // minimum total def count (catches "accidentally dropped half the captures")
		imports    []string // import strings expected to appear
		importsMin int
	}

	cases := []struct {
		file string
		ext  string
		want want
	}{
		{
			file: "utils.ts",
			ext:  ".ts",
			want: want{
				// getInputLines, getParagraphs (arrow-assigned-to-const),
				// readFile (direct function_declaration under export),
				// AuthService (interface), Session (type alias),
				// AuthServiceImpl (class) + login + logout methods.
				defs:    []string{"getInputLines", "getParagraphs", "readFile", "AuthService", "Session", "AuthServiceImpl", "login", "logout"},
				defsMin: 6,
			},
		},
		{
			file: "main.go",
			ext:  ".go",
			want: want{
				defs:       []string{"main", "Listen", "Server"},
				defsMin:    3,
				imports:    []string{"fmt", "os", "strings"},
				importsMin: 3,
			},
		},
		{
			file: "Repo.kt",
			ext:  ".kt",
			want: want{
				// UserRepository (interface → class), UserRepositoryImpl (class),
				// topLevelHelper (top-level fun), getAll, insert, provide (methods).
				// KNOWN GAP (see design-doc.md § Known gaps): gotreesitter's Kotlin
				// grammar does not capture `object Foo { ... }` — UserModule is
				// missed. If this test starts reporting UserModule in future,
				// check and update design-doc.md accordingly.
				defs:       []string{"UserRepository", "UserRepositoryImpl", "topLevelHelper", "getAll", "insert", "provide"},
				defsMin:    4,
				imports:    []string{"kotlinx.coroutines.flow.Flow", "kotlinx.coroutines.flow.flow"},
				importsMin: 2,
			},
		},
		{
			file: "reader.h",
			ext:  ".h",
			want: want{
				// FileReader (class), Config (struct), processFile (function).
				defs:       []string{"FileReader", "Config", "processFile"},
				defsMin:    3,
				imports:    []string{"string", "vector"},
				importsMin: 2,
			},
		},
		{
			file: "util.py",
			ext:  ".py",
			want: want{
				// Known gotreesitter grammar gap: a class with a method containing
				// an f-string (e.g. `return f"hello {self.x}"`) can cause subsequent
				// top-level defs to be missed. In util.py, `read_lines` is silently
				// skipped. Tracked in design-doc.md known gaps. `fetch_data` is
				// captured because its body doesn't hit the same parser edge case.
				defs:       []string{"Service", "greet", "fetch_data"},
				defsMin:    3,
				imports:    []string{"os", "typing"},
				importsMin: 2,
			},
		},
	}

	for _, tc := range cases {
		t.Run(tc.file, func(t *testing.T) {
			// Subtests share PackageStats (package-global). NewRun resets
			// the counters so ParseErrors assertions are scoped to this fixture.
			parse.NewRun()

			f := &model.FileIndex{
				Path: tc.file,
				Ext:  tc.ext,
			}
			parse.All(mustAbs(t, fixtureDir), []*model.FileIndex{f})

			// Highlight known gotreesitter grammar gaps in test output so
			// future upgrades make the regression/fix loudly visible.
			switch tc.file {
			case "Repo.kt":
				if containsDefName(f, "UserModule") {
					t.Logf("gotreesitter regression/fix: Kotlin object_declaration now captured! Remove the workaround in design-doc.md § Known gaps and in this test.")
				}
			case "util.py":
				if containsDefName(f, "read_lines") {
					t.Logf("gotreesitter regression/fix: Python f-string-followed-by-def now captured! Remove the workaround in design-doc.md § Known gaps and in this test.")
				}
			}

			if parseErrs := parse.PackageStats.ParseErrors.Load(); parseErrs != 0 {
				t.Fatalf("parse errors: %d", parseErrs)
			}

			if len(f.Defs) < tc.want.defsMin {
				t.Errorf("defs: got %d, want at least %d (defs=%v)", len(f.Defs), tc.want.defsMin, defNames(f))
			}
			for _, want := range tc.want.defs {
				if !containsDefName(f, want) {
					t.Errorf("missing def %q (got: %v)", want, defNames(f))
				}
			}

			if len(f.Imports) < tc.want.importsMin {
				t.Errorf("imports: got %d, want at least %d (imports=%v)", len(f.Imports), tc.want.importsMin, f.Imports)
			}
			for _, want := range tc.want.imports {
				if !containsString(f.Imports, want) {
					t.Errorf("missing import %q (got: %v)", want, f.Imports)
				}
			}
		})
	}
}

func mustAbs(t *testing.T, p string) string {
	t.Helper()
	abs, err := filepath.Abs(p)
	if err != nil {
		t.Fatal(err)
	}
	return abs
}

func defNames(f *model.FileIndex) []string {
	var names []string
	for _, d := range f.Defs {
		names = append(names, d.Name)
		for _, c := range d.Children {
			names = append(names, c.Name)
		}
	}
	return names
}

func containsDefName(f *model.FileIndex, name string) bool {
	for _, d := range f.Defs {
		if d.Name == name {
			return true
		}
		for _, c := range d.Children {
			if c.Name == name {
				return true
			}
		}
	}
	return false
}

func containsString(haystack []string, needle string) bool {
	for _, h := range haystack {
		if h == needle {
			return true
		}
	}
	return false
}
