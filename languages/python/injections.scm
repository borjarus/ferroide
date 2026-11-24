; SQL in strings that start with a statement keyword.
((string (string_content) @injection.content)
  (#match? @injection.content "^\\s*(?i:select|insert|update|delete|with|create|alter|drop)\\s")
  (#set! injection.language "sql"))
