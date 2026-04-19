(module (expression_statement (assignment left: (identifier) @name.definition.constant) @definition.constant))

(class_definition
  name: (identifier) @name.definition.class) @definition.class

(function_definition
  name: (identifier) @name.definition.function) @definition.function

(call
  function: [
      (identifier) @name.reference.call
      (attribute
        attribute: (identifier) @name.reference.call)
  ]) @reference.call

; ---- tingle additions: imports ----
(import_statement
  name: (dotted_name) @name.reference.import) @reference.import

(import_from_statement
  module_name: (dotted_name) @name.reference.import) @reference.import
