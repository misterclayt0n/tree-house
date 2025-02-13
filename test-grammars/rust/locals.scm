; Scopes

[
  (function_item)
  (struct_item)
  (enum_item)
  (union_item)
  (type_item)
  (trait_item)
  (impl_item)
  (closure_expression)
  (block)
] @local.scope

; Definitions

(parameter
  (identifier) @local.definition.variable.parameter)

; TODO: what to highlight for these?
; (type_parameters
;   (type_identifier) @local.definition)
; (constrained_type_parameter
;   left: (type_identifier) @local.definition)
; (optional_type_parameter
;   name: (type_identifier) @local.definition)

(closure_parameters (identifier) @local.definition.variable.parameter)

; References
(identifier) @local.reference
(type_identifier) @local.reference
