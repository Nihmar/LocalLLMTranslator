--- pandoc/filters/epub_cleanup.lua
--
-- USER-EDITABLE plain Lua filter (no panflute; Lua 5.4, bundled `pandoc` module).
--
-- Purpose: remove leftover pipeline artefacts and empty paragraphs from the
-- generated HTML/EPUB, so the exported book does not leak internal machinery.
--
-- Removed artefacts
--   * inline `⟦n⟧` placeholders that were not re-injected (safety net);
--   * raw HTML comments such as `<!-- b000417 -->` / `<!-- chunk c000123 -->`
--     used as block/chunk markers by the pipeline;
--   * raw HTML blocks containing placeholder tokens;
--   * spans/divs carrying the marker classes `block-marker`, `chunk-marker`
--     or `pipeline-artefact`;
--   * paragraphs and plain blocks that end up empty/whitespace-only.
--
-- Usage:
--   pandoc input.md -o out.epub --lua-filter=pandoc/filters/epub_cleanup.lua

local PLACEHOLDER = "⟦%d+⟧"
local ARTEFACT_CLASSES = {
  ["block-marker"] = true,
  ["chunk-marker"] = true,
  ["pipeline-artefact"] = true,
}

local function is_blank(block)
  if block.t ~= "Para" and block.t ~= "Plain" then
    return false
  end
  return pandoc.utils.stringify(block):gsub("%s", "") == ""
end

function Str(element)
  local cleaned = element.text:gsub(PLACEHOLDER, "")
  if cleaned ~= element.text then
    element.text = cleaned
  end
  return element
end

function RawInline(element)
  if element.text:match("^%s*<!%-%-.-%-%->%s*$") then
    return pandoc.Inlines({})
  end
  return element
end

function RawBlock(element)
  if element.text:match("<!%-%-.-%-%->") or element.text:match(PLACEHOLDER) then
    return pandoc.Blocks({})
  end
  return element
end

local function has_artefact_class(element)
  for _, class in ipairs(element.classes) do
    if ARTEFACT_CLASSES[class] then
      return true
    end
  end
  return false
end

function Span(element)
  if has_artefact_class(element) then
    return pandoc.Inlines({})
  end
  return element
end

function Div(element)
  if has_artefact_class(element) then
    return pandoc.Blocks({})
  end
  return element
end

function Para(element)
  if is_blank(element) then
    return pandoc.Blocks({})
  end
  return element
end

function Plain(element)
  if is_blank(element) then
    return pandoc.Blocks({})
  end
  return element
end
