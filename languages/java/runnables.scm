((method_declaration name: (identifier) @name) @run
  (#eq? @name "main")
  (#set! tag "main"))

((method_declaration
   (modifiers [(marker_annotation name: (identifier) @_a) (annotation name: (identifier) @_a)])
   name: (identifier) @name) @run
  (#any-of? @_a "Test" "ParameterizedTest" "RepeatedTest")
  (#set! tag "test"))

((class_declaration name: (identifier) @name) @run
  (#match? @name "Tests?$")
  (#set! tag "test-module"))
