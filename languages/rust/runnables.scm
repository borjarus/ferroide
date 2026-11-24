; fn main
((function_item name: (identifier) @name) @run
  (#eq? @name "main")
  (#set! tag "main"))

; #[test] fn …, #[tokio::test] fn …
((attribute_item
   (attribute [(identifier) @_attr (scoped_identifier name: (identifier) @_attr)]))
  .
  (function_item name: (identifier) @name) @run
  (#eq? @_attr "test")
  (#set! tag "test"))

; #[test] #[ignore] fn …
((attribute_item
   (attribute [(identifier) @_attr (scoped_identifier name: (identifier) @_attr)]))
  .
  (attribute_item)
  .
  (function_item name: (identifier) @name) @run
  (#eq? @_attr "test")
  (#set! tag "test"))

; #[cfg(test)] mod tests
((attribute_item
   (attribute (identifier) @_cfg arguments: (token_tree (identifier) @_test)))
  .
  (mod_item name: (identifier) @name) @run
  (#eq? @_cfg "cfg")
  (#eq? @_test "test")
  (#set! tag "test-module"))
