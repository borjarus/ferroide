; if __name__ == "__main__":
((if_statement
   condition: (comparison_operator (identifier) @_name (string)))
  @run
  (#eq? @_name "__name__")
  (#set! tag "main"))

; def test_…
((function_definition name: (identifier) @name) @run
  (#match? @name "^test")
  (#set! tag "test"))

; class Test…
((class_definition name: (identifier) @name) @run
  (#match? @name "^Test")
  (#set! tag "test-module"))
