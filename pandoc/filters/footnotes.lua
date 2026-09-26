--- pandoc/filters/footnotes.lua
--
-- USER-EDITABLE plain Lua filter (no panflute, no external modules — only the
-- `pandoc` module bundled with Pandoc; requires Lua 5.4, shipped by Pandoc 3.x).
--
-- Purpose: keep footnote definitions attached to their references and
-- normalise footnote numbering.
--
-- Behaviour
--   * default: every `Note` is cleaned — empty paragraphs and whitespace-only
--     paragraphs are dropped, and an empty note is replaced by a single empty
--     paragraph so Pandoc never emits a dangling reference.  The note stays a
--     real footnote, so Pandoc numbers it sequentially in reading order.
--   * `footnotes-endnotes: true` in the YAML metadata: the notes are lifted out
--     into a single "Notes" section at the end of the document, numbered 1..N
--     in reading order, and each reference becomes a plain `[n]` marker.  This
--     is the deterministic, explicitly numbered form used for print.
--
-- Everything runs inside the `Pandoc` finalizer so the metadata is read before
-- the notes are visited (a `Meta` callback may run after the inline traversal).
--
-- Usage:
--   pandoc input.md -o out.epub --lua-filter=pandoc/filters/footnotes.lua
--   pandoc input.md -o out.pdf  --lua-filter=pandoc/filters/footnotes.lua

local function is_blank_para(block)
  if block.t ~= "Para" and block.t ~= "Plain" then
    return false
  end
  return pandoc.utils.stringify(block):gsub("%s", "") == ""
end

local function metadata_flag(meta, key)
  local value = meta[key]
  if value == nil then
    return false
  end
  return pandoc.utils.stringify(value) ~= "false"
end

function Pandoc(doc)
  local to_endnotes = metadata_flag(doc.meta, "footnotes-endnotes")
  local counter = 0
  local collected = {}

  local walked = doc:walk({
    Note = function(note)
      counter = counter + 1

      local cleaned = pandoc.Blocks({})
      for _, block in ipairs(note.content) do
        if not is_blank_para(block) then
          cleaned:insert(block)
        end
      end
      if #cleaned == 0 then
        cleaned:insert(pandoc.Para({}))
      end

      if to_endnotes then
        collected[#collected + 1] = { number = counter, blocks = cleaned }
        return pandoc.Inlines({ pandoc.Str("[" .. counter .. "]") })
      end

      note.content = cleaned
      return note
    end,
  })

  if not to_endnotes or #collected == 0 then
    return walked
  end

  local section = pandoc.Blocks({ pandoc.Header(1, { pandoc.Str("Notes") }) })
  for _, entry in ipairs(collected) do
    local body = pandoc.Blocks({ pandoc.Para({ pandoc.Str("[" .. entry.number .. "] ") }) })
    for _, block in ipairs(entry.blocks) do
      body:insert(block)
    end
    section:insert(pandoc.Div(body, pandoc.Attr("", { "endnote" })))
  end

  walked.blocks:extend(section)
  return walked
end
