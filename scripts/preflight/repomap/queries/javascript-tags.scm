; JavaScript tree-sitter tags query.
; Derived from the standard tree-sitter-javascript tags.scm (MIT, tree-sitter org)
; and aider's queries/tree-sitter-language-pack/javascript-tags.scm.
; Captures: @name.definition.* for defs, @name.reference.* for refs.

; ---- definitions ----

(function_declaration
    name: (identifier) @name.definition.function) @definition.function

(generator_function_declaration
    name: (identifier) @name.definition.function) @definition.function

(class_declaration
    name: (identifier) @name.definition.class) @definition.class

(method_definition
    name: (property_identifier) @name.definition.method) @definition.method

(variable_declarator
    name: (identifier) @name.definition.function
    value: [(arrow_function) (function_expression)]) @definition.function

(variable_declarator
    name: (identifier) @name.definition.class
    value: (class)) @definition.class

(assignment_expression
    left: (member_expression
        property: (property_identifier) @name.definition.method)
    right: [(arrow_function) (function_expression)]) @definition.method

(pair
    key: (property_identifier) @name.definition.method
    value: [(arrow_function) (function_expression)]) @definition.method

; ---- references ----

(call_expression
    function: (identifier) @name.reference.call) @reference.call

(call_expression
    function: (member_expression
        property: (property_identifier) @name.reference.call)) @reference.call

(new_expression
    constructor: (identifier) @name.reference.class) @reference.class

(identifier) @name.reference.use @reference.use
