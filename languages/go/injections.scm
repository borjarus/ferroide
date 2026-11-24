; SQL in raw strings that start with a statement keyword.
((raw_string_literal (raw_string_literal_content) @injection.content)
  (#match? @injection.content "^\\s*(?i:select|insert|update|delete|with|create|alter|drop)\\s")
  (#set! injection.language "sql"))
