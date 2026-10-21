; Ren'Py highlights.
; Every node type and anonymous token below must exist in grammar/: a stale
; name makes the whole query fail and Zed then shows no highlighting at all.
; When several patterns capture the same node, the later one wins.

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

; -- Screens, transforms, styles, translations ------------------------
(screen_statement name: (identifier) @function)
(show_screen_statement name: (identifier) @function)
(hide_screen_statement name: (identifier) @function)
(call_screen_statement name: (identifier) @function)
(transform_statement name: (dotted_name) @function)
(testcase_statement name: (identifier) @function)
(style_statement name: (identifier) @type)
(style_statement parent: (identifier) @type)
(translate_statement style: (identifier) @type)
(translate_statement language: (identifier) @attribute)
(translate_statement id: (identifier) @label)
(layeredimage_statement name: (image_name) @constant)
(user_statement name: (identifier) @keyword)

; Screen language, ATL and style lines: a line names a property unless its
; name is a statement or displayable; argument names that are properties
; are highlighted too.
(element name: (identifier) @property)

((element (identifier) @property)
  (#match? @property "^(selected_)?(idle_|hover_|insensitive_|activate_)?(text_|side_|viewport_|scrollbar_|vscrollbar_)?(action|activate_sound|activated|additive|adjust_spacing|adjustment|align|alignaround|allow|allow_underfull|alpha|alt|alternate|alternate_keysym|anchor|anchorangle|anchoraround|anchorradius|angle|antialias|area|around|arrowkeys|auto|axis|background|bar_invert|bar_resizing|bar_vertical|base_bar|black_color|blend|blur|bold|bottom_bar|bottom_gutter|bottom_margin|bottom_padding|bottom_to_top|box|box_align|box_justify|box_layout|box_reverse|box_wrap|box_wrap_spacing|capture|caret|caret_blink|changed|child|child_size|clicked|clipping|color|cols|copypaste|corner1|corner2|crop|crop_relative|debug|default_focus|delay|drag_handle|drag_joined|drag_name|drag_offscreen|drag_raise|draggable|dragged|dragging|drop_allowable|drop_shadow|drop_shadow_color|droppable|dropped|edgescroll|emoji_font|exclude|extra_alt|finished|first_indent|first_spacing|fit|fit_first|focus_mask|font|font_features|foreground|fps|gl_anisotropic|gl_blend_func|gl_color_mask|gl_cull_face|gl_depth|gl_drawable_resolution|gl_mipmap|gl_pixel_perfect|gl_texture_scaling|gl_texture_wrap|gl_texture_wrap_tex0|gl_texture_wrap_tex1|gl_texture_wrap_tex2|gl_texture_wrap_tex3|ground|group_alt|height|hinting|hover_sound|hovered|hyperlink_functions|id|invert_offsets|italic|justify|kerning|key_events|keyboard_focus|keyboard_focus_insets|keysym|layout|left_bar|left_gutter|left_margin|left_padding|line_leading|line_overlap_split|line_spacing|margin|mask|matrixanchor|matrixcolor|matrixtransform|maximum|maxsize|mesh|mesh_pad|min_overlap|min_width|minimum|minwidth|mipmap|modal|mouse|mouse_drop|mousewheel|multiline|nearest|newline_indent|offset|order_reverse|orientation|outline_scaling|outlines|padding|pagekeys|perspective|pixel_width|point_to|pos|predict|prefer_emoji|prefer_screen_to_id|prefer_top|preferred_side|radius|rect|released|repeat|rest_indent|right_bar|right_gutter|right_margin|right_padding|right_to_left|roll_forward|rotate|rotate_pad|rows|ruby_style|scrollbars|selected_hover|selected_idle|selected_insensitive|sensitive|shader|shaper|show_cancels_hide|size|size_group|slow|slow_abortable|slow_cps|slow_cps_multiplier|slow_done|snapped|spacing|strikethrough|style|style_prefix|style_suffix|subpixel|substitute|text_align|text_y_fudge|textalign|textshader|thumb|thumb_align|thumb_offset|thumb_shadow|tooltip|top_bar|top_gutter|top_margin|top_padding|transform_anchor|transpose|underline|unhovered|unscrollable|variant|vertical|width|xadjustment|xalign|xanchor|xanchoraround|xaround|xcenter|xfill|xfit|xinitial|xmargin|xmaximum|xminimum|xoffset|xpadding|xpan|xpos|xrotate|xsize|xspacing|xtile|xycenter|xysize|xzoom|yadjustment|yalign|yanchor|yanchoraround|yaround|ycenter|yfill|yfit|yinitial|ymargin|ymaximum|yminimum|yoffset|ypadding|ypan|ypos|yrotate|ysize|yspacing|ytile|yzoom|zoom|zorder|zpos|zrotate|zzoom)$"))
((style_statement (identifier) @property)
  (#match? @property "^(selected_)?(idle_|hover_|insensitive_|activate_)?(text_|side_|viewport_|scrollbar_|vscrollbar_)?(action|activate_sound|activated|additive|adjust_spacing|adjustment|align|alignaround|allow|allow_underfull|alpha|alt|alternate|alternate_keysym|anchor|anchorangle|anchoraround|anchorradius|angle|antialias|area|around|arrowkeys|auto|axis|background|bar_invert|bar_resizing|bar_vertical|base_bar|black_color|blend|blur|bold|bottom_bar|bottom_gutter|bottom_margin|bottom_padding|bottom_to_top|box|box_align|box_justify|box_layout|box_reverse|box_wrap|box_wrap_spacing|capture|caret|caret_blink|changed|child|child_size|clicked|clipping|color|cols|copypaste|corner1|corner2|crop|crop_relative|debug|default_focus|delay|drag_handle|drag_joined|drag_name|drag_offscreen|drag_raise|draggable|dragged|dragging|drop_allowable|drop_shadow|drop_shadow_color|droppable|dropped|edgescroll|emoji_font|exclude|extra_alt|finished|first_indent|first_spacing|fit|fit_first|focus_mask|font|font_features|foreground|fps|gl_anisotropic|gl_blend_func|gl_color_mask|gl_cull_face|gl_depth|gl_drawable_resolution|gl_mipmap|gl_pixel_perfect|gl_texture_scaling|gl_texture_wrap|gl_texture_wrap_tex0|gl_texture_wrap_tex1|gl_texture_wrap_tex2|gl_texture_wrap_tex3|ground|group_alt|height|hinting|hover_sound|hovered|hyperlink_functions|id|invert_offsets|italic|justify|kerning|key_events|keyboard_focus|keyboard_focus_insets|keysym|layout|left_bar|left_gutter|left_margin|left_padding|line_leading|line_overlap_split|line_spacing|margin|mask|matrixanchor|matrixcolor|matrixtransform|maximum|maxsize|mesh|mesh_pad|min_overlap|min_width|minimum|minwidth|mipmap|modal|mouse|mouse_drop|mousewheel|multiline|nearest|newline_indent|offset|order_reverse|orientation|outline_scaling|outlines|padding|pagekeys|perspective|pixel_width|point_to|pos|predict|prefer_emoji|prefer_screen_to_id|prefer_top|preferred_side|radius|rect|released|repeat|rest_indent|right_bar|right_gutter|right_margin|right_padding|right_to_left|roll_forward|rotate|rotate_pad|rows|ruby_style|scrollbars|selected_hover|selected_idle|selected_insensitive|sensitive|shader|shaper|show_cancels_hide|size|size_group|slow|slow_abortable|slow_cps|slow_cps_multiplier|slow_done|snapped|spacing|strikethrough|style|style_prefix|style_suffix|subpixel|substitute|text_align|text_y_fudge|textalign|textshader|thumb|thumb_align|thumb_offset|thumb_shadow|tooltip|top_bar|top_gutter|top_margin|top_padding|transform_anchor|transpose|underline|unhovered|unscrollable|variant|vertical|width|xadjustment|xalign|xanchor|xanchoraround|xaround|xcenter|xfill|xfit|xinitial|xmargin|xmaximum|xminimum|xoffset|xpadding|xpan|xpos|xrotate|xsize|xspacing|xtile|xycenter|xysize|xzoom|yadjustment|yalign|yanchor|yanchoraround|yaround|ycenter|yfill|yfit|yinitial|ymargin|ymaximum|yminimum|yoffset|ypadding|ypan|ypos|yrotate|ysize|yspacing|ytile|yzoom|zoom|zorder|zpos|zrotate|zzoom)$"))
((show_screen_statement (identifier) @keyword)
  (#match? @keyword "^(nopredict|onlayer|zorder|with|expression|as|pass)$"))
((call_screen_statement (identifier) @keyword)
  (#match? @keyword "^(nopredict|with|expression|pass)$"))

((element name: (identifier) @keyword)
  (#match? @keyword "^(add|areapicker|bar|button|dismiss|drag|draggroup|fixed|frame|grid|has|hbox|hotbar|hotspot|image|imagebutton|imagemap|input|key|label|mousearea|nearrect|null|on|side|text|textbutton|timer|transform|vbar|vbox|viewport|vpgrid|window|use|transclude|if|elif|else|for|in|showif|pass|break|continue|animation|block|choice|contains|event|function|parallel|repeat|time|warp|pause|instant|linear|ease|easein|easeout|(ease|easein|easeout)_(back|bounce|circ|cubic|elastic|expo|quad|quart|quint)|always|group|attribute|run|click|type|exit|skip|assert|until|scroll|drag|move|advance|call|jump)$"))
((element (identifier) @keyword)
  (#match? @keyword "^(if|else|in|not|and|or|at|as)$"))
; has vbox
((element name: (identifier) @_has . (identifier) @keyword)
  (#eq? @_has "has"))

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
  "screen"
  "transform"
  "style"
  "is"
  "translate"
  "strings"
  "old"
  "new"
  "layeredimage"
  "testcase"
  "nvl"
  "clear"
  "early"
  "offset"
  "id"
] @keyword

"$" @keyword

["True" "False"] @boolean

["=" "+=" "|=" "-" "@"] @operator

[":" "."] @punctuation.delimiter

["," "."] @punctuation.delimiter

(operator) @operator

; -- Literals ----------------------------------------------------------
(string) @string
(number) @number
(comment) @comment

; -- Text inside strings ------------------------------------------------
(interpolation) @variable.special
(text_tag) @tag
(escape_sequence) @string.escape
