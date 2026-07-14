; Rust tree-sitter tags query.
; Derived from the standard tree-sitter-rust tags.scm (MIT, tree-sitter org)
; and aider's queries/tree-sitter-language-pack/rust-tags.scm.
; Captures: @name.definition.* for defs, @name.reference.* for refs.

; ---- definitions ----

(struct_item
    name: (type_identifier) @name.definition.class) @definition.class

(enum_item
    name: (type_identifier) @name.definition.class) @definition.class

(union_item
    name: (type_identifier) @name.definition.class) @definition.class

(type_item
    name: (type_identifier) @name.definition.class) @definition.class

(trait_item
    name: (type_identifier) @name.definition.interface) @definition.interface

(declaration_list
    (function_item
        name: (identifier) @name.definition.method)) @definition.method

(impl_item
    trait: (type_identifier) @name.definition.interface) @definition.interface

(function_item
    name: (identifier) @name.definition.function) @definition.function

(const_item
    name: (identifier) @name.definition.constant) @definition.constant

(static_item
    name: (identifier) @name.definition.constant) @definition.constant

(mod_item
    name: (identifier) @name.definition.module) @definition.module

(macro_definition
    name: (identifier) @name.definition.macro) @definition.macro

; ---- references ----

(call_expression
    function: (identifier) @name.reference.call) @reference.call

(call_expression
    function: (field_expression
        field: (field_identifier) @name.reference.call)) @reference.call

(call_expression
    function: (scoped_identifier
        name: (identifier) @name.reference.call)) @reference.call

(macro_invocation
    macro: (identifier) @name.reference.call) @reference.call

(type_identifier) @name.reference.type @reference.type
