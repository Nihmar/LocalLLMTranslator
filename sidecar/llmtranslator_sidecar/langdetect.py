"""Guess the language of a text sample from its function words.

Book metadata is not reliable (a French novel declaring ``dc:language=en`` is common), so the
"new book" form asks the text itself. Articles, prepositions and pronouns make up a large share
of any prose and differ clearly between the languages below, so counting them over a few
thousand words is enough to tell them apart without a model or a dependency.
"""

# The Russian list is Cyrillic on purpose: its letters are not look-alikes of Latin ones.
# ruff: noqa: RUF001

from __future__ import annotations

import re

#: The most frequent function words of each language, space separated.
_WORDS: dict[str, str] = {
    "en": "the of and to a in is was that he it for with as his on at by had but not you",
    "fr": "le la les de des du et un une est que qui il elle dans pour pas ne sur au avec se",
    "it": "il lo la gli le di del della che e un una per non con si è nel alla sono ma",
    "de": "der die das und ist nicht ein eine zu mit sich den dem auf für von ich sie es",
    "es": "el la los las de del que y en un una por con no se es para lo su al",
    "pt": "o a os as de do da que e em um uma para com não se é no na por",
    "nl": "de het een en van in is dat op te zijn met voor niet die aan er maar",
    "ru": "и в не на я что он с как а то это по но она к у из",
}
_FUNCTION_WORDS = {code: frozenset(words.split()) for code, words in _WORDS.items()}

_WORD_RE = re.compile(r"[^\W\d_]+", re.UNICODE)

#: Below this many function words the sample says nothing.
_MIN_HITS = 20
#: The winner must lead the runner-up by this factor to count as a guess.
_MARGIN = 1.3


def guess_language(text: str, max_words: int = 4000) -> str | None:
    """The ISO 639-1 code whose function words dominate ``text``, or ``None`` when unsure."""
    words = [word.lower() for word in _WORD_RE.findall(text)[:max_words]]
    scores = {
        code: sum(1 for word in words if word in vocabulary)
        for code, vocabulary in _FUNCTION_WORDS.items()
    }
    ranked = sorted(scores.items(), key=lambda item: item[1], reverse=True)
    (best, best_score), (_, second_score) = ranked[0], ranked[1]
    if best_score < _MIN_HITS or best_score < second_score * _MARGIN:
        return None
    return best
