((function_declaration name: (identifier) @name) @run
  (#eq? @name "main")
  (#set! tag "main"))

((function_declaration name: (identifier) @name) @run
  (#match? @name "^(Test|Benchmark|Example|Fuzz)")
  (#set! tag "test"))
