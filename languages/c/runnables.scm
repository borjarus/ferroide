((function_definition
   declarator: (function_declarator declarator: (identifier) @name)) @run
  (#eq? @name "main")
  (#set! tag "main"))
