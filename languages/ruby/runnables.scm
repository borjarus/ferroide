; RSpec: it / specify / scenario "..."
((call
   method: (identifier) @_m
   arguments: (argument_list . (string (string_content) @name))) @run
  (#any-of? @_m "it" "specify" "scenario" "test")
  (#set! tag "test"))

; RSpec: describe / context "..."
((call
   method: (identifier) @_m
   arguments: (argument_list . [(string (string_content) @name) (constant) @name])) @run
  (#any-of? @_m "describe" "context" "feature")
  (#set! tag "test-module"))

; Minitest: def test_...
((method name: (identifier) @name) @run
  (#match? @name "^test_")
  (#set! tag "test"))
