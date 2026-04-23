; tree-sitter-kotlin-ng schema (newer canonical Kotlin grammar).
; Node names differ from the older fwcd grammar the Go version targets:
;   - class / interface / enum all render as `class_declaration`
;   - name fields are `identifier`, not `type_identifier` / `simple_identifier`
;   - `import` (not `import_header`); payload is `qualified_identifier`

(class_declaration
  name: (identifier) @name.definition.class) @definition.class

(function_declaration
  name: (identifier) @name.definition.function) @definition.function

(object_declaration
  name: (identifier) @name.definition.object) @definition.object

; Workaround for a kotlin-ng misparse: a top-level `@Annotation private fun
; Foo() { ... }` can parse as
;   annotated_expression → annotated_expression → infix_expression
;     (identifier "private") (identifier "fun") (call_expression ...)
; rather than a proper `function_declaration`. Extract the function name
; from that shape so @Preview/@Composable composables aren't silently
; dropped. Guarded by `#eq? @_fn_kw "fun"` so non-`fun` infix expressions
; (actual infix operators) don't produce false captures.
(infix_expression
  (identifier)
  (identifier) @_fn_kw
  (call_expression
    (call_expression
      (identifier) @name.definition.function)
    (annotated_lambda))
  (#eq? @_fn_kw "fun")) @definition.function

; ---- tingle additions: package + imports ----
(package_header
  (qualified_identifier) @name.reference.package) @reference.package

(import
  (qualified_identifier) @name.reference.import) @reference.import

; ---- tingle additions: same-package symbol references ----
; Unqualified call targets — `foo()` where `foo` is a top-level decl.
; Qualified calls like `Foo.bar()` have a `navigation_expression` as the
; first child, not an identifier, so this rule won't match them.
(call_expression
  (identifier) @name.reference.symbol)

; Leftmost identifier of any navigation chain — `Foo.bar` → `Foo`,
; `Foo.bar.baz` → `Foo` (innermost nav_expr matches, outer ones don't
; since their first child is another navigation_expression). The anchor
; `.` forces first-named-child position.
(navigation_expression
  . (identifier) @name.reference.symbol)

; Type references — `val x: Foo`, parameter types, return types.
(user_type
  (identifier) @name.reference.symbol)
