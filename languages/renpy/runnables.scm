; A run button beside every label: runs the game warped to it. The label's
; name reaches tasks.json as $ZED_CUSTOM_renpy_label.
(
  (label_statement
    "label" @run
    name: (label_name) @renpy_label)
  (#set! tag renpy-label)
)
