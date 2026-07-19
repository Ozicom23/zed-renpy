; Ren'Py highlights.
; Every node type and anonymous token below is verified against
; tree-sitter-renpy @ 8a98470c0eba8d9c41d12e5a75118fa6aed4cfb7 (v0.4.0).

; -- Labels: definitions and flow targets --------------------------
(label_statement name: (label_name) @function)
(menu_statement name: (label_name) @function)
(jump_statement target: (label_name) @function)
(call_statement target: (label_name) @function)
(call_statement from: (label_name) @function)

; -- Dialogue ------------------------------------------------------
(say_statement who: (dotted_name) @variable)
(say_attribute) @attribute

; -- define / default ----------------------------------------------
(define_statement name: (variable_name) @variable)
(default_statement name: (variable_name) @variable)

; -- Image tags ------------------------------------------------------
(image_name) @constant
(image_specifier) @constant

; -- Transitions and transforms --------------------------------------
(with_statement transition: (simple_expression) @function)
(say_statement transition: (simple_expression) @function)
(show_statement transition: (simple_expression) @function)
(scene_statement transition: (simple_expression) @function)
(hide_statement transition: (simple_expression) @function)
(window_statement transition: (simple_expression) @function)
(show_property transforms: (simple_expression) @function)
(camera_statement transforms: (simple_expression) @function)
(show_layer_statement transforms: (simple_expression) @function)

; -- Channels, stores, layers, aliases -------------------------------
(play_statement channel: (identifier) @variable)
(stop_statement channel: (identifier) @variable)
(queue_statement channel: (identifier) @variable)
(python_block store: (identifier) @variable)
(init_statement store: (identifier) @variable)
(show_property alias: (identifier) @variable)
(show_property layer: (identifier) @variable)
(camera_statement layer: (identifier) @variable)
(show_layer_statement layer: (identifier) @variable)

; -- Keywords ---------------------------------------------------------
[
  "label"
  "menu"
  "jump"
  "call"
  "return"
  "pass"
  "if"
  "elif"
  "else"
  "while"
  "init"
  "python"
  "in"
  "define"
  "default"
  "image"
  "scene"
  "show"
  "hide"
  "with"
  "at"
  "as"
  "behind"
  "onlayer"
  "zorder"
  "expression"
  "layer"
  "camera"
  "window"
  "auto"
  "pause"
  "play"
  "queue"
  "stop"
  "voice"
  "sustain"
  "set"
  "from"
  "rpy"
  "fadein"
  "fadeout"
  "volume"
  "loop"
  "noloop"
  "if_changed"
] @keyword

"$" @keyword

["True" "False"] @boolean

["=" "+=" "|=" "-" "@"] @operator

[":" "."] @punctuation.delimiter

["(" ")" "[" "]"] @punctuation.bracket

; -- Literals (generic, keep last) -------------------------------------
(string) @string
(number) @number
(comment) @comment
