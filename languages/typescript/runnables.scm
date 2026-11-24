; test("…", …), it("…", …)
((call_expression
   function: (identifier) @_fn
   arguments: (arguments . (string (string_fragment) @name))) @run
  (#any-of? @_fn "test" "it")
  (#set! tag "test"))

; describe("…", …)
((call_expression
   function: (identifier) @_fn
   arguments: (arguments . (string (string_fragment) @name))) @run
  (#eq? @_fn "describe")
  (#set! tag "test-module"))
