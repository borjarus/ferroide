; inherits: grammar
; SQL in string literals that start with a statement keyword.
((raw_string_literal (string_content) @injection.content)
  (#match? @injection.content "^\\s*(?i:select|insert|update|delete|with|create|alter|drop)\\s")
  (#set! injection.language "sql"))
((string_literal (string_content) @injection.content)
  (#match? @injection.content "^\\s*(?i:select|insert|update|delete|with|create|alter|drop)\\s")
  (#set! injection.language "sql"))
