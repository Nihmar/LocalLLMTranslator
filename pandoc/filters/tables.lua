--- pandoc/filters/tables.lua
--
-- USER-EDITABLE plain Lua filter (no panflute; Lua 5.4, bundled `pandoc` module).
--
-- Purpose: ensure table captions/labels survive the pipeline and column
-- alignment is preserved.
--
-- Behaviour
--   * A paragraph that looks like a table caption ("Table 1: ...", "Table: ...",
--     "Table 3. ...") immediately followed by a table with an empty caption is
--     promoted into that table's caption, so the text is not lost and Pandoc
--     emits it as a real <caption>/`\caption`.
--   * A caption is always ensured to exist (Pandoc objects with a nil caption
--     can trip downstream writers).
--   * If a table arrives with no column specifications at all, one is
--     synthesised from the header row with `AlignDefault`, so the table keeps a
--     column count and does not collapse.  Existing alignments are left
--     untouched and therefore preserved exactly.
--
-- Usage:
--   pandoc input.md -o out.html --lua-filter=pandoc/filters/tables.lua

local function looks_like_caption(block)
  if block.t ~= "Para" then
    return false
  end
  local text = pandoc.utils.stringify(block)
  return text:match("^%s*[Tt]able%s*%d*%s*[:%.]") ~= nil
end

local function caption_is_empty(table_el)
  if table_el.caption == nil then
    return true
  end
  return #table_el.caption.long == 0
end

function Blocks(blocks)
  local out = pandoc.Blocks({})
  local total = #blocks
  local index = 1
  while index <= total do
    local block = blocks[index]
    if looks_like_caption(block)
        and index < total
        and blocks[index + 1].t == "Table"
        and caption_is_empty(blocks[index + 1]) then
      local table_el = blocks[index + 1]
      table_el.caption = pandoc.Caption({ block })
      out:insert(table_el)
      index = index + 2
    else
      out:insert(block)
      index = index + 1
    end
  end
  return out
end

function Table(table_el)
  if table_el.caption == nil then
    table_el.caption = pandoc.Caption({})
  end

  if #table_el.colspecs == 0 and table_el.head ~= nil and #table_el.head.rows > 0 then
    local column_count = #table_el.head.rows[1].cells
    local specs = {}
    for _ = 1, column_count do
      specs[#specs + 1] = { pandoc.AlignDefault, 0.0 }
    end
    table_el.colspecs = specs
  end

  return table_el
end
