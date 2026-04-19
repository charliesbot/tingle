; Based on aider's go-tags.scm — simplified to remove #strip! and
; #set-adjacent! predicates (unsupported in gotreesitter). We don't consume
; @doc captures, so dropping them has no effect on extraction.

(function_declaration
  name: (identifier) @name.definition.function) @definition.function

(method_declaration
  name: (field_identifier) @name.definition.method) @definition.method

(call_expression
  function: [
    (identifier) @name.reference.call
    (parenthesized_expression (identifier) @name.reference.call)
    (selector_expression field: (field_identifier) @name.reference.call)
    (parenthesized_expression (selector_expression field: (field_identifier) @name.reference.call))
  ]) @reference.call

(type_spec
  name: (type_identifier) @name.definition.type) @definition.type

(type_identifier) @name.reference.type @reference.type

; ---- tingle additions: imports ----
(import_declaration
  (import_spec
    path: (interpreted_string_literal) @name.reference.import)) @reference.import

(import_declaration
  (import_spec_list
    (import_spec
      path: (interpreted_string_literal) @name.reference.import))) @reference.import
