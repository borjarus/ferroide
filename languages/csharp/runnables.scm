((method_declaration name: (identifier) @name) @run
  (#eq? @name "Main")
  (#set! tag "main"))

((method_declaration
   (attribute_list (attribute name: (identifier) @_a))
   name: (identifier) @name) @run
  (#any-of? @_a "Test" "TestCase" "Fact" "Theory" "TestMethod")
  (#set! tag "test"))
