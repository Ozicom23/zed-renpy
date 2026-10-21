(label_statement
  "label" @context
  name: (label_name) @name) @item

(menu_statement
  "menu" @context
  name: (label_name)? @name) @item

(define_statement
  "define" @context
  name: (variable_name) @name) @item

(default_statement
  "default" @context
  name: (variable_name) @name) @item

(image_statement
  "image" @context
  name: (image_name) @name) @item

(init_statement
  "init" @context
  "python"? @context) @item

(python_block
  "python" @context) @item

(screen_statement
  "screen" @context
  name: (identifier) @name) @item

(transform_statement
  "transform" @context
  name: (dotted_name) @name) @item

(style_statement
  "style" @context
  name: (identifier) @name) @item

(layeredimage_statement
  "layeredimage" @context
  name: (image_name) @name) @item

(translate_statement
  "translate" @context
  language: (identifier) @name
  id: (identifier)? @name) @item

(testcase_statement
  "testcase" @context
  name: (identifier) @name) @item
