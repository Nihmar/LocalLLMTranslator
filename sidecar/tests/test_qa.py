"""QA heuristics: one controlled input per finding kind, plus the clean-text case.

Every kind in the contract is exercised by exactly one table row, so a new kind that is
implemented but never reachable fails the suite.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, cast

import pytest
from llmtranslator_sidecar.qa import check

CLEAN_SOURCE = (
    "# Chapter One\n\nThe harbour was quiet that morning, and nobody expected the ship.\n"
)
CLEAN_TARGET = (
    "# Capitolo Uno\n\nIl porto era tranquillo quella mattina, e nessuno aspettava la nave.\n"
)

UNTRANSLATED_TEXT = (
    "The lantern keeper walked along the harbour wall at dawn and saw the ship arrive.\n"
)
LENGTH_SOURCE = (
    "The keeper recorded every vessel that passed the harbour during the long winter watch.\n"
)
LATIN_SOURCE = "The beacon keeper signals the harbour at dawn.\n"
LATIN_TARGET = "灯塔守望者 beacon keeper signals 向港口发出信号。整夜不眠。直到潮水退去。\n"


@dataclass(slots=True)
class Case:
    name: str
    source: str
    target: str
    glossary: dict[str, str]
    placeholders: list[list[Any]]
    expect: set[str]


CASES: list[Case] = [
    Case("clean", CLEAN_SOURCE, CLEAN_TARGET, {"harbour": "porto"}, [], set()),
    Case(
        "empty",
        "The harbour was quiet that morning.\n",
        "   \n",
        {},
        [],
        {"empty"},
    ),
    Case(
        "placeholder_broken",
        "Hello ⟦1⟧world⟦2⟧ and ⟦3⟧more⟦4⟧.\n",
        "Ciao ⟦1⟧mondo⟦2⟧ e ⟦4⟧altro⟦4⟧.\n",
        {},
        [[1, "a"], [2, "b"], [3, "c"], [4, "d"]],
        {"placeholder_broken"},
    ),
    Case(
        "markdown_malformed",
        "## Section\n\nBody text here.\n",
        "# Section\n\nBody text here.\n",
        {},
        [],
        {"markdown_malformed"},
    ),
    Case(
        "untranslated",
        UNTRANSLATED_TEXT,
        UNTRANSLATED_TEXT,
        {},
        [],
        {"untranslated"},
    ),
    Case(
        "duplicate",
        "The ship arrived at dawn.\n",
        "The ship arrived at dawn. The ship arrived at dawn.\n",
        {},
        [],
        {"duplicate"},
    ),
    Case(
        "length_anomaly",
        LENGTH_SOURCE,
        "ok.\n",
        {},
        [],
        {"length_anomaly"},
    ),
    Case(
        "latin_leftover",
        LATIN_SOURCE,
        LATIN_TARGET,
        {},
        [],
        {"latin_leftover"},
    ),
    Case(
        "glossary_mismatch",
        "The harbour was quiet.\n",
        "Il porto era tranquillo.\n",
        {"harbour": "approdo"},
        [],
        {"glossary_mismatch"},
    ),
    Case(
        "glossary_conflict",
        "The harbour and the pier were quiet.\n",
        "Il porto e il molo erano tranquilli.\n",
        {"harbour": "porto", "pier": "porto"},
        [],
        {"glossary_conflict"},
    ),
]


def _findings(result: dict[str, Any]) -> list[dict[str, Any]]:
    return cast("list[dict[str, Any]]", result["findings"])


def _kinds(result: dict[str, Any]) -> set[str]:
    return {finding["kind"] for finding in _findings(result)}


@pytest.mark.parametrize("case", CASES, ids=[case.name for case in CASES])
def test_check_reports_exactly_the_expected_kinds(case: Case) -> None:
    result = check(
        source_text=case.source,
        target_text=case.target,
        glossary=case.glossary,
        placeholders=case.placeholders,
    )
    assert _kinds(result) == case.expect


@pytest.mark.parametrize("case", CASES, ids=[case.name for case in CASES])
def test_findings_have_well_formed_shape(case: Case) -> None:
    result = check(
        source_text=case.source,
        target_text=case.target,
        glossary=case.glossary,
        placeholders=case.placeholders,
    )
    findings = _findings(result)
    assert isinstance(findings, list)
    for finding in findings:
        assert finding["severity"] in {"critical", "major", "minor"}
        assert finding["block_id"] is None
        assert isinstance(finding["details"], dict)


def test_empty_source_and_target_produce_no_findings() -> None:
    result = check(source_text="", target_text="   \n", glossary={}, placeholders=[])
    assert result["findings"] == []


def test_clean_text_has_no_findings() -> None:
    result = check(
        source_text=CLEAN_SOURCE,
        target_text=CLEAN_TARGET,
        glossary={"harbour": "porto"},
        placeholders=[],
    )
    assert result["findings"] == []


def test_placeholder_details_report_missing_and_duplicated() -> None:
    result = check(
        source_text="⟦1⟧⟦2⟧⟦3⟧text\n",
        target_text="⟦1⟧⟦3⟧⟦3⟧testo\n",
        glossary={},
        placeholders=[[1, "a"], [2, "b"], [3, "c"]],
    )
    (finding,) = result["findings"]
    assert finding["kind"] == "placeholder_broken"
    assert finding["severity"] == "critical"
    assert finding["details"] == {"missing": [2], "duplicated": [3]}


def test_markdown_reports_a_block_sequence_change() -> None:
    result = check(
        source_text="first\n\nsecond\n", target_text="first\n", glossary={}, placeholders=[]
    )
    assert _kinds(result) == {"markdown_malformed"}


def test_markdown_reports_a_list_marker_change() -> None:
    result = check(
        source_text="- a\n- b\n", target_text="1. a\n2. b\n", glossary={}, placeholders=[]
    )
    assert _kinds(result) == {"markdown_malformed"}


def test_markdown_reports_a_table_pipe_change() -> None:
    source = "| a | b |\n|---|---|\n| 1 | 2 |\n"
    target = "| a | b |\n|---|---|\n| 1 | 2 | 3 |\n"
    result = check(source_text=source, target_text=target, glossary={}, placeholders=[])
    assert _kinds(result) == {"markdown_malformed"}


def test_markdown_reports_an_unbalanced_code_fence() -> None:
    result = check(
        source_text="```\ncode\n```\n", target_text="```\ncode\n", glossary={}, placeholders=[]
    )
    assert _kinds(result) == {"markdown_malformed"}


def test_severity_matches_the_documented_mapping() -> None:
    assert (
        check(source_text="x\n", target_text=" \n", glossary={}, placeholders=[])["findings"][0][
            "severity"
        ]
        == "critical"
    )
    assert (
        check(
            source_text="a ⟦1⟧b ⟦2⟧c\n",
            target_text="a ⟦1⟧b\n",
            glossary={},
            placeholders=[[1, "x"], [2, "y"]],
        )["findings"][0]["severity"]
        == "critical"
    )
    assert (
        check(
            source_text=LENGTH_SOURCE,
            target_text="ok.\n",
            glossary={},
            placeholders=[],
        )["findings"][0]["severity"]
        == "minor"
    )


def test_latin_leftover_is_major_when_long() -> None:
    phrase = "the beacon keeper signals the harbour entrance every single night without fail"
    source = f"{phrase}.\n"
    cjk = "灯塔守望者发出信号。整夜不眠。反复如此。" * 10
    target = f"{cjk} {phrase} {cjk}\n"

    result = check(source_text=source, target_text=target, glossary={}, placeholders=[])
    leftovers = [finding for finding in result["findings"] if finding["kind"] == "latin_leftover"]
    assert leftovers
    assert all(finding["severity"] == "major" for finding in leftovers)
