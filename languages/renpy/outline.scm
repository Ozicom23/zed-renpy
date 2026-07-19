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
