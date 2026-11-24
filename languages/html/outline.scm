; Headings, and elements with an id.
((element
   (start_tag (tag_name) @_tag)
   (text) @name) @definition.heading
  (#match? @_tag "^[hH][1-6]$"))

(element
  (start_tag
    (attribute
      (attribute_name) @_id
      (quoted_attribute_value (attribute_value) @name)))
  (#eq? @_id "id")) @definition.element
