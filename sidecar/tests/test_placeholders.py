"""Placeholder substitution is what keeps inline Markdown from being corrupted.

If the model can see a URL, it can translate it. If it can see `x = 1`, it can "fix" it.
Lifting those literals out of the string and putting them back afterwards is the only
mechanism in the pipeline that is actually verifiable, which is why the round-trip and the
variant tolerance are both tested here.
"""

from __future__ import annotations

from llmtranslator_sidecar.placeholders import reinject, substitute


def test_untouched_response_round_trips_exactly() -> None:
    text = "Il **re** disse a [Maria](https://example.org/m) di usare `x = 1`[^7]."
    llm_text, placeholders = substitute(text)
    result = reinject(llm_text, placeholders)
    assert result.text == text
    assert result.ok
    assert result.missing == []
    assert result.duplicated == []
    assert not result.reordered


def test_urls_never_reach_the_prompt() -> None:
    text = "Vedi [il documento](https://example.org/very/secret/path.pdf) e http://plain.example"
    llm_text, _ = substitute(text)
    assert "example.org" not in llm_text
    assert "secret" not in llm_text


def test_inline_code_is_opaque() -> None:
    text = "Usa `printf('%s')` per stampare"
    llm_text, placeholders = substitute(text)
    assert "printf" not in llm_text
    assert any(literal == "`printf('%s')`" for _, literal in placeholders)


def test_link_keeps_its_text_translatable_and_its_url_hidden() -> None:
    text = "[the king](https://example.org/k)"
    llm_text, placeholders = substitute(text)
    assert "the king" in llm_text
    assert "example.org" not in llm_text
    literals = [literal for _, literal in placeholders]
    assert literals[0] == "["
    assert literals[1] == "](https://example.org/k)"


def test_emphasis_delimiters_are_placeholders_but_words_are_not() -> None:
    llm_text, placeholders = substitute("**La città**")
    assert "La città" in llm_text
    assert [literal for _, literal in placeholders] == ["**", "**"]


def test_placeholder_numbers_follow_reading_order() -> None:
    llm_text, placeholders = substitute("**a** then `b` then [c](d) and *e*")
    assert [index for index, _ in placeholders] == list(range(1, 8))
    positions = [llm_text.index(f"⟦{index}⟧") for index, _ in placeholders]
    assert positions == sorted(positions)


def test_map_is_a_pure_function_of_the_input() -> None:
    text = "**a** [b](c) `d`"
    assert substitute(text) == substitute(text)


def test_missing_placeholder_is_reported() -> None:
    text = "**bold** and `code`"
    llm_text, placeholders = substitute(text)
    damaged = llm_text.replace("⟦1⟧", "").replace("⟦2⟧", "")
    result = reinject(damaged, placeholders)
    assert not result.ok
    assert result.missing == [1, 2]


def test_duplicated_placeholder_is_reported_and_collapsed() -> None:
    text = "**bold**"
    llm_text, placeholders = substitute(text)
    damaged = llm_text.replace("⟦2⟧", "⟦1⟧")
    result = reinject(damaged, placeholders)
    assert not result.ok
    assert result.duplicated == [1]
    # The first occurrence still stands in, so the text stays usable.
    assert result.text.count("**") == 1


def test_reordering_is_auto_repaired() -> None:
    text = "**a** then `b`"
    llm_text, placeholders = substitute(text)
    # Swap the two opening tokens, a mistake models make when they restructure a clause.
    swapped = llm_text.replace("⟦1⟧", "\x00").replace("⟦3⟧", "⟦1⟧").replace("\x00", "⟦3⟧")
    result = reinject(swapped, placeholders)
    assert result.reordered
    assert result.ok
    assert result.text == text


def test_variant_bracket_spellings_are_accepted() -> None:
    text = "**bold** and `code`"
    _, placeholders = substitute(text)
    # Three tokens: emphasis contributes a delimiter pair, inline code is a single opaque
    # atom. Same content, respelled in each of the ways models actually mangle the token.
    assert [index for index, _ in placeholders] == [1, 2, 3]
    variants = [
        "[1]bold[2] and [3]",
        "\u30101\u3011bold\u30102\u3011 and \u30103\u3011",
        "[[1]]bold[[2]] and [[3]]",
    ]
    for damaged in variants:
        result = reinject(damaged, placeholders)
        assert result.ok, damaged
        assert result.text == text, damaged


def test_a_bare_citation_is_not_claimed_as_a_placeholder() -> None:
    # Only token 1 is expected; the "[3]" in the prose is a citation, not a placeholder.
    _, placeholders = substitute("**bold**")
    result = reinject("As shown in [3], the ⟦1⟧king⟦2⟧ spoke.", placeholders)
    assert result.ok
    assert "[3]" in result.text
    assert "**king**" in result.text


def test_empty_placeholder_map_is_a_no_op() -> None:
    result = reinject("plain text", [])
    assert result.text == "plain text"
    assert result.ok


def test_math_and_footnotes_are_opaque() -> None:
    llm_text, placeholders = substitute("Formula $E = mc^2$ e nota[^12] qui")
    assert "mc^2" not in llm_text
    assert "[^12]" not in llm_text
    literals = [literal for _, literal in placeholders]
    assert "$E = mc^2$" in literals
    assert "[^12]" in literals


def test_front_matter_yaml_is_not_a_placeholder_source() -> None:
    # The YAML block is non-translatable, so the pipeline never builds a chunk whose
    # sendable text is the front matter. Even the pure substitution pass allocates nothing
    # for plain YAML, so a round-trip can never corrupt the document's metadata.
    yaml_block = '---\ntitle: "The Lantern Keeper"\nauthor: Fixture Author\nlang: en\n---'
    llm_text, placeholders = substitute(yaml_block)
    assert placeholders == []
    assert llm_text == yaml_block
    assert reinject(llm_text, placeholders).text == yaml_block


def test_invented_placeholder_index_is_dropped_not_fatal() -> None:
    # A model sometimes renumbers or invents a token. The pass must report it and keep
    # going: raising here turned a validation failure into a sidecar internal error.
    text = "**La città**"
    llm_text, placeholders = substitute(text)
    assert [index for index, _ in placeholders] == [1, 2]
    damaged = llm_text.replace("⟦2⟧", "⟦99⟧")
    result = reinject(damaged, placeholders)
    assert not result.ok
    assert result.unknown == [99]
    assert result.missing == [2]
    assert result.duplicated == []
    assert "⟦" not in result.text
    assert result.text == "**La città"


def test_invented_index_alone_is_reported_and_removed() -> None:
    _, placeholders = substitute("**bold**")
    result = reinject("⟦99⟧bold⟦2⟧", placeholders)
    assert not result.ok
    assert result.unknown == [99]
    assert result.missing == [1]
    assert result.text == "bold**"


def test_unknown_indices_do_not_count_as_duplicates() -> None:
    _, placeholders = substitute("**bold**")
    # Three occurrences of ⟦1⟧: the first stands in, the others are collapsed.
    result = reinject("⟦1⟧a⟦1⟧b⟦1⟧", placeholders)
    assert result.duplicated == [1]
    assert result.unknown == []
    assert result.text == "**ab"
