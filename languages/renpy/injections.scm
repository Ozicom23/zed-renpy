; `$ ...` one-liners, python / init python block lines, define/default
; values, return values and image displayables are all opaque
; python_content leaf tokens.
((python_content) @injection.content
  (#set! injection.language "python"))

; if/elif/while conditions, menu-choice `if` conditions and `menu set`
; expressions are opaque python_expression leaf tokens.
((python_expression) @injection.content
  (#set! injection.language "python"))

; Bracket groups: screen-language / ATL / style arguments such as
; `action Jump("start")` or `pos (0.5, 1.0)`, label and call parameters.
((bracketed) @injection.content
  (#set! injection.language "python"))
